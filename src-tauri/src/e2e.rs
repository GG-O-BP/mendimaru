use std::path::PathBuf;

const ROOT_ENVIRONMENT_VARIABLE: &str = "MENDIMARU_E2E_ROOT";
const ROOT_MARKER_FILE: &str = ".mendimaru-e2e-root";
const ROOT_MARKER_CONTENT: &str = "mendimaru isolated native e2e\n";

pub(crate) fn require_isolated_root() -> Result<PathBuf, String> {
    let configured = std::env::var_os(ROOT_ENVIRONMENT_VARIABLE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{ROOT_ENVIRONMENT_VARIABLE} is required for an e2e build"))?;
    if !configured.is_absolute() || !configured.is_dir() {
        return Err(format!(
            "{ROOT_ENVIRONMENT_VARIABLE} must name an existing absolute directory"
        ));
    }
    let canonical = configured
        .canonicalize()
        .map_err(|error| format!("failed to inspect {ROOT_ENVIRONMENT_VARIABLE}: {error}"))?;
    let canonical = display_path(canonical);
    let marker = std::fs::read_to_string(canonical.join(ROOT_MARKER_FILE))
        .map_err(|_| format!("{ROOT_ENVIRONMENT_VARIABLE} is missing its safety marker"))?;
    if marker != ROOT_MARKER_CONTENT {
        return Err(format!(
            "{ROOT_ENVIRONMENT_VARIABLE} has an invalid safety marker"
        ));
    }
    Ok(canonical)
}

fn display_path(path: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path
}

pub(crate) fn directory(name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("invalid e2e directory name".to_string());
    }
    Ok(require_isolated_root()?.join(name))
}

#[cfg(test)]
mod tests {
    use super::directory;

    #[test]
    fn rejects_directory_traversal() {
        assert!(directory("../outside").is_err());
    }
}
