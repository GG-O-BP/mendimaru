const LAUNCH_STUDIO_TEMPLATE: &str = include_str!("../../scripts/launch_studio.ps1");
const INSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/install_studio.ps1");
const UNINSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/uninstall_studio.ps1");

pub(super) fn launch_studio_script(
    executable_path: &str,
    project_path: Option<&str>,
    windows_report_path: &str,
) -> String {
    LAUNCH_STUDIO_TEMPLATE
        .replace("__EXECUTABLE_PATH__", &powershell_literal(executable_path))
        .replace(
            "__PROJECT_PATH__",
            &powershell_literal(project_path.unwrap_or_default()),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
}

pub(super) fn install_script(
    windows_installer_path: &str,
    windows_report_path: &str,
    install_root: &str,
    version: &str,
) -> String {
    INSTALL_STUDIO_TEMPLATE
        .replace(
            "__INSTALLER_PATH__",
            &powershell_literal(windows_installer_path),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
}

pub(super) fn uninstall_script(
    data_root: &str,
    install_root: &str,
    version: &str,
    windows_report_path: &str,
) -> String {
    UNINSTALL_STUDIO_TEMPLATE
        .replace("__DATA_ROOT__", &powershell_literal(data_root))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
}

pub(super) fn powershell_literal(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::{install_script, launch_studio_script, uninstall_script};

    #[test]
    fn replaces_every_script_placeholder() {
        let launch = launch_studio_script("studio.exe", Some("project.mpr"), "launch.json");
        let install = install_script("setup.exe", "install.json", "install-root", "11.1.0");
        let uninstall = uninstall_script("data-root", "install-root", "11.1.0", "remove.json");
        for script in [launch, install, uninstall] {
            assert!(!script.contains("__"), "unreplaced placeholder in script");
        }
    }
}
