use super::*;

pub(super) fn parse(values: &[String]) -> Result<CliCommand, BackendError> {
    let (options, _) = parse_options(
        values,
        &[
            "--session-id",
            "--since",
            "--until",
            "--max-bytes",
            "--max-lines",
            "--cursor",
        ],
        &[],
    )?;
    let number = |key: &str, default: u32| -> Result<u32, BackendError> {
        options
            .get(key)
            .map(|v| {
                v.parse()
                    .map_err(|_| BackendError::invalid_request("invalid Studio log bound"))
            })
            .unwrap_or(Ok(default))
    };
    let logs = crate::studio_logs::LogOptions {
        max_bytes: number("--max-bytes", 65536)?,
        max_lines: number("--max-lines", 200)?,
        since: options.get("--since").cloned(),
        until: options.get("--until").cloned(),
        cursor: options.get("--cursor").cloned(),
    };
    let mut request = crate::ui_automation::Request::new(
        &required_map_option(&options, "--session-id")?,
        crate::ui_automation::Operation::StudioLogs,
    );
    request.logs = Some(logs);
    request.validate()?;
    Ok(CliCommand::StudioLogs { request })
}

pub(super) async fn run(
    config: &crate::models::AppConfig,
    request: &crate::ui_automation::Request,
) -> Result<CommandOutput, CommandError> {
    let data = crate::ui_automation::execute(config, request).await?;
    let page = crate::studio_logs::validate_page(request, data)?;
    let mut output = CommandOutput::data(page)?;
    output.studio_session_id = Some(request.session_id.clone());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_reject_paths_commands_and_unbounded_reads() {
        let base = ["--session-id", "studio-4242-639250850131064367"];
        let args = |extra: &[&str]| {
            base.iter()
                .chain(extra.iter())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };
        assert!(parse(&args(&[])).is_ok());
        for extra in [
            vec!["--path", "C:\\secret"],
            vec!["--script", "whoami"],
            vec!["--max-bytes", "262145"],
            vec!["--max-lines", "0"],
            vec!["--since", "yesterday"],
            vec!["--cursor", "../../secret"],
        ] {
            assert!(parse(&args(&extra)).is_err());
        }
    }
}
