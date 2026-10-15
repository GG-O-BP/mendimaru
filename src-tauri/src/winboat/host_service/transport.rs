//! DB 프로토콜은 해석하지 않는다. 고정 크기 프레임과 방향별 EOF로 TLS 위에서도
//! TCP half-close를 보존하고, 쓰기 완료 후에만 다음 프레임을 읽어 역압을 전달한다.
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_rustls::rustls::{self, pki_types::PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;

pub(super) async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    key: Arc<zeroize::Zeroizing<[u8; 32]>>,
    target: std::net::SocketAddr,
    ready: tokio::sync::mpsc::Sender<u16>,
    active: Arc<AtomicUsize>,
) -> io::Result<()> {
    let capacity = Arc::new(tokio::sync::Semaphore::new(65));
    let data_capacity = Arc::new(tokio::sync::Semaphore::new(64));
    let control_taken = Arc::new(AtomicBool::new(false));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            // 완료됐지만 아직 join하지 않은 작업의 메타데이터까지 65개로 제한한다.
            accepted = listener.accept(), if tasks.len() < 65 => {
                let (socket, _) = accepted?;
                let Ok(permit) = capacity.clone().try_acquire_owned() else { continue; };
                let acceptor = acceptor.clone(); let key = key.clone(); let active = active.clone(); let ready = ready.clone(); let control_taken = control_taken.clone(); let data_capacity = data_capacity.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let opened = tokio::time::timeout(CONNECT_TIMEOUT, async {
                        socket.set_nodelay(true)?;
                        let mut tls = acceptor.accept(socket).await?;
                        let kind = authenticate(&mut tls, key.as_ref()).await?;
                        Ok::<_, io::Error>((tls, kind))
                    }).await;
                    let Ok(Ok((mut tls, kind))) = opened else { return; };
                    if kind == 0 {
                        if control_taken.swap(true, Ordering::SeqCst) { return; }
                        let Ok(Ok(port)) = tokio::time::timeout(CONNECT_TIMEOUT, tls.read_u16()).await else { return; };
                        if port == 0 || ready.send(port).await.is_err() { return; }
                        loop {
                            let pulse = tokio::time::timeout(CONNECT_TIMEOUT, async { tls.write_all(&[0]).await?; tls.flush().await?; tls.read_u8().await }).await;
                            if !matches!(pulse, Ok(Ok(0))) { break; }
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        let _ = ready.send(0).await;
                    } else {
                        let Ok(_data_permit) = data_capacity.try_acquire_owned() else { return; };
                        let Ok(Ok(db)) = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await else { return; };
                        if tls.write_all(&[1]).await.is_err() { return; }
                        struct Count(Arc<AtomicUsize>);
                        impl Drop for Count { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); } }
                        active.fetch_add(1, Ordering::Relaxed); let _count = Count(active);
                        let _ = relay(tls, db).await;
                    }
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

pub(super) async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    key: &[u8; 32],
) -> io::Result<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let kind = stream.read_u8().await?;
    if kind > 1 {
        return Err(io::Error::other("invalid channel"));
    }
    let mut challenge = [0; 33];
    getrandom::fill(&mut challenge[..32]).map_err(io::Error::other)?;
    stream.write_all(&challenge[..32]).await?;
    stream.flush().await?;
    challenge[32] = kind;
    let mut proof = [0; 32];
    stream.read_exact(&mut proof).await?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("fixed HMAC key");
    mac.update(&challenge);
    mac.verify_slice(&proof)
        .map_err(|_| io::Error::other("authentication rejected"))?;
    Ok(kind)
}

pub(super) const FRAME_BYTES: usize = 16 * 1024;
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn tls_server() -> io::Result<(TlsAcceptor, String)> {
    use sha2::{Digest, Sha256};
    let (acceptor, certificate) = tls_identity()?;
    Ok((
        acceptor,
        format!("{:x}", Sha256::digest(certificate.as_ref())),
    ))
}

