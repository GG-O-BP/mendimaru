use super::*;
use crate::contracts::UiActionKind;
use crate::ui_automation::{Operation, Request, Selector};

pub(super) fn parse_ui(values: &[String]) -> Result<CliCommand, BackendError> {
    let operation = match values.first().map(String::as_str) {
        Some("capabilities") => Operation::Capabilities,
        Some("tree") => Operation::Tree,
        Some("find") => Operation::Find,
        Some("action") => Operation::Action,
        Some("wait") => Operation::Wait,
        Some("screenshot") => Operation::Screenshot,
        Some("release") => Operation::Release,
        _ => {
            return Err(BackendError::invalid_request(
                "expected a supported UI command",
            ))
        }
    };
    let (options, flags) = parse_options(
        &values[1..],
        &[
            "--session-id",
            "--timeout-ms",
            "--role",
            "--name",
            "--automation-id",
            "--scope-id",
            "--element-id",
            "--action",
            "--key",
            "--condition",
            "--window-id",
            "--region",
        ],
        &["--value-stdin"],
    )?;
    let mut r = Request::new(&required_map_option(&options, "--session-id")?, operation);
    if let Some(v) = options.get("--timeout-ms") {
        r.timeout_ms = v
            .parse()
            .map_err(|_| BackendError::invalid_request("invalid UI timeout"))?;
    }
    if ["--role", "--name", "--automation-id", "--scope-id"]
        .iter()
        .any(|key| options.contains_key(*key))
    {
        r.selector = Some(Selector {
            role: options.get("--role").cloned(),
            name: options.get("--name").cloned(),
            automation_id: options.get("--automation-id").cloned(),
            scope_id: options.get("--scope-id").cloned(),
        });
    }
    r.element_id = options.get("--element-id").cloned();
    r.action = options
        .get("--action")
        .map(|v| match v.as_str() {
            "invoke" => Ok(UiActionKind::Invoke),
            "click" => Ok(UiActionKind::Click),
            "focus" => Ok(UiActionKind::Focus),
            "set-value" => Ok(UiActionKind::SetValue),
            "keyboard-input" => Ok(UiActionKind::KeyboardInput),
            _ => Err(BackendError::invalid_request("invalid UI action")),
        })
        .transpose()?;
    let read_value = flags.contains("--value-stdin");
    if read_value != (r.action == Some(UiActionKind::SetValue))
        || (options.contains_key("--key") && r.action != Some(UiActionKind::KeyboardInput))
    {
        return Err(BackendError::invalid_request(
            "set-value requires --value-stdin; keyboard-input requires --key",
        ));
    }
    r.value = if read_value {
        Some(String::new())
    } else {
        options.get("--key").cloned()
    };
    r.condition = options.get("--condition").cloned();
    r.window_id = options.get("--window-id").cloned();
    r.region = options
        .get("--region")
        .map(|v| {
            let values = v
                .split(',')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| BackendError::invalid_request("invalid screenshot region"))?;
            values
                .try_into()
                .map_err(|_| BackendError::invalid_request("expected x,y,width,height"))
        })
        .transpose()?;
    r.validate()?;
    Ok(CliCommand::Ui {
        request: r,
        read_value,
    })
}

pub(super) async fn run_ui(
    config: &crate::models::AppConfig,
    request: &Request,
    read_value: bool,
) -> Result<CommandOutput, CommandError> {
    let mut request = request.clone();
    if read_value {
        use tokio::io::AsyncReadExt;
        let mut value = Vec::new();
        tokio::io::stdin()
            .take(258)
            .read_to_end(&mut value)
            .await
            .map_err(|_| BackendError::invalid_request("UI value input failed"))?;
        let value = String::from_utf8(value)
            .map_err(|_| BackendError::invalid_request("invalid UI value input"))?;
        request.value = Some(
            value
                .strip_suffix('\n')
                .unwrap_or(&value)
                .trim_end_matches('\r')
                .into(),
        );
    }
    request.validate()?;
    let data = crate::ui_automation::execute(config, &request).await?;
    let mut output = CommandOutput::data(data)?;
    output.studio_session_id = Some(request.session_id);
    Ok(output)
}

#[cfg(target_os = "linux")]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UiReply {
    data: Option<Value>,
    error: Option<BackendError>,
}

