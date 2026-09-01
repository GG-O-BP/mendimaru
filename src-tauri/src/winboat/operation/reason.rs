use super::report::WindowsOperationState;

pub(in crate::winboat) fn localize_windows_reason(reason: &str) -> String {
    if let Some(path) = reason.strip_prefix("MENDIMARU_STUDIO_EXECUTABLE_NOT_FOUND:") {
        return crate::tr!("error-script-studio-executable-not-found", path = path);
    }
    if let Some(code) = reason.strip_prefix("MENDIMARU_STUDIO_EXITED_BEFORE_WINDOW:") {
        return crate::tr!(
            "error-script-studio-exited-before-window",
            code = localize_numeric_text(code)
        );
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_INSTALLER_NOT_FOUND:") {
        return crate::tr!("error-script-installer-not-found", path = path);
    }
    if let Some(code) = reason.strip_prefix("MENDIMARU_INSTALLER_EXIT_CODE:") {
        return crate::tr!(
            "error-script-installer-exit-code",
            code = localize_numeric_text(code)
        );
    }
    if let Some(version) = reason.strip_prefix("MENDIMARU_STUDIO_NOT_CREATED:") {
        return crate::tr!("error-script-studio-not-created", version = version);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_PARTIAL_CLEANUP_FAILED:") {
        return crate::tr!("error-script-partial-cleanup-failed", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_UNINSTALLER_NOT_FOUND:") {
        return crate::tr!("error-script-uninstaller-not-found", path = path);
    }
    if let Some(code) = reason.strip_prefix("MENDIMARU_UNINSTALLER_EXIT_CODE:") {
        return crate::tr!(
            "error-script-uninstaller-exit-code",
            code = localize_numeric_text(code)
        );
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_UNINSTALL_STILL_EXISTS:") {
        return crate::tr!("error-script-uninstall-still-exists", path = path);
    }
    if let Some(version) = reason.strip_prefix("MENDIMARU_UNINSTALL_METADATA_MISSING:") {
        return crate::tr!("error-script-uninstall-metadata-missing", version = version);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_EXECUTABLE_NOT_FOUND:") {
        return crate::tr!("error-script-executable-not-found", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_DIRECTORY_NOT_FOUND:") {
        return crate::tr!("error-script-directory-not-found", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_PATH_OUTSIDE_TRUST_ROOT:") {
        return crate::tr!("error-script-path-outside-root", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_REPARSE_POINT:") {
        return crate::tr!("error-script-reparse-point", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_EXECUTABLE_INVALID:") {
        return crate::tr!("error-script-executable-invalid", path = path);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_HASH_MISMATCH:") {
        return crate::tr!("error-script-hash-mismatch", path = path);
    }
    if let Some(reason) = reason.strip_prefix("MENDIMARU_SIGNATURE_INVALID:") {
        return crate::tr!("error-script-signature-invalid", reason = reason);
    }
    if let Some(publisher) = reason.strip_prefix("MENDIMARU_PUBLISHER_INVALID:") {
        return crate::tr!("error-script-publisher-invalid", publisher = publisher);
    }
    if let Some(path) = reason.strip_prefix("MENDIMARU_EXECUTABLE_CHANGED:") {
        return crate::tr!("error-script-executable-changed", path = path);
    }
    match reason {
        "MENDIMARU_STUDIO_WINDOW_TIMEOUT" => crate::tr!(
            "error-script-studio-window-timeout",
            minutes = crate::i18n::format_number(4)
        ),
        "MENDIMARU_ADMIN_REQUIRED" => crate::tr!("error-script-admin-required"),
        "MENDIMARU_PROJECT_STILL_OPEN" => crate::tr!("error-script-project-still-open"),
        "MENDIMARU_STUDIO_STILL_RUNNING" | "MENDIMARU_STUDIO_RUNNING" => {
            crate::tr!("error-script-studio-still-running")
        }
        "MENDIMARU_STUDIO_SESSION_ENDED" => crate::tr!("error-script-studio-session-ended"),
        "MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE" => {
            crate::tr!("error-script-studio-session-window-unavailable")
        }
        "MENDIMARU_STUDIO_SESSION_CLOSE_PENDING" => {
            crate::tr!("error-script-studio-session-close-pending")
        }
        "MENDIMARU_STUDIO_SESSION_CLOSE_REJECTED" => {
            crate::tr!("error-script-studio-session-close-rejected")
        }
        "MENDIMARU_STUDIO_SESSION_QUERY_FAILED" => {
            crate::tr!("error-script-studio-session-query-failed")
        }
        "MENDIMARU_STUDIO_SESSION_ENUMERATION_FAILED" => {
            crate::tr!("error-script-studio-session-enumeration-failed")
        }
        "MENDIMARU_STUDIO_SESSION_REPORT_FAILED" => {
            crate::tr!("error-script-studio-session-report-failed")
        }
        "MENDIMARU_PROJECT_NOT_READY" => crate::tr!("error-script-project-not-ready"),
        "MENDIMARU_PROJECT_INVALID" => crate::tr!("error-script-project-invalid"),
        "MENDIMARU_PATH_INVALID" => crate::tr!("error-script-path-invalid"),
        _ => reason.to_string(),
    }
}

pub(super) fn localize_operation_state(state: WindowsOperationState) -> String {
    match state {
        WindowsOperationState::Starting => crate::tr!("operation-state-starting"),
        WindowsOperationState::Running => crate::tr!("operation-state-running"),
        WindowsOperationState::Succeeded => crate::tr!("operation-state-succeeded"),
        WindowsOperationState::Failed => crate::tr!("operation-state-failed"),
        _ => state.as_str().to_string(),
    }
}

fn localize_numeric_text(value: &str) -> String {
    value
        .parse::<u64>()
        .map(crate::i18n::format_number)
        .unwrap_or_else(|_| value.to_string())
}
