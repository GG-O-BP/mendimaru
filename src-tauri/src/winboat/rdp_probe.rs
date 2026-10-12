//! Protocol-level readiness of the WinBoat RDP endpoint (#227).
//!
//! A bare TCP connect does not prove that Windows is answering RDP: Docker's
//! userland proxy, rootless port forwarders and QEMU host forwarding accept
//! the TCP handshake on the published host port immediately, even while the
//! container is still being recreated or the guest RDP service is not
//! listening yet. The probe therefore sends an X.224 Connection Request with
//! an RDP negotiation request ([MS-RDPBCGR] 2.2.1.1) and requires an X.224
//! Connection Confirm. It never starts TLS, never authenticates and sends no
//! credentials, so it cannot record or change any certificate trust.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// TPKT + X.224 Connection Request + RDP_NEG_REQ requesting TLS, CredSSP and
/// CredSSP with Early User Authorization (0x0b). Any negotiation outcome,
/// including RDP_NEG_FAILURE, still arrives inside a Connection Confirm.
const CONNECTION_REQUEST: [u8; 19] = [
    0x03, 0x00, 0x00, 0x13, // TPKT version 3, length 19
    0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, // X.224 CR TPDU
    0x01, 0x00, 0x08, 0x00, 0x0b, 0x00, 0x00, 0x00, // RDP_NEG_REQ
];
const MAX_CONFIRM_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeOutcome {
    /// Windows answered the X.224 request with a Connection Confirm.
    Ready,
    /// Nothing accepted the TCP connection.
    Refused,
    /// The connection was accepted and then closed or answered with something
    /// other than RDP, which is what a port forwarder does while the guest
    /// side is not listening.
    NotRdp,
    /// The endpoint did not answer before the probe deadline.
    Timeout,
}

pub(super) async fn probe(host: &str, port: u16, timeout: Duration) -> ProbeOutcome {
    match tokio::time::timeout(timeout, exchange(host, port)).await {
        Ok(outcome) => outcome,
        Err(_) => ProbeOutcome::Timeout,
    }
}

async fn exchange(host: &str, port: u16) -> ProbeOutcome {
    let Ok(mut stream) = tokio::net::TcpStream::connect((host, port)).await else {
        return ProbeOutcome::Refused;
    };
    if stream.write_all(&CONNECTION_REQUEST).await.is_err() {
        return ProbeOutcome::NotRdp;
    }
    let mut header = [0_u8; 4];
    if stream.read_exact(&mut header).await.is_err() {
        return ProbeOutcome::NotRdp;
    }
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if header[0] != 0x03 || header[1] != 0x00 || !(11..=MAX_CONFIRM_BYTES).contains(&length) {
        return ProbeOutcome::NotRdp;
    }
    let mut body = vec![0_u8; length - header.len()];
    if stream.read_exact(&mut body).await.is_err() {
        return ProbeOutcome::NotRdp;
    }
    if is_connection_confirm(&body) {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::NotRdp
    }
}

fn is_connection_confirm(tpdu: &[u8]) -> bool {
    // X.224 length indicator (excluding itself) followed by the CC code.
    tpdu.len() >= 7 && usize::from(tpdu[0]) < tpdu.len() && tpdu[1] & 0xf0 == 0xd0
}

