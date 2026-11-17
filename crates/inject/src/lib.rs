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

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW, MODULEENTRY32W,
    PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CreateRemoteThread, GetCurrentProcessId, GetExitCodeThread, IsWow64Process, OpenProcess,
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

// ----------------------------------------------------------------------------------------
// Releasing the hook DLL from processes ("eject")
// ----------------------------------------------------------------------------------------

/// Result of releasing the hook DLL from one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eject {
    /// the process does not have the DLL
    NotLoaded,
    /// hooks removed and DLL unloaded
    Released,
    /// hooks are off, but the DLL could not be unloaded safely right now
    Busy,
    /// the loaded DLL is an older version without the unhook entry point
    Unsupported,
}

/// Base address of Rerout's hook DLL (any version / bitness) inside `pid`, if it is loaded.
fn hook_module(pid: u32) -> Option<usize> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: MODULEENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut found = None;
        let mut ok = Module32FirstW(snap, &mut e);
        while ok != 0 {
            let n = e.szModule.iter().position(|&c| c == 0).unwrap_or(e.szModule.len());
            let name = String::from_utf16_lossy(&e.szModule[..n]).to_lowercase();
            if name.starts_with("rerout_hook_") && name.ends_with(".dll") {
                found = Some(e.modBaseAddr as usize);
                break;
            }
            ok = Module32NextW(snap, &mut e);
        }
        CloseHandle(snap);
        found
    }
}

unsafe fn read_mem(proc: HANDLE, addr: usize, buf: &mut [u8]) -> bool {
    let mut got = 0usize;
    ReadProcessMemory(proc, addr as *const c_void, buf.as_mut_ptr() as *mut c_void, buf.len(), &mut got) != 0
        && got == buf.len()
}

fn le32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// Address of an exported function of the module at `base` in the remote process. The export table
/// is read from the *loaded image* (the file on disk may be a different build by now).
unsafe fn remote_export(proc: HANDLE, base: usize, name: &str) -> Option<usize> {
    let mut hdr = [0u8; 0x400];
    if !read_mem(proc, base, &mut hdr) || &hdr[..2] != b"MZ" {
        return None;
    }
    let nt = le32(&hdr, 0x3c);
    if nt + 0x90 > hdr.len() || &hdr[nt..nt + 4] != b"PE\0\0" {
        return None;
    }
    let magic = u16::from_le_bytes([hdr[nt + 24], hdr[nt + 25]]);
    let dir = match magic {
        0x20b => nt + 24 + 112, // PE32+
        0x10b => nt + 24 + 96,  // PE32
        _ => return None,
    };
    let export_rva = le32(&hdr, dir);
    if export_rva == 0 {
        return None;
    }
    let mut ed = [0u8; 40];
    if !read_mem(proc, base + export_rva, &mut ed) {
        return None;
    }
    let (names_n, funcs_rva, names_rva, ords_rva) = (le32(&ed, 24), le32(&ed, 28), le32(&ed, 32), le32(&ed, 36));
    if names_n == 0 || names_n > 4096 {
        return None;
    }
    let mut names = vec![0u8; names_n * 4];
    let mut ords = vec![0u8; names_n * 2];
    if !read_mem(proc, base + names_rva, &mut names) || !read_mem(proc, base + ords_rva, &mut ords) {
        return None;
    }
    for i in 0..names_n {
        let mut s = vec![0u8; name.len() + 1];
        if !read_mem(proc, base + le32(&names, i * 4), &mut s) {
            continue;
        }
        if &s[..name.len()] == name.as_bytes() && s[name.len()] == 0 {
            let ord = u16::from_le_bytes([ords[i * 2], ords[i * 2 + 1]]) as usize;
            let mut f = [0u8; 4];
            if !read_mem(proc, base + funcs_rva + ord * 4, &mut f) {
                return None;
            }
            return Some(base + le32(&f, 0));
        }
    }
    None
}

