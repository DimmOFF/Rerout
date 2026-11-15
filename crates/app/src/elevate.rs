//! Administrator rights: needed to close TCP connections (SetTcpEntry).

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Is this process running elevated (as administrator)?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Start a new elevated instance (UAC prompt). The new instance waits until this one
/// has exited. Returns false when the prompt was cancelled or failed.
pub fn restart_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let args = format!("--wait-pid={}", unsafe { GetCurrentProcessId() });
    unsafe {
        let r = ShellExecuteW(
            null_mut(),
            wide("runas").as_ptr(),
            wide(&exe.to_string_lossy()).as_ptr(),
            wide(&args).as_ptr(),
            null(),
            SW_SHOWNORMAL,
        );
        (r as isize) > 32
    }
}

/// At startup: if we were started by `restart_as_admin`, wait for the old instance to exit,
/// so that the single-instance lock is free.
pub fn wait_for_parent() {
    let Some(pid) = std::env::args().find_map(|a| {
        a.strip_prefix("--wait-pid=")
            .and_then(|v| v.parse::<u32>().ok())
    }) else {
        return;
    };
    unsafe {
        let h = OpenProcess(0x0010_0000, 0, pid); // SYNCHRONIZE
        if !h.is_null() {
            WaitForSingleObject(h, 15_000);
            CloseHandle(h);
        }
    }
}