/// Waits until `required` consecutive probes see a Connection Confirm. A
/// single answer right after a recreation is not trusted to be stable.
pub(super) async fn wait_until_stable(
    host: &str,
    port: u16,
    timeout: Duration,
    required: usize,
    interval: Duration,
) -> Result<(), ProbeOutcome> {
    let started = tokio::time::Instant::now();
    let mut consecutive = 0;
    let mut last = ProbeOutcome::Timeout;
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(last);
        }
        last = probe(host, port, remaining.min(Duration::from_secs(3))).await;
        if last == ProbeOutcome::Ready {
            consecutive += 1;
            if consecutive >= required.max(1) {
                return Ok(());
            }
        } else {
            consecutive = 0;
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(last);
        }
        tokio::time::sleep(remaining.min(interval)).await;
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// Connection Confirm with RDP_NEG_RSP selecting TLS, as Windows sends it.
    pub(crate) const CONNECTION_CONFIRM: [u8; 19] = [
        0x03, 0x00, 0x00, 0x13, 0x0e, 0xd0, 0x00, 0x00, 0x12, 0x34, 0x00, 0x02, 0x1f, 0x08, 0x00,
        0x01, 0x00, 0x00, 0x00,
    ];

    #[derive(Clone, Copy)]
    pub(crate) enum Behaviour {
        Confirm,
        CloseImmediately,
        Garbage,
        Silent,
    }

    /// A local stand-in whose answers follow `script` connection by
    /// connection, repeating the last entry.
    pub(crate) async fn fake_endpoint(script: Vec<Behaviour>) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let behaviour = script[index.min(script.len() - 1)];
                tokio::spawn(async move {
                    let mut request = [0_u8; 19];
                    match behaviour {
                        Behaviour::Confirm => {
                            if stream.read_exact(&mut request).await.is_ok()
                                && request == CONNECTION_REQUEST
                            {
                                let _ = stream.write_all(&CONNECTION_CONFIRM).await;
                            }
                        }
                        Behaviour::CloseImmediately => {}
                        Behaviour::Garbage => {
                            let _ = stream.read_exact(&mut request).await;
                            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                        }
                        Behaviour::Silent => {
                            let _ = stream.read_exact(&mut request).await;
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                });
            }
        });
        (port, connections)
    }

    #[tokio::test]
    async fn only_an_x224_connection_confirm_counts_as_ready() {
        let (ready, _) = fake_endpoint(vec![Behaviour::Confirm]).await;
        assert_eq!(
            probe("127.0.0.1", ready, Duration::from_secs(2)).await,
            ProbeOutcome::Ready
        );

        // Docker's proxy accepts and then drops the connection while the
        // guest side is not listening yet.
        let (proxy, _) = fake_endpoint(vec![Behaviour::CloseImmediately]).await;
        assert_eq!(
            probe("127.0.0.1", proxy, Duration::from_secs(2)).await,
            ProbeOutcome::NotRdp
        );

        let (garbage, _) = fake_endpoint(vec![Behaviour::Garbage]).await;
        assert_eq!(
            probe("127.0.0.1", garbage, Duration::from_secs(2)).await,
            ProbeOutcome::NotRdp
        );

        let (silent, _) = fake_endpoint(vec![Behaviour::Silent]).await;
        assert_eq!(
            probe("127.0.0.1", silent, Duration::from_millis(200)).await,
            ProbeOutcome::Timeout
        );

        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(
            probe("127.0.0.1", closed_port, Duration::from_secs(10)).await,
            ProbeOutcome::Refused
        );
    }

    #[tokio::test]
    async fn readiness_requires_consecutive_confirms_after_a_flap() {
        let (port, connections) = fake_endpoint(vec![
            Behaviour::Confirm,
            Behaviour::CloseImmediately,
            Behaviour::Confirm,
            Behaviour::Confirm,
        ])
        .await;
        wait_until_stable(
            "127.0.0.1",
            port,
            Duration::from_secs(5),
            2,
            Duration::from_millis(10),
        )
        .await
        .expect("stable after two consecutive confirms");
        assert_eq!(connections.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn readiness_is_bounded_and_reports_the_last_observation() {
        let (port, _) = fake_endpoint(vec![Behaviour::CloseImmediately]).await;
        let error = wait_until_stable(
            "127.0.0.1",
            port,
            Duration::from_millis(150),
            2,
            Duration::from_millis(10),
        )
        .await
        .expect_err("a forwarder without a guest listener is never ready");
        assert_eq!(error, ProbeOutcome::NotRdp);
    }

    #[test]
    fn the_connection_request_is_a_well_formed_rdp_negotiation() {
        assert_eq!(
            usize::from(u16::from_be_bytes([
                CONNECTION_REQUEST[2],
                CONNECTION_REQUEST[3]
            ])),
            CONNECTION_REQUEST.len()
        );
        assert_eq!(usize::from(CONNECTION_REQUEST[4]), 14);
        assert_eq!(CONNECTION_REQUEST[5], 0xe0);
        assert!(is_connection_confirm(&CONNECTION_CONFIRM[4..]));
        assert!(!is_connection_confirm(&CONNECTION_REQUEST[4..]));
        assert!(!is_connection_confirm(&[0x06, 0xd0]));
    }
}