/// Run `start(param)` in a new thread of the remote process and return its exit code.
unsafe fn run_thread(proc: HANDLE, start: usize, param: usize, timeout_ms: u32) -> Result<u32, String> {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(start);
    let th = CreateRemoteThread(proc, null(), 0, Some(f), param as *const c_void, 0, null_mut());
    if th.is_null() {
        return Err(format!("CreateRemoteThread failed: error {}", GetLastError()));
    }
    let wait = WaitForSingleObject(th, timeout_ms);
    let mut code = 0u32;
    GetExitCodeThread(th, &mut code);
    CloseHandle(th);
    if wait != 0 {
        return Err("timed out".into());
    }
    Ok(code)
}

/// Release the hook DLL from a process of the same bitness as this one.
pub fn eject_same_arch(pid: u32) -> Result<Eject, String> {
    let Some(base) = hook_module(pid) else { return Ok(Eject::NotLoaded) };
    unsafe {
        let access = PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_READ;
        let proc = OpenProcess(access, 0, pid);
        if proc.is_null() {
            return Err(format!("OpenProcess({pid}) failed: error {}", GetLastError()));
        }
        let result = (|| -> Result<Eject, String> {
            let Some(entry) = remote_export(proc, base, "rerout_unhook") else {
                return Ok(Eject::Unsupported);
            };
            // 1. the DLL restores the hooked functions and waits for threads inside its own code
            if run_thread(proc, entry, 0, 10_000)? != 1 {
                return Ok(Eject::Busy);
            }
            // 2. FreeLibrary until the module is gone (it may have been loaded more than once)
            let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
            let Some(free) = GetProcAddress(k32, b"FreeLibrary\0".as_ptr()) else {
                return Err("FreeLibrary not found".into());
            };
            for _ in 0..4 {
                run_thread(proc, free as usize, base, 5_000)?;
                if hook_module(pid).is_none() {
                    return Ok(Eject::Released);
                }
            }
            Ok(Eject::Busy)
        })();
        CloseHandle(proc);
        result
    }
}

/// Release the hook DLL from `pid` (any bitness; the other one goes through the helper exe).
pub fn eject(pid: u32, dist: &Path) -> Result<Eject, String> {
    if hook_module(pid).is_none() {
        return Ok(Eject::NotLoaded); // also for protected processes we cannot look into
    }
    let target32 = is_32bit_process(pid).ok_or("could not open the process")?;
    if cfg!(target_pointer_width = "32") == target32 {
        return eject_same_arch(pid);
    }
    let arch = if target32 { "x86" } else { "x64" };
    let helper = dist.join(format!("rerout-injector-{arch}.exe"));
    let status = Command::new(&helper)
        .arg("eject")
        .arg(pid.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("could not run {}: {e}", helper.display()))?;
    match status.code() {
        Some(0) => Ok(Eject::Released),
        Some(10) => Ok(Eject::NotLoaded),
        Some(11) => Ok(Eject::Busy),
        Some(12) => Ok(Eject::Unsupported),
        other => Err(format!("helper failed (exit code {other:?})")),
    }
}

/// Outcome of `release_all`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Report {
    pub released: usize,
    pub busy: usize,
    pub unsupported: usize,
    pub errors: usize,
}

fn all_pids() -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            out.push(e.th32ProcessID);
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

/// Release the hook DLL from every process that has it loaded.
pub fn release_all(dist: &Path) -> Report {
    let me = unsafe { GetCurrentProcessId() };
    let mut rep = Report::default();
    for pid in all_pids() {
        if pid == 0 || pid == 4 || pid == me {
            continue;
        }
        match eject(pid, dist) {
            Ok(Eject::Released) => rep.released += 1,
            Ok(Eject::Busy) => rep.busy += 1,
            Ok(Eject::Unsupported) => rep.unsupported += 1,
            Ok(Eject::NotLoaded) => {}
            Err(_) => rep.errors += 1,
        }
    }
    rep
}
