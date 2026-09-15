//! A foreground, explicitly opted-in watcher. No RDP/VM lifecycle or system
//! hostname/port changes; NDJSON progress remains visible throughout its life.
use std::ffi::OsString;

const HELP: &str = "Usage: mendimaru assets watch --project-id ID --rewrite-generated-assets\n\
Linux WinBoat only. Run alongside Studio Pro before F5. Rewrites only generated\n\
deployment/web/{layouts,pages} widget imports to relative paths and keeps watching\n\
after rebuilds. Rspack must finish rebuilding before reloading the browser.\n\
Emits NDJSON status until Ctrl+C; an error stops the watcher with exit 1.\n\
Does not edit model/widget sources, configure hosts/ports, or start/stop Studio.\n\
Use project list to find the project ID. See docs/winboat-assets.md.";

pub(super) fn dispatch(arguments: &[OsString]) -> i32 {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        println!("{HELP}");
        return 0;
    }
    let id = match parse(arguments) {
        Ok(id) => id,
        Err(message) => {
            report(false, "invalid_request", message, None);
            return 2;
        }
    };
    #[cfg(target_os = "linux")]
    {
        match tauri::async_runtime::block_on(watch(&id)) {
            Ok(()) => 0,
            Err(message) => {
                report(false, "failed", &message, None);
                1
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        report(
            false,
            "unsupported",
            "asset normalization requires Linux WinBoat",
            None,
        );
        3
    }
}

fn parse(arguments: &[OsString]) -> Result<String, &'static str> {
    let values = arguments
        .iter()
        .map(|value| value.to_str().ok_or("arguments must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    if values.first() != Some(&"watch") {
        return Err("expected assets watch; run assets --help");
    }
    let mut id = None;
    let mut opt_in = false;
    let mut index = 1;
    while index < values.len() {
        match values[index] {
            "--project-id" if id.is_none() => {
                index += 1;
                id = values.get(index).copied();
                if id.is_none() {
                    return Err("--project-id requires a project ID");
                }
            }
            "--rewrite-generated-assets" if !opt_in => opt_in = true,
            _ => return Err("unknown or duplicate asset watcher option; run assets --help"),
        }
        index += 1;
    }
    if !opt_in {
        return Err("--rewrite-generated-assets is required: this command changes generated layout/page imports");
    }
    let id = id.ok_or("--project-id is required")?;
    if !id.strip_prefix("project_").is_some_and(|suffix| {
        suffix.len() == 64 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err("the project ID is invalid");
    }
    Ok(id.to_string())
}

fn report(ok: bool, state: &str, message: &str, counts: Option<serde_json::Value>) {
    use std::io::Write;
    println!(
        "{}",
        serde_json::json!({
            "schemaVersion": crate::contracts::CONTRACT_SCHEMA_VERSION,
            "command": "assets.watch", "ok": ok, "state": state,
            "generatedAssetsRewriteEnabled": !matches!(state, "invalid_request" | "unsupported"), "message": message, "counts": counts,
        })
    );
    let _ = std::io::stdout().flush();
}

#[cfg(target_os = "linux")]
async fn watch(project_id: &str) -> Result<(), String> {
    use crate::winboat::asset_normalizer::Normalizer;
    use std::path::Path;
    crate::i18n::initialize("en-US").map_err(|_| "localization initialization failed")?;
    let paths = crate::app_paths::AppPaths::discover_for_cli()
        .map_err(|_| "application directories are unavailable")?;
    let config = crate::application::load_config(&paths).map_err(|_| {
        "configuration is unavailable; configure the WinBoat shared workspace first"
    })?;
    let project = crate::application::resolve_project(&config, project_id).map_err(|_| {
        "the project ID is unavailable; run project list for the configured shared workspace"
    })?;
    let directory = Path::new(&project.mpr_path)
        .parent()
        .ok_or("the selected project directory is unavailable")?;
    let mut normalizer = Normalizer::new(
        Path::new(&config.shared_directory),
        directory,
        &config.windows_shared_directory,
    )
    .map_err(safe_error)?;
    // Validate the directory tree before announcing that the watcher is active.
    normalizer.scan().map_err(safe_error)?;
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "asset watcher termination handling is unavailable")?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "asset watcher cancellation handling is unavailable")?;
    report(true, "watching", "Generated layout/page widget imports will be rewritten. Wait for Studio's Rspack build to finish before loading the ordinary browser. Keep this process running across F5/rebuilds.", None);
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Consume interval's immediate first tick: require a real quiet interval.
    interval.tick().await;
    loop {
        tokio::select! {
            _ = interrupt.recv() => break,
            _ = signal.recv() => break,
            _ = interval.tick() => {
                let scan = normalizer.scan().map_err(safe_error)?;
                if scan.rewritten_files > 0 {
                    report(true, "normalized", "Generated imports normalized; wait for Rspack to finish bundling the widgets and CSS, then reload the browser.", Some(serde_json::to_value(scan).map_err(|_| "asset status serialization failed")?));
                }
            }
        }
    }
    report(true, "stopped", "Asset watcher stopped. Existing generated repairs remain; start the watcher again before rebuilding.", None);
    Ok(())
}

#[cfg(target_os = "linux")]
fn safe_error(error: std::io::Error) -> String {
    if let Some(message) = crate::winboat::asset_normalizer::diagnostic(&error) {
        format!(
            "Asset watcher stopped: {message}. Check the supported scope in docs/winboat-assets.md."
        )
    } else {
        "Asset watcher stopped: generated files are unavailable, unsafe, or unwritable. Check shared-workspace permissions, remove symlink/hardlink indirection, and restart assets watch before F5; no Studio or VM action was performed.".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use std::ffi::OsString;
    #[cfg(target_os = "linux")]
    #[test]
    fn diagnostic_never_echoes_external_error_paths() {
        let message = super::safe_error(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "/private/fixture secret",
        ));
        assert!(!message.contains("/private/fixture"));
        assert!(!message.contains("secret"));
    }

    #[test]
    fn requires_explicit_opt_in_and_unique_arguments() {
        let id = format!("project_{}", "a".repeat(64));
        let parse_args =
            |args: Vec<&str>| parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>());
        assert!(parse_args(vec!["watch", "--project-id", &id]).is_err());
        assert_eq!(
            parse_args(vec![
                "watch",
                "--project-id",
                &id,
                "--rewrite-generated-assets"
            ])
            .unwrap(),
            id
        );
        assert!(parse_args(vec![
            "watch",
            "--project-id",
            &id,
            "--rewrite-generated-assets",
            "--rewrite-generated-assets"
        ])
        .is_err());
        assert!(parse_args(vec![
            "watch",
            "--project-id",
            "bad",
            "--rewrite-generated-assets"
        ])
        .is_err());
    }
}
