use super::super::security::{authenticate_report, authenticated_envelope};
use super::*;

// 키는 같은 UID의 private Unix IPC 안에서만 전달되고, 공유 채널에는
// Studio 세션의 독립 AES 키로 암호화한 값만 남는다. DB 자격증명은 없다.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Launch {
    pub id: String,
    pub host: Ipv4Addr,
    pub port: u16,
    pub pin: String,
    pub guest_port: u16,
    pub operation_key: String,
}
impl Drop for Launch {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.operation_key.zeroize();
    }
}
impl Launch {
    fn validate(&self) -> Result<(), BackendError> {
        if !crate::host_service::valid_id(&self.id, "hostsvc_")
            || !self.host.is_private()
            || self.port == 0
            || self.pin.len() != 64
            || !self
                .pin
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || STANDARD.decode(&self.operation_key).map(|k| k.len()).ok() != Some(32)
        {
            return Err(fail("invalid host-service Studio launch"));
        }
        Ok(())
    }
}

pub(crate) async fn launch(session: &str, packet: Launch) -> Result<(), BackendError> {
    packet.validate()?;
    let paths = AppPaths::discover_for_cli().map_err(io_error)?;
    let config = crate::config::load_config_from(&paths).map_err(io_error)?;
    let lease =
        super::super::vm_use::acquire(&config, super::super::vm_use::Mode::Exclusive, START)
            .await?;
    lease
        .run(async {
            if packet.host != super::bridge_gateway(&config).await? {
                return Err(fail("host-service Studio endpoint does not match the private VM bridge"));
            }
            let (base, security, sequence) = super::super::ui_channel(session).map_err(io_error)?;
            let path = |suffix: &str| {
                let mut p = base.as_os_str().to_owned();
                p.push(suffix);
                PathBuf::from(p)
            };
            let request = path(".host-service.request");
            let report = path(".host-service.report");
            for p in [&request, &report] {
                if let Ok(meta) = std::fs::symlink_metadata(p) {
                    if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
                        return Err(fail("host-service Studio channel is untrusted"));
                    }
                    std::fs::remove_file(p).map_err(io_error)?;
                }
            }
            struct Files(PathBuf, PathBuf);
            impl Drop for Files {
                fn drop(&mut self) {
                    let _ = std::fs::remove_file(&self.0);
                    let _ = std::fs::remove_file(&self.1);
                }
            }
            let _files = Files(request.clone(), report.clone());
            let sealed = security
                .seal_ui_value(&packet.operation_key)
                .map_err(io_error)?;
            let payload = serde_json::to_vec(&serde_json::json!({
                "id":packet.id,"host":packet.host,"port":packet.port,"pin":packet.pin,
                "guestPort":packet.guest_port,"studioSessionId":session,
                "expiresAt":chrono::Utc::now().timestamp_millis()+30000,"sealedKey":sealed
            }))
            .map_err(io_error)?;
            let bytes = authenticated_envelope(&security, sequence, &payload).map_err(io_error)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&request)
                .map_err(io_error)?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(io_error)?;
            let until = tokio::time::Instant::now() + Duration::from_secs(30);
            while tokio::time::Instant::now() < until {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(&report)
                {
                    Ok(file) => {
                        let m = file.metadata().map_err(io_error)?;
                        if !m.is_file() || m.nlink() != 1 || m.len() > MAX_STATE {
                            return Err(fail("host-service Studio report is untrusted"));
                        }
                        let mut bytes = Vec::new();
                        file.take(MAX_STATE + 1)
                            .read_to_end(&mut bytes)
                            .map_err(io_error)?;
                        let auth = authenticate_report(&bytes, &security).map_err(io_error)?;
                        let value: Value =
                            serde_json::from_slice(&auth.payload).map_err(io_error)?;
                        if value["id"] != packet.id {
                            return Err(fail("host-service Studio report identity mismatch"));
                        }
                        return if value["ok"] == true {
                            Ok(())
                        } else {
                            Err(fail(if value["reason"]=="owned" { "Studio monitor still owns the previous host-service helper; retry after control cleanup" } else { "host-service Studio helper refused startup" }))
                        };
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(e)),
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(fail(
                "host-service Studio helper did not acknowledge startup",
            ))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_launch_is_closed_and_rejects_general_proxy_destinations() {
        let mut packet = Launch {
            id: format!("hostsvc_{}", "a".repeat(32)),
            host: Ipv4Addr::new(172, 20, 0, 1),
            port: 12345,
            pin: "a".repeat(64),
            guest_port: 0,
            operation_key: STANDARD.encode([42; 32]),
        };
        packet.validate().unwrap();
        for address in [
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(8, 8, 8, 8),
        ] {
            packet.host = address;
            assert!(packet.validate().is_err());
        }
        packet.host = Ipv4Addr::new(172, 20, 0, 1);
        packet.operation_key = "invalid".into();
        assert!(packet.validate().is_err());
        let mut json = serde_json::to_value(&packet).unwrap();
        json["password"] = "not-accepted".into();
        assert!(serde_json::from_value::<Launch>(json).is_err());
    }
}