fn tls_identity() -> io::Result<(TlsAcceptor, rustls::pki_types::CertificateDer<'static>)> {
    let key = rcgen::KeyPair::generate().map_err(io::Error::other)?;
    let cert = rcgen::CertificateParams::new(vec!["mendimaru-host-service".into()])
        .map_err(io::Error::other)?
        .self_signed(&key)
        .map_err(io::Error::other)?;
    let provider = rustls::crypto::ring::default_provider();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .map_err(io::Error::other)?;
    // 재개 티켓과 0-RTT를 쓰지 않는다. 매 연결은 현재 세션 키로 다시 인증한다.
    config.max_early_data_size = 0;
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    Ok((TlsAcceptor::from(Arc::new(config)), cert.der().clone()))
}

async fn encode<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut source: R,
    mut destination: W,
) -> io::Result<()> {
    let mut buffer = [0; FRAME_BYTES + 4];
    loop {
        let count = tokio::time::timeout(IDLE_TIMEOUT, source.read(&mut buffer[4..])).await??;
        // 작은 SQL 응답마다 4-byte 헤더가 별도 TLS 레코드가 되지 않게 합친다.
        buffer[..4].copy_from_slice(&(count as u32).to_be_bytes());
        tokio::time::timeout(IDLE_TIMEOUT, async {
            destination.write_all(&buffer[..count + 4]).await?;
            destination.flush().await
        })
        .await??;
        if count == 0 {
            return Ok(());
        }
    }
}

async fn decode<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut source: R,
    mut destination: W,
) -> io::Result<()> {
    let mut buffer = [0; FRAME_BYTES];
    loop {
        let count = tokio::time::timeout(IDLE_TIMEOUT, source.read_u32()).await?? as usize;
        if count > FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid stream frame",
            ));
        }
        if count == 0 {
            return destination.shutdown().await;
        }
        tokio::time::timeout(IDLE_TIMEOUT, async {
            source.read_exact(&mut buffer[..count]).await?;
            destination.write_all(&buffer[..count]).await
        })
        .await??;
    }
}

