use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE,
};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub(super) fn system_executable(name: &str) -> Result<PathBuf, String> {
    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(crate::tr!("error-native-operation-executable", path = name));
    }
    let executable = PathBuf::from(OsString::from_wide(&buffer[..length])).join(name);
    if !executable.is_file() {
        return Err(crate::tr!(
            "error-native-operation-executable",
            path = executable.display()
        ));
    }
    Ok(executable)
}

pub(super) fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

pub(super) fn run_elevated(executable: &Path, arguments: &[String]) -> Result<u32, String> {
    if executable.components().count() > 1 && !executable.is_file() {
        return Err(crate::tr!(
            "error-native-operation-executable",
            path = executable.display()
        ));
    }
    let verb = wide("runas");
    let executable_wide = wide(executable.as_os_str());
    let parameters = arguments
        .iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let parameters_wide = wide(&parameters);
    let directory_wide = executable
        .parent()
        .map(|directory| wide(directory.as_os_str()));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = executable_wide.as_ptr();
    info.lpParameters = if arguments.is_empty() {
        std::ptr::null()
    } else {
        parameters_wide.as_ptr()
    };
    info.lpDirectory = directory_wide
        .as_ref()
        .map_or(std::ptr::null(), |directory| directory.as_ptr());
    info.nShow = SW_SHOWNORMAL;

    let launched = unsafe { ShellExecuteExW(&mut info) };
    if launched == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_CANCELLED {
            return Err(crate::tr!("error-native-elevation-cancelled"));
        }
        return Err(crate::tr!("error-native-elevation-start", code = code));
    }
    if info.hProcess.is_null() {
        return Err(crate::tr!("error-native-process-handle"));
    }

    let wait_result = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
    if wait_result != 0 {
        unsafe {
            CloseHandle(info.hProcess);
        }
        return Err(crate::tr!("error-native-process-wait", code = wait_result));
    }
    let mut exit_code = 0_u32;
    let read = unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) };
    unsafe {
        CloseHandle(info.hProcess);
    }
    if read == 0 {
        return Err(crate::tr!(
            "error-native-process-exit-code",
            code = unsafe { GetLastError() }
        ));
    }
    Ok(exit_code)
}

pub(super) fn studio_is_running(executable: &Path) -> Result<bool, String> {
    const SCRIPT: &str = r#"
$target = [IO.Path]::GetFullPath($env:MENDIMARU_PROCESS_PATH)
$running = @(Get-CimInstance Win32_Process -Filter "Name = 'studiopro.exe'" -ErrorAction Stop | Where-Object {
  -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
  [IO.Path]::GetFullPath($_.ExecutablePath).Equals($target, [StringComparison]::OrdinalIgnoreCase)
})
[Console]::Out.Write(($running.Count -gt 0).ToString())
"#;
    let output = hidden_command("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .env("MENDIMARU_PROCESS_PATH", executable)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| crate::tr!("error-native-process-inspect", error = error))?;
    if !output.status.success() {
        return Err(crate::tr!(
            "error-native-process-inspect",
            error = String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .eq_ignore_ascii_case("true"))
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(super) fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_string();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        if character == '"' {
            quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            quoted.push('"');
        } else {
            quoted.extend(std::iter::repeat_n('\\', backslashes));
            quoted.push(character);
        }
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{hidden_command, quote_windows_argument, run_elevated, system_executable};

    #[test]
    fn quotes_windows_arguments_without_shell_interpretation() {
        assert_eq!(quote_windows_argument("/SILENT"), "/SILENT");
        assert_eq!(
            quote_windows_argument(r"C:\Program Files\Mendix\App.mpr"),
            r#""C:\Program Files\Mendix\App.mpr""#
        );
        assert_eq!(quote_windows_argument(""), r#""""#);
        assert_eq!(
            quote_windows_argument("say \"hello\""),
            r#""say \"hello\"""#
        );
    }

    #[test]
    fn refuses_a_missing_elevated_executable_before_showing_uac() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let missing = temporary.path().join("missing-installer.exe");
        assert!(run_elevated(&missing, &[]).is_err());
    }

    #[test]
    fn hidden_command_does_not_allocate_a_powershell_console() {
        const SCRIPT: &str = r#"
Add-Type -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();' -Name ConsoleWindow -Namespace MendimaruTest
[Console]::Out.Write(([MendimaruTest.ConsoleWindow]::GetConsoleWindow() -eq [IntPtr]::Zero).ToString())
"#;
        let output = hidden_command("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                SCRIPT,
            ])
            .output()
            .expect("run hidden PowerShell probe");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "True");
    }

    #[test]
    fn resolves_windows_installer_from_the_protected_system_directory() {
        let executable = system_executable("msiexec.exe").expect("system Windows Installer");
        assert!(executable.is_file());
        assert!(executable
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("msiexec.exe")));
    }
}
