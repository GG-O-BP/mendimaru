//! 세션 중계의 공개 계약. DB 계정/암호/연결 문자열 필드는 의도적으로 없다.
use crate::contracts::{BackendError, BackendId, CapabilityId};
use crate::models::AppConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    pub runtime_session_id: String,
    pub service: String,
    pub allow_host_port: u16,
    pub guest_port: u16,
    /// Explicit opt-in: environment variable name, never its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m2ee_password_env: Option<String>,
}

impl StartRequest {
    pub fn validate(&self) -> Result<(), BackendError> {
        if self.service != "mysql"
            || self.allow_host_port != 3306
            || !valid_id(&self.runtime_session_id, "runtime_")
        {
            return Err(BackendError::invalid_request(
                "host-service requires mysql, explicit --allow-host-port 3306, and a Runtime session ID",
            ));
        }
        if self
            .m2ee_password_env
            .as_deref()
            .is_some_and(|name| !valid_password_env(name))
        {
            return Err(BackendError::invalid_request(
                "invalid M2EE password environment variable name",
            ));
        }
        Ok(())
    }
}

pub(crate) fn valid_password_env(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}

pub(crate) fn valid_m2ee_password(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub(crate) fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub schema_version: String,
    pub session_id: String,
    pub runtime_session_id: String,
    pub backend: BackendId,
    pub service: String,
    pub state: String,
    pub guest_host: String,
    pub guest_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub guest_listener_verified: bool,
    pub target_tcp_verified: bool,
    pub mysql_handshake_verified: bool,
    pub database_authentication_verified: bool,
    pub encrypted: bool,
    pub active_connections: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_host_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_host_port: Option<u16>,
    /// Configured translation only; not proof of Runtime authentication.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub m2ee_authentication_configured: bool,
}

pub(crate) async fn start(
    config: &AppConfig,
    request: &StartRequest,
) -> Result<Status, BackendError> {
    request.validate()?;
    #[cfg(target_os = "linux")]
    {
        crate::winboat::host_service::start(config, request).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(BackendError::unsupported(
            native_backend(),
            CapabilityId::HostServiceStart,
        ))
    }
}

pub(crate) async fn inspect(
    config: &AppConfig,
    session: &str,
    stop: bool,
) -> Result<Status, BackendError> {
    if !valid_id(session, "hostsvc_") {
        return Err(BackendError::invalid_request(
            "invalid host-service session ID",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        crate::winboat::host_service::inspect(config, session, stop).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(BackendError::unsupported(
            native_backend(),
            if stop {
                CapabilityId::HostServiceStop
            } else {
                CapabilityId::HostServiceStatus
            },
        ))
    }
}

pub(crate) async fn list(config: &AppConfig) -> Result<Vec<Status>, BackendError> {
    #[cfg(target_os = "linux")]
    {
        crate::winboat::host_service::list(config).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(BackendError::unsupported(
            native_backend(),
            CapabilityId::HostServiceStatus,
        ))
    }
}

#[cfg(not(target_os = "linux"))]
fn native_backend() -> BackendId {
    if cfg!(target_os = "windows") {
        BackendId::WindowsNative
    } else {
        BackendId::MacNative
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn failure(capability: CapabilityId, message: &'static str) -> BackendError {
    BackendError::operation(BackendId::LinuxWinboat, capability, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m2ee_environment_contract_is_explicit_bounded_and_utf8() {
        for name in ["MXCLI_APP_ADMIN_PASSWORD", "_M2EE_2"] {
            assert!(valid_password_env(name));
        }
        for name in ["", "2PASSWORD", "A=B", "PATH/FILE", "암호", "A\nB"] {
            assert!(!valid_password_env(name));
        }
        assert!(valid_m2ee_password("fixture-é-한글"));
        for value in [
            String::new(),
            "x".repeat(257),
            "é".repeat(129),
            "a\nb".into(),
        ] {
            assert!(!valid_m2ee_password(&value));
        }
        assert!(valid_m2ee_password(&"é".repeat(128)));
    }

    #[test]
    fn restricts_destinations_and_never_accepts_credentials() {
        let mut r = StartRequest {
            runtime_session_id: format!("runtime_{}", "a".repeat(32)),
            service: "mysql".into(),
            allow_host_port: 3306,
            guest_port: 0,
            m2ee_password_env: None,
        };
        assert!(r.validate().is_ok());
        for port in [0, 22, 80, 3307] {
            r.allow_host_port = port;
            assert!(r.validate().is_err());
        }
        r.allow_host_port = 3306;
        r.service = "tcp".into();
        assert!(r.validate().is_err());
        let mut v = serde_json::to_value(&r).unwrap();
        v["password"] = "never accepted".into();
        assert!(serde_json::from_value::<StartRequest>(v).is_err());
        assert!(!valid_id("hostsvc_../control", "hostsvc_"));
    }
}