pub(super) async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    target: TcpStream,
) -> io::Result<()> {
    target.set_nodelay(true)?;
    let (encrypted_read, encrypted_write) = tokio::io::split(stream);
    let (target_read, target_write) = target.into_split();
    tokio::try_join!(
        decode(encrypted_read, target_write),
        encode(target_read, encrypted_write)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn real_tls_rejects_untrusted_certificates_and_unauthenticated_database_access() {
        let database = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (acceptor, certificate) = tls_identity().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let (ready, _rx) = tokio::sync::mpsc::channel(1);
        let server = tokio::spawn(serve(
            listener,
            acceptor,
            Arc::new(zeroize::Zeroizing::new([42; 32])),
            database.local_addr().unwrap(),
            ready,
            active.clone(),
        ));
        let connector = |certificate| {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(certificate).unwrap();
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
            tokio_rustls::TlsConnector::from(Arc::new(config))
        };
        let (_, unrelated) = tls_identity().unwrap();
        let name = rustls::pki_types::ServerName::try_from("mendimaru-host-service").unwrap();
        assert!(connector(unrelated)
            .connect(name.clone(), TcpStream::connect(address).await.unwrap())
            .await
            .is_err());
        let trusted = connector(certificate);
        let mut unauthenticated = trusted
            .connect(name.clone(), TcpStream::connect(address).await.unwrap())
            .await
            .unwrap();
        unauthenticated.write_u8(1).await.unwrap();
        let mut challenge = [0; 32];
        unauthenticated.read_exact(&mut challenge).await.unwrap();
        unauthenticated.write_all(&[0; 32]).await.unwrap();
        assert!(!matches!(
            tokio::time::timeout(Duration::from_secs(2), unauthenticated.read_u8())
                .await
                .unwrap(),
            Ok(1)
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(30), database.accept())
                .await
                .is_err()
        );

        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut clients = Vec::new();
        let mut targets = Vec::new();
        for _ in 0..4 {
            let mut client = trusted
                .connect(name.clone(), TcpStream::connect(address).await.unwrap())
                .await
                .unwrap();
            client.write_u8(1).await.unwrap();
            let mut challenge = [1; 33];
            client.read_exact(&mut challenge[..32]).await.unwrap();
            let mut mac = Hmac::<Sha256>::new_from_slice(&[42; 32]).unwrap();
            mac.update(&challenge);
            client
                .write_all(&mac.finalize().into_bytes())
                .await
                .unwrap();
            assert_eq!(client.read_u8().await.unwrap(), 1);
            targets.push(database.accept().await.unwrap().0);
            clients.push(client);
        }
        assert_eq!(active.load(Ordering::Relaxed), 4);
        // 인증하지 않고 멈춘 연결이 admission 한도를 채워도 기존 DB 스트림은
        // 진행해야 한다. 이 연결들이 사라지면 새 TLS 연결도 다시 받아야 한다.
        let mut flood = Vec::new();
        for _ in 0..70 {
            flood.push(TcpStream::connect(address).await.unwrap());
        }
        clients[0].write_u32(4).await.unwrap();
        clients[0].write_all(b"load").await.unwrap();
        let mut received = [0; 4];
        tokio::time::timeout(Duration::from_secs(5), targets[0].read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, b"load");
        targets[0].write_all(b"ok").await.unwrap();
        assert_eq!(clients[0].read_u32().await.unwrap(), 2);
        let mut response = [0; 2];
        clients[0].read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"ok");
        drop(flood);
        let recovered = tokio::time::timeout(
            Duration::from_secs(5),
            trusted.connect(name, TcpStream::connect(address).await.unwrap()),
        )
        .await
        .unwrap()
        .unwrap();
        drop(recovered);
        server.abort();
        let _ = server.await;
        for target in &mut targets {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), target.read_u8())
                    .await
                    .unwrap()
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        assert_eq!(active.load(Ordering::Relaxed), 0);
        assert!(std::net::TcpListener::bind(address).is_ok());
    }

    #[tokio::test]
    async fn authentication_rejects_foreign_keys_replay_and_channel_substitution() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut replay = [0u8; 32];
        for attempt in 0..5 {
            let (mut host, mut guest) = tokio::io::duplex(256);
            let server = tokio::spawn(async move { authenticate(&mut host, &[42; 32]).await });
            guest.write_u8(1).await.unwrap();
            let mut challenge = [0; 33];
            guest.read_exact(&mut challenge[..32]).await.unwrap();
            challenge[32] = if attempt == 3 { 0 } else { 1 };
            let key = if attempt == 2 { [43; 32] } else { [42; 32] };
            let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
            mac.update(&challenge);
            let proof: [u8; 32] = mac.finalize().into_bytes().into();
            guest
                .write_all(if attempt == 1 { &replay } else { &proof })
                .await
                .unwrap();
            let result = server.await.unwrap();
            assert_eq!(result.is_ok(), attempt == 0 || attempt == 4);
            if attempt == 0 {
                replay = proof;
            }
        }
    }

    #[tokio::test]
    async fn bounded_frames_preserve_large_binary_payload_and_both_half_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut database, _) = listener.accept().await.unwrap();
        let (wire, mut guest) = tokio::io::duplex(1031);
        let running = tokio::spawn(relay(wire, target));
        let payload: Vec<u8> = (0..100_003).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        let database = tokio::spawn(async move {
            let mut received = Vec::new();
            database.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected);
            database
                .write_all(b"committed after client EOF")
                .await
                .unwrap();
            database.shutdown().await.unwrap();
        });
        for chunk in payload.chunks(FRAME_BYTES) {
            guest.write_u32(chunk.len() as u32).await.unwrap();
            guest.write_all(chunk).await.unwrap();
        }
        guest.write_u32(0).await.unwrap();
        let count = guest.read_u32().await.unwrap() as usize;
        let mut reply = vec![0; count];
        guest.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, b"committed after client EOF");
        assert_eq!(guest.read_u32().await.unwrap(), 0);
        database.await.unwrap();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_and_truncated_frames() {
        for size in [FRAME_BYTES as u32 + 1, 5] {
            let (mut writer, reader) = tokio::io::duplex(32);
            writer.write_u32(size).await.unwrap();
            writer.shutdown().await.unwrap();
            assert!(decode(reader, tokio::io::sink()).await.is_err());
        }
    }
}
