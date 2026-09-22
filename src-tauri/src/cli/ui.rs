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
        Some("reconnect") => Operation::Reconnect,
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
    caller: crate::ui_automation::coordination::Caller,
    reservation: crate::ui_automation::coordination::Reservation,
    reader: &mut tokio::io::Take<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let r = serde_json::from_str::<Request>(line.strip_prefix("ui ").unwrap_or(""));
    let reply = match r {
        Ok(r) if r.session_id == session_id && r.validate().is_ok() => {
            let operation = r.operation;
            let cancellation = crate::process::CancellationToken::default();
            let cancellation_for_job = cancellation.clone();
            let execute = async {
                let paths = AppPaths::discover_for_cli().map_err(|_| {
                    crate::ui_automation::error(operation, "ui-session-unavailable")
                })?;
                let config = crate::application::load_config(&paths).map_err(|_| {
                    crate::ui_automation::error(operation, "ui-session-unavailable")
                })?;
                // Coordination (#152): every accepted request joins the
                // session queue under its keeper-accept arrival, then the
                // desktop foreground scope when it needs one. Both waits stay
                // inside this request's own timeout budget.
                let coordinator = crate::ui_automation::coordination::global();
                let job =
                    crate::ui_automation::coordination::Job::new(reservation.arrival(), &r, caller)
                        .map_err(|_| {
                            crate::ui_automation::error(operation, "ui-invalid-request")
                        })?;
                let deadline = tokio::time::Instant::now() + Duration::from_millis(r.timeout_ms);
                let ticket = coordinator.enqueue(reservation, job)?;
                let mut entered = ticket.admit(&config, deadline, Some(&cancellation)).await?;
                let mode = crate::ui_automation::coordination::vm_mode(entered.job().concurrency);
                let wait = deadline.saturating_duration_since(tokio::time::Instant::now());
                let lease = crate::winboat::vm_use::acquire_for(
                    &config,
                    mode,
                    operation.capability(),
                    wait,
                )
                .await?;
                lease
                    .run(async move {
                        let result = crate::ui_automation::owned_request(
                            &config,
                            &r,
                            Some(&cancellation_for_job),
                        )
                        .await;
                        entered.finish(match &result {
                            Ok(_) => None,
                            Err(failure) => Some(failure.message.as_str()),
                        });
                        result
                    })
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
    // The job's own budget bounds queue admission and the helper phase each,
    // so a queued request may legitimately take up to twice its timeout plus
    // the cancellation-response grace before its reply must arrive.
    let count = tokio::time::timeout(
        Duration::from_millis(request.timeout_ms.saturating_mul(2) + 6500),
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
    fn only_known_uia_diagnostics_survive_cli_sanitization() {
        for (reference, accepted) in [
            (
                "uia:System.Runtime.InteropServices.COMException:-2146233088",
                true,
            ),
            (
                "uia:System.Windows.Automation.ElementNotAvailableException:-2146233079",
                true,
            ),
            ("uia:private.secret:42", false),
            ("uia:System.TimeoutException:2147483648", false),
            ("uia:System.TimeoutException:+1", false),
            ("uia:System.TimeoutException:1:path", false),
            ("uia:System.TimeoutException:1\n", false),
        ] {
            let mut error = crate::ui_automation::error(Operation::Tree, "ui-provider-failed");
            error.diagnostic_ref = Some(reference.into());
            assert_eq!(
                sanitize_backend_error(error).diagnostic_ref.as_deref(),
                accepted.then_some(reference)
            );
        }
    }
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn serve_ui_never_strands_the_queue_after_rejected_requests() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        const SESSION: &str = "studio-4242-639250850131064367";
        let coordinator = crate::ui_automation::coordination::global();
        let caller = crate::ui_automation::coordination::Caller::self_identity();

        async fn keeper_round_trip(
            line: &str,
            session: &str,
            caller: crate::ui_automation::coordination::Caller,
            reservation: crate::ui_automation::coordination::Reservation,
        ) -> serde_json::Value {
            let (caller_side, keeper_side) = tokio::net::UnixStream::pair().expect("pair");
            let (read_half, mut write_half) = keeper_side.into_split();
            let mut reader = BufReader::new(read_half).take(crate::ui_automation::MAX_REQUEST + 16);
            serve_ui(
                line,
                session,
                caller,
                reservation,
                &mut reader,
                &mut write_half,
            )
            .await;
            let mut reply = String::new();
            let mut caller_reader =
                BufReader::new(caller_side).take(crate::ui_automation::MAX_RESPONSE + 1);
            caller_reader
                .read_line(&mut reply)
                .await
                .expect("keeper reply");
            serde_json::from_str(&reply).expect("bounded JSON reply")
        }

        // A request addressed to another session is rejected outright; its
        // keeper-accepted reservation must free its queue position.
        let foreign = Request::new("studio-4243-639250850131064368", Operation::Tree);
        let (_arrival, reservation) = coordinator.reserve(SESSION);
        let line = format!("ui {}\n", serde_json::to_string(&foreign).expect("JSON"));
        let reply = keeper_round_trip(&line, SESSION, caller, reservation).await;
        let error = reply["error"].as_object().expect("rejected with an error");
        assert_eq!(error["message"].as_str(), Some("invalid UI keeper request"));

        // A well-formed request for this keeper resolves to a bounded reply
        // even when the environment cannot serve it.
        let mut valid = Request::new(SESSION, Operation::Tree);
        valid.timeout_ms = 300;
        let (_arrival, reservation) = coordinator.reserve(SESSION);
        let line = format!("ui {}\n", serde_json::to_string(&valid).expect("JSON"));
        let reply = keeper_round_trip(&line, SESSION, caller, reservation).await;
        let error = reply["error"].as_object().expect("bounded failure");
        let message = error["message"].as_str().expect("text");
        assert!(message.starts_with("ui-"), "bounded reason, got {message}");

        // Later arrivals on the same session still admit: neither the
        // rejected request nor the bounded failure stranded the queue.
        let (_arrival, reservation) = coordinator.reserve(SESSION);
        let job =
            crate::ui_automation::coordination::Job::new(reservation.arrival(), &valid, caller)
                .expect("job");
        let ticket = coordinator
            .enqueue(reservation, job)
            .expect("later arrival enqueues");
        let mut entered = ticket
            .admit(
                &fixture_app_config(),
                tokio::time::Instant::now() + std::time::Duration::from_millis(300),
                None,
            )
            .await
            .expect("the queue keeps admitting after rejections");
        entered.finish(None);
    }

    #[cfg(target_os = "linux")]
    fn fixture_app_config() -> crate::models::AppConfig {
        crate::models::AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "fixture".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: format!("serve-ui-fixture-{}", std::process::id()),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 9,
            shared_directory: "/missing".into(),
            windows_shared_directory: "fixture".into(),
            freerdp_binary: "fixture".into(),
            mendix_install_root: "fixture".into(),
            mendix_data_root: "fixture".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }
}
