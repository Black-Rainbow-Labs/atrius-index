//! Questions about other processes: is one running, stop one, and who this user is.

/// Whether a process with this id is running.
///
/// @param pid - the process id
pub fn is_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: the handle is checked before use and closed after.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            ok && code == STILL_ACTIVE as u32
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 checks that the process exists and sends nothing.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

/// Stops a process. Answers whether it was asked to stop.
///
/// @param pid - the process id
pub fn stop(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        // SAFETY: the handle is checked before use and closed after.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let ok = TerminateProcess(handle, 0) != 0;
            CloseHandle(handle);
            ok
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: SIGTERM to a process id this program wrote down itself.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
    }
}

/// This user's numeric id, for launchctl's `gui/<uid>` domain.
#[cfg(unix)]
pub fn user_id() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// This user's numeric id. Windows has none; it is only used on macOS.
#[cfg(windows)]
pub fn user_id() -> u32 {
    0
}