#[cfg(target_os = "linux")]
pub(super) async fn serve_ui(
    line: &str,
    session_id: &str,
    reader: &mut tokio::io::Take<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let r = serde_json::from_str::<Request>(line.strip_prefix("ui ").unwrap_or(""));
    let reply = match r {
        Ok(r) if r.session_id == session_id && r.validate().is_ok() => {
            let operation = r.operation;
            let cancellation = crate::process::CancellationToken::default();
            let execute = async {
                let paths = AppPaths::discover_for_cli().map_err(|_| {
                    crate::ui_automation::error(operation, "ui-session-unavailable")
                })?;
                let config = crate::application::load_config(&paths).map_err(|_| {
                    crate::ui_automation::error(operation, "ui-session-unavailable")
                })?;
                let lease = crate::winboat::vm_use::acquire(
                    &config,
                    crate::winboat::vm_use::Mode::Exclusive,
                    operation.capability(),
                )
                .await?;
                lease
                    .run(crate::ui_automation::owned_request(&r, Some(&cancellation)))
                    .await
            };
            tokio::pin!(execute);
            let mut unexpected = [0u8; 1];
            // Caller EOF requests cancellation. Keep the VM lease until the
            // guest acknowledges cancellation or its bounded deadline expires.
            let result = tokio::select! {
                value = &mut execute => value,
                _ = reader.read(&mut unexpected) => { cancellation.cancel(); execute.await },
            };
            match result {
                Ok(data) => UiReply {
                    data: Some(data),
                    error: None,
                },
                Err(error) => UiReply {
                    data: None,
                    error: Some(error),
                },
            }
        }
        _ => UiReply {
            data: None,
            error: Some(BackendError::invalid_request("invalid UI keeper request")),
        },
    };
    if let Ok(mut payload) = serde_json::to_vec(&reply) {
        payload.push(b'\n');
        let _ = tokio::time::timeout(Duration::from_secs(3), writer.write_all(&payload)).await;
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn request_keeper_ui(
    paths: &AppPaths,
    request: &Request,
) -> Result<Value, BackendError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let op = request.operation;
    let failure = || crate::ui_automation::error(op, "ui-session-unavailable");
    let directory = ensure_session_socket_directory(paths).map_err(|_| failure())?;
    let path = directory.join(session_socket_name(&request.session_id));
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| failure())?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(crate::ui_automation::error(op, "ui-bridge-untrusted"));
    }
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(path),
    )
    .await
    .map_err(|_| failure())?
    .map_err(|_| failure())?;
    if stream.peer_cred().map_err(|_| failure())?.uid() != unsafe { libc::geteuid() } {
        return Err(failure());
    }
    let payload = format!(
        "ui {}\n",
        serde_json::to_string(request).map_err(|_| failure())?
    );
    stream
        .write_all(payload.as_bytes())
        .await
        .map_err(|_| failure())?;
    let mut reader = BufReader::new(stream).take(crate::ui_automation::MAX_RESPONSE + 1);
    let mut line = String::new();
    let count = tokio::time::timeout(
        Duration::from_millis(request.timeout_ms + 6500),
        reader.read_line(&mut line),
    )
    .await
    .map_err(|_| crate::ui_automation::error(op, "ui-helper-timeout"))?
    .map_err(|_| failure())?;
    if count as u64 > crate::ui_automation::MAX_RESPONSE || !line.ends_with('\n') {
        return Err(failure());
    }
    let reply: UiReply = serde_json::from_str(&line).map_err(|_| failure())?;
    match (reply.data, reply.error) {
        (Some(data), None) => Ok(data),
        (None, Some(error)) => Err(error),
        _ => Err(failure()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_keeps_values_off_argv_and_rejects_arbitrary_commands() {
        let base = [
            "action",
            "--session-id",
            "studio-4242-639250850131064367",
            "--element-id",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:1.2",
            "--action",
            "set-value",
        ];
        let mut args = base.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert!(parse_ui(&args).is_err());
        args.push("--value-stdin".into());
        assert!(parse_ui(&args).is_ok());
        for extra in [
            vec!["--value", "secret"],
            vec!["--script", "secret.ps1"],
            vec!["--process-id", "123"],
            vec!["--key", "F5"],
        ] {
            let mut bad = args.clone();
            bad.extend(extra.into_iter().map(str::to_string));
            assert!(parse_ui(&bad).is_err());
        }
        let args = [
            "wait",
            "--session-id",
            "studio-4242-639250850131064367",
            "--condition",
            "running",
            "--timeout-ms",
            "60000",
        ]
        .map(str::to_string);
        assert!(parse_ui(&args).is_ok());
    }
}
