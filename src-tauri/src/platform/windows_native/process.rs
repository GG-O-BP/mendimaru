use crate::process::{
    CancellationToken, CommandFailure, CommandFailureKind, CommandPolicy, WindowsJob,
};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_CANCELLED, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, OpenProcess, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
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

pub(super) fn run_elevated(
    executable: &Path,
    arguments: &[String],
    policy: CommandPolicy,
    cancellation: &CancellationToken,
    operation: &str,
) -> Result<u32, CommandFailure> {
    if executable.components().count() > 1 && !executable.is_file() {
        return Err(CommandFailure::new(
            CommandFailureKind::Spawn,
            operation,
            None,
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
            return Err(CommandFailure::new(
                CommandFailureKind::Cancelled,
                operation,
                None,
            ));
        }
        return Err(CommandFailure::new(
            CommandFailureKind::Spawn,
            operation,
            Some(std::io::Error::from_raw_os_error(code as i32)),
        ));
    }
    if info.hProcess.is_null() {
        return Err(CommandFailure::new(
            CommandFailureKind::Wait,
            operation,
            None,
        ));
    }

    let handle = ProcessHandle(info.hProcess);
    let process_id = unsafe { GetProcessId(handle.0) };
    let job = WindowsJob::attach(handle.0).ok();
    wait_for_process(
        handle.0,
        process_id,
        job.as_ref(),
        policy,
        cancellation,
        operation,
    )
}

fn wait_for_process(
    handle: windows_sys::Win32::Foundation::HANDLE,
    process_id: u32,
    job: Option<&WindowsJob>,
    policy: CommandPolicy,
    cancellation: &CancellationToken,
    operation: &str,
) -> Result<u32, CommandFailure> {
    let started = Instant::now();
    loop {
        let kind = if cancellation.is_cancelled() {
            Some(CommandFailureKind::Cancelled)
        } else if started.elapsed() >= policy.timeout {
            Some(CommandFailureKind::Timeout)
        } else {
            None
        };
        if let Some(kind) = kind {
            let cleanup = terminate_process_tree(
                handle,
                process_id,
                job,
                policy.termination_grace.max(Duration::from_secs(1)),
            )
            .err();
            return Err(CommandFailure::new(kind, operation, cleanup));
        }

        let remaining = policy.timeout.saturating_sub(started.elapsed());
        let wait_result = unsafe {
            WaitForSingleObject(
                handle,
                duration_milliseconds(remaining.min(Duration::from_millis(250))),
            )
        };
        if wait_result == WAIT_OBJECT_0 {
            break;
        }
        if wait_result != WAIT_TIMEOUT {
            let wait_error = std::io::Error::last_os_error();
            let cleanup = terminate_process_tree(
                handle,
                process_id,
                job,
                policy.termination_grace.max(Duration::from_secs(1)),
            )
            .err()
            .unwrap_or(wait_error);
            return Err(CommandFailure::new(
                CommandFailureKind::Wait,
                operation,
                Some(cleanup),
            ));
        }
    }

    if let Some(job) = job {
        loop {
            let active = job.active_processes().map_err(|error| {
                let cleanup = terminate_process_tree(
                    handle,
                    process_id,
                    Some(job),
                    policy.termination_grace.max(Duration::from_secs(1)),
                )
                .err()
                .unwrap_or(error);
                CommandFailure::new(CommandFailureKind::Wait, operation, Some(cleanup))
            })?;
            if active == 0 {
                break;
            }
            let kind = if cancellation.is_cancelled() {
                Some(CommandFailureKind::Cancelled)
            } else if started.elapsed() >= policy.timeout {
                Some(CommandFailureKind::Timeout)
            } else {
                None
            };
            if let Some(kind) = kind {
                let cleanup = terminate_process_tree(
                    handle,
                    process_id,
                    Some(job),
                    policy.termination_grace.max(Duration::from_secs(1)),
                )
                .err();
                return Err(CommandFailure::new(kind, operation, cleanup));
            }
            std::thread::sleep(
                policy
                    .timeout
                    .saturating_sub(started.elapsed())
                    .min(Duration::from_millis(50)),
            );
        }
    } else {
        terminate_descendants(
            process_id,
            policy.termination_grace.max(Duration::from_secs(1)),
        );
    }

    let mut exit_code = 0_u32;
    let read = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    if read == 0 {
        return Err(CommandFailure::new(
            CommandFailureKind::Wait,
            operation,
            Some(std::io::Error::last_os_error()),
        ));
    }
    Ok(exit_code)
}

struct ProcessHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn duration_milliseconds(duration: Duration) -> u32 {
    duration.as_millis().clamp(1, u32::MAX as u128) as u32
}

fn terminate_process_tree(
    root_handle: windows_sys::Win32::Foundation::HANDLE,
    root_pid: u32,
    job: Option<&WindowsJob>,
    wait: Duration,
) -> std::io::Result<()> {
    if let Some(job) = job {
        job.terminate();
    } else {
        if unsafe { TerminateProcess(root_handle, 1) } == 0 {
            let error = std::io::Error::last_os_error();
            if unsafe { WaitForSingleObject(root_handle, 0) } != WAIT_OBJECT_0 {
                return Err(error);
            }
        }
        terminate_descendants(root_pid, wait);
    }
    let result = unsafe { WaitForSingleObject(root_handle, duration_milliseconds(wait)) };
    if result == WAIT_OBJECT_0 {
        if let Some(job) = job {
            wait_for_empty_job(job, wait)
        } else {
            Ok(())
        }
    } else if result == WAIT_TIMEOUT {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the elevated process did not terminate before the cleanup deadline",
        ))
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn wait_for_empty_job(job: &WindowsJob, wait: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + wait;
    loop {
        if job.active_processes()? == 0 {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the elevated process tree did not terminate before the cleanup deadline",
            ));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn terminate_descendants(root_pid: u32, wait: Duration) {
    use std::collections::{BTreeMap, BTreeSet};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    let snapshot = ProcessHandle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut parents = BTreeMap::new();
    let mut available = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while available {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        available = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }

    let mut descendants = BTreeSet::from([root_pid]);
    loop {
        let before = descendants.len();
        for (&pid, &parent) in &parents {
            if descendants.contains(&parent) {
                descendants.insert(pid);
            }
        }
        if descendants.len() == before {
            break;
        }
    }
    descendants.remove(&root_pid);
    let handles = descendants
        .into_iter()
        .filter_map(|pid| {
            let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
            (!handle.is_null()).then_some(ProcessHandle(handle))
        })
        .collect::<Vec<_>>();
    for handle in &handles {
        unsafe {
            TerminateProcess(handle.0, 1);
        }
    }
    let deadline = Instant::now() + wait;
    for handle in &handles {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        unsafe {
            WaitForSingleObject(handle.0, duration_milliseconds(remaining));
        }
    }
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
    let mut command = hidden_command("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .env("MENDIMARU_PROCESS_PATH", executable)
        .stdin(Stdio::null());
    let output = crate::process::output_sync(
        command,
        CommandPolicy::STATUS,
        None,
        "Studio Pro process inspection",
    )
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
    use super::{
        hidden_command, quote_windows_argument, run_elevated, system_executable, wait_for_process,
    };
    use crate::process::{CancellationToken, CommandFailureKind, CommandPolicy, WindowsJob};
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Stdio};
    use std::time::Duration;

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
        let failure = run_elevated(
            &missing,
            &[],
            CommandPolicy::new(Duration::from_secs(1), 0),
            &CancellationToken::default(),
            "missing installer fixture",
        )
        .expect_err("missing elevated executable is rejected");
        assert_eq!(failure.kind(), CommandFailureKind::Spawn);
    }

    #[test]
    fn fake_elevated_process_has_bounded_timeout_and_cleanup() {
        let mut child = fixture_process("while ($true) { Start-Sleep -Milliseconds 100 }");
        let handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        let job = WindowsJob::attach(handle).ok();
        let failure = wait_for_process(
            handle,
            child.id(),
            job.as_ref(),
            CommandPolicy::new(Duration::from_millis(100), 0),
            &CancellationToken::default(),
            "fake elevated timeout",
        )
        .expect_err("fake elevated process times out");

        assert_eq!(failure.kind(), CommandFailureKind::Timeout);
        if let Some(job) = &job {
            assert_eq!(job.active_processes().expect("query timeout job"), 0);
        }
        child.wait().expect("timed out fixture is reaped");
    }

    #[test]
    fn fake_elevated_process_observes_user_cancellation() {
        let mut child = fixture_process("while ($true) { Start-Sleep -Milliseconds 100 }");
        let handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        let job = WindowsJob::attach(handle).ok();
        let cancellation = CancellationToken::default();
        let trigger = cancellation.clone();
        let cancellation_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });
        let failure = wait_for_process(
            handle,
            child.id(),
            job.as_ref(),
            CommandPolicy::new(Duration::from_secs(5), 0),
            &cancellation,
            "fake elevated cancellation",
        )
        .expect_err("fake elevated process is cancelled");

        cancellation_thread
            .join()
            .expect("cancellation thread joins");
        assert_eq!(failure.kind(), CommandFailureKind::Cancelled);
        if let Some(job) = &job {
            assert_eq!(job.active_processes().expect("query cancelled job"), 0);
        }
        child.wait().expect("cancelled fixture is reaped");
    }

    #[test]
    fn fake_elevated_process_preserves_success_and_reboot_exit_codes() {
        for exit_code in [0_u32, 1641, 3010] {
            let mut child = fixture_process(&format!("exit {exit_code}"));
            let handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
            let job = WindowsJob::attach(handle).ok();
            let actual = wait_for_process(
                handle,
                child.id(),
                job.as_ref(),
                CommandPolicy::new(Duration::from_secs(5), 0),
                &CancellationToken::default(),
                "fake elevated success",
            )
            .expect("fake elevated process exits");

            assert_eq!(actual, exit_code);
            if let Some(job) = &job {
                assert_eq!(job.active_processes().expect("query successful job"), 0);
            }
            child.wait().expect("successful fixture is reaped");
        }
    }

    fn fixture_process(script: &str) -> Child {
        let mut command = hidden_command("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start fake elevated process")
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
