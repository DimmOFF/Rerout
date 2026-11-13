//! Запуск процесса в приостановленном состоянии и внедрение hook-DLL.
//!
//! Внедрение работает только между процессами одной разрядности
//! (адрес LoadLibraryW берётся из собственного kernel32). Для другой
//! разрядности `inject` запускает helper `rerout-injector-{x86|x64}.exe`.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CreateRemoteThread, GetExitCodeThread, IsWow64Process, OpenProcess,
    ResumeThread, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    PROCESS_CREATE_THREAD, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    STARTUPINFOW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// true — процесс 32-битный (WOW64). Предполагается 64-битная Windows.
pub fn is_32bit_process(pid: u32) -> Option<bool> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut wow: i32 = 0;
        let ok = IsWow64Process(h, &mut wow);
        CloseHandle(h);
        if ok == 0 {
            None
        } else {
            Some(wow != 0)
        }
    }
}

/// Внедрение через CreateRemoteThread(LoadLibraryW). Та же разрядность.
pub fn inject_same_arch(pid: u32, dll: &Path) -> Result<(), String> {
    let wpath: Vec<u16> = dll
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let bytes = wpath.len() * 2;

    unsafe {
        let access = PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_READ;
        let proc: HANDLE = OpenProcess(access, 0, pid);
        if proc.is_null() {
            return Err(format!("OpenProcess({pid}) failed: error {}", GetLastError()));
        }

        let result = (|| -> Result<(), String> {
            let remote = VirtualAllocEx(proc, null(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            if remote.is_null() {
                return Err(format!("VirtualAllocEx failed: error {}", GetLastError()));
            }

            let finish = |r: Result<(), String>| {
                VirtualFreeEx(proc, remote, 0, MEM_RELEASE);
                r
            };

            if WriteProcessMemory(proc, remote, wpath.as_ptr() as *const c_void, bytes, null_mut()) == 0 {
                return finish(Err(format!("WriteProcessMemory failed: error {}", GetLastError())));
            }

            let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
            let Some(load) = GetProcAddress(k32, b"LoadLibraryW\0".as_ptr()) else {
                return finish(Err("LoadLibraryW not found".into()));
            };
            let start: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(load);

            let th = CreateRemoteThread(proc, null(), 0, Some(start), remote, 0, null_mut());
            if th.is_null() {
                return finish(Err(format!("CreateRemoteThread failed: error {}", GetLastError())));
            }

            let wait = WaitForSingleObject(th, 15_000);
            let mut code: u32 = 0;
            GetExitCodeThread(th, &mut code);
            CloseHandle(th);

            if wait != 0 {
                // Таймаут: память не освобождаем, поток ещё может её читать.
                return Err("timed out while loading the DLL".into());
            }
            if code == 0 {
                return finish(Err("LoadLibraryW returned NULL (the DLL failed to load)".into()));
            }
            finish(Ok(()))
        })();

        CloseHandle(proc);
        result
    }
}

/// Внедрить hook-DLL нужной разрядности. `dist` — папка с exe/dll.
pub fn inject(pid: u32, dist: &Path) -> Result<(), String> {
    let target32 = is_32bit_process(pid).ok_or("could not open the process")?;
    let arch = if target32 { "x86" } else { "x64" };

    let dll = dist.join(format!("rerout_hook_{arch}.dll"));
    if !dll.exists() {
        return Err(format!("file not found: {}", dll.display()));
    }

    let own32 = cfg!(target_pointer_width = "32");
    if own32 == target32 {
        return inject_same_arch(pid, &dll);
    }

    let helper = dist.join(format!("rerout-injector-{arch}.exe"));
    let out = Command::new(&helper)
        .arg(pid.to_string())
        .arg(&dll)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run {}: {e}", helper.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Процесс, созданный приостановленным.
pub struct Suspended {
    pub pid: u32,
    process: HANDLE,
    thread: HANDLE,
}

impl Suspended {
    pub fn resume(self) {
        unsafe {
            ResumeThread(self.thread);
            CloseHandle(self.thread);
            CloseHandle(self.process);
        }
    }

    pub fn kill(self) {
        unsafe {
            TerminateProcess(self.process, 1);
            CloseHandle(self.thread);
            CloseHandle(self.process);
        }
    }
}

pub fn spawn_suspended(exe: &Path) -> Result<Suspended, String> {
    let mut cmd = wide(&format!("\"{}\"", exe.display()));
    let cwd = exe.parent().map(|p| wide(&p.display().to_string()));

    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        let ok = CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED,
            null(),
            cwd.as_ref().map_or(null(), |c| c.as_ptr()),
            &si,
            &mut pi,
        );
        if ok == 0 {
            let err = GetLastError();
            return Err(if err == 740 {
                "this app needs administrator rights: run Rerout as administrator".into()
            } else {
                format!("CreateProcessW failed: error {err}")
            });
        }
        Ok(Suspended {
            pid: pi.dwProcessId,
            process: pi.hProcess,
            thread: pi.hThread,
        })
    }
}
