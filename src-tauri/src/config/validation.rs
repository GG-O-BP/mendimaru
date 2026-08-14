use crate::models::AppConfig;
use std::path::Path;

pub(crate) fn normalize_and_validate(config: &mut AppConfig) -> Result<(), String> {
    config.language_preference = crate::i18n::normalize_preference(&config.language_preference)?;
    config.winboat_executable = super::expand_home(config.winboat_executable.trim());
    config.compose_file = super::expand_home(config.compose_file.trim());
    config.shared_directory = super::expand_home(config.shared_directory.trim());
    config.freerdp_binary = super::expand_home(config.freerdp_binary.trim());
    config.api_url = config.api_url.trim_end_matches('/').trim().to_string();
    config.rdp_host = config.rdp_host.trim().to_string();
    config.windows_shared_directory = config
        .windows_shared_directory
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_install_root = config
        .mendix_install_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_data_root = config
        .mendix_data_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.container_name = config.container_name.trim().to_string();

    let shared = Path::new(&config.shared_directory);
    if !shared.is_absolute() || !shared.is_dir() {
        return Err(crate::tr!("error-shared-directory-invalid"));
    }
    config.shared_directory = shared
        .canonicalize()
        .map_err(|error| crate::tr!("error-shared-directory-inspect", error = error))?
        .to_string_lossy()
        .to_string();

    if config.compose_file.is_empty()
        || config.container_name.is_empty()
        || config.api_url.is_empty()
        || config.rdp_host.is_empty()
        || config.windows_shared_directory.is_empty()
    {
        return Err(crate::tr!("error-winboat-connection-empty"));
    }
    if config.startup_timeout_seconds == 0 || config.startup_timeout_seconds > 900 {
        return Err(crate::tr!(
            "error-startup-timeout-range",
            minimum = crate::i18n::format_number(1),
            maximum = crate::i18n::format_number(900)
        ));
    }
    Ok(())
}
