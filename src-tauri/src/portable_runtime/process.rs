#[cfg(any(windows, target_os = "macos"))]
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcessIdentity {
    pub(super) pid: u32,
    pub(super) start_token: String,
}

pub(super) fn identity(pid: u32) -> Result<ProcessIdentity, String> {
    platform_identity(pid)
}

pub(super) fn matches(expected: &ProcessIdentity) -> bool {
    identity(expected.pid).is_ok_and(|observed| observed.start_token == expected.start_token)
}

#[cfg(unix)]
pub(super) fn configure_detached_supervisor(command: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
pub(super) fn configure_detached_supervisor(command: &mut tokio::process::Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(target_os = "linux")]
pub(super) fn configure_runtime_child(command: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn configure_runtime_child(command: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
pub(super) fn configure_runtime_child(command: &mut tokio::process::Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(unix)]
pub(super) fn terminate_supervisor(expected: &ProcessIdentity) {
    if matches(expected) {
        unsafe {
            libc::kill(expected.pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
pub(super) fn terminate_supervisor(expected: &ProcessIdentity) {
    if !matches(expected) {
        return;
    }
    unsafe {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
        let handle = OpenProcess(PROCESS_TERMINATE, 0, expected.pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
pub(super) fn terminate_runtime_group(expected: &ProcessIdentity, force: bool) {
    if !matches(expected) {
        return;
    }
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    unsafe {
        libc::kill(-(expected.pid as i32), signal);
    }
}

#[cfg(windows)]
pub(super) fn terminate_runtime_group(expected: &ProcessIdentity, _force: bool) {
    if !matches(expected) {
        return;
    }
    let _ = Command::new("taskkill")
        .args(["/PID", &expected.pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(target_os = "linux")]
fn platform_identity(pid: u32) -> Result<ProcessIdentity, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|error| format!("could not read process identity: {error}"))?;
    let closing = stat
        .rfind(')')
        .ok_or_else(|| "the process identity is malformed".to_string())?;
    let remainder = stat
        .get(closing + 2..)
        .ok_or_else(|| "the process identity is malformed".to_string())?;
    let fields = remainder.split_whitespace().collect::<Vec<_>>();
    // /proc stat field 22 is the process start time. The first field in
    // `remainder` is field 3 because pid and comm were removed.
    let start = fields
        .get(19)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "the process start identity is invalid".to_string())?;
    Ok(ProcessIdentity {
        pid,
        start_token: start.to_string(),
    })
}

#[cfg(windows)]
fn platform_identity(pid: u32) -> Result<ProcessIdentity, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        SYNCHRONIZE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return Err(format!(
                "could not open process identity: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut exit_code = 0_u32;
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        let active = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
        let times = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        if !active || !times {
            return Err("the process is not active".to_string());
        }
        let token = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        if token == 0 {
            return Err("the process start identity is invalid".to_string());
        }
        Ok(ProcessIdentity {
            pid,
            start_token: token.to_string(),
        })
    }
}

#[cfg(target_os = "macos")]
fn platform_identity(pid: u32) -> Result<ProcessIdentity, String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .map_err(|error| format!("could not inspect process identity: {error}"))?;
    if !output.status.success() {
        return Err("the process is not active".to_string());
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err("the process start identity is invalid".to_string());
    }
    Ok(ProcessIdentity {
        pid,
        start_token: token,
    })
}

#[cfg(windows)]
pub(super) struct RuntimeContainment {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl RuntimeContainment {
    pub(super) fn attach(pid: u32) -> Result<Self, String> {
        use std::mem::size_of;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!(
                    "could not create the runtime job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(format!(
                    "could not configure the runtime job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let process = OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if process.is_null() || AssignProcessToJobObject(job, process) == 0 {
                if !process.is_null() {
                    windows_sys::Win32::Foundation::CloseHandle(process);
                }
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(format!(
                    "could not contain the runtime process: {}",
                    std::io::Error::last_os_error()
                ));
            }
            windows_sys::Win32::Foundation::CloseHandle(process);
            Ok(Self { handle: job })
        }
    }
}

#[cfg(windows)]
impl Drop for RuntimeContainment {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(unix)]
pub(super) struct RuntimeContainment {
    process_group: i32,
}

#[cfg(unix)]
impl RuntimeContainment {
    pub(super) fn attach(pid: u32) -> Result<Self, String> {
        let pid =
            i32::try_from(pid).map_err(|_| "the process identifier is invalid".to_string())?;
        let process_group = unsafe { libc::getpgid(pid) };
        if process_group == -1 {
            return Err(format!(
                "could not inspect the process group: {}",
                std::io::Error::last_os_error()
            ));
        }
        if process_group != pid {
            return Err("the child process does not own its process group".to_string());
        }
        Ok(Self { process_group })
    }
}

#[cfg(unix)]
impl Drop for RuntimeContainment {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
    }
}

#[cfg(not(any(windows, unix)))]
pub(super) struct RuntimeContainment;

#[cfg(not(any(windows, unix)))]
impl RuntimeContainment {
    pub(super) fn attach(_pid: u32) -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_binds_pid_to_start_time() {
        let current = identity(std::process::id()).expect("current process identity");
        assert_eq!(current.pid, std::process::id());
        assert!(!current.start_token.is_empty());
        assert!(matches(&current));
        let stale = ProcessIdentity {
            pid: current.pid,
            start_token: "1".to_string(),
        };
        assert!(!matches(&stale));
    }
}
