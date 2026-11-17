//! Hook-DLL injected into the target process.
//!
//! The policy (which interface to use, loopback allowed or not, rule on/off) is
//! re-read from the config every ~1.5 s, so changes made in the Rerout window
//! apply to already running applications.
//!
//! Hooks: connect / WSAConnect / ConnectEx / sendto / WSASendTo (right before a
//! connect or send the socket gets IP_UNICAST_IF / IPV6_UNICAST_IF, or the call
//! fails when no allowed interface is available), plus CreateProcessW so that
//! child processes get the same rule. Loopback destinations are never bound to
//! an interface (that would break the app's own local servers); they are only
//! allowed or blocked. Listening sockets are not touched at all.
//! ConnectEx is not exported: WSAIoctl hands out its address, so WSAIoctl is
//! hooked and the returned pointer is replaced.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::{Relaxed, SeqCst}};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use retour::GenericDetour;
use windows_sys::Win32::Networking::WinSock::{setsockopt, WSASetLastError};
use windows_sys::Win32::System::LibraryLoader::{
    DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW, GetProcAddress,
};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE};
use windows_sys::Win32::System::Threading::{ResumeThread, Sleep, CREATE_SUSPENDED, PROCESS_INFORMATION};

type ConnectFn = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
type WsaConnectFn =
    unsafe extern "system" fn(usize, *const u8, i32, *const c_void, *mut c_void, *const c_void, *const c_void) -> i32;
type SendToFn = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
type WsaSendToFn = unsafe extern "system" fn(
    usize,
    *const c_void,
    u32,
    *mut u32,
    u32,
    *const u8,
    i32,
    *mut c_void,
    *mut c_void,
) -> i32;
type WsaIoctlFn = unsafe extern "system" fn(
    usize,
    u32,
    *mut c_void,
    u32,
    *mut c_void,
    u32,
    *mut u32,
    *mut c_void,
    *mut c_void,
) -> i32;
type ConnectExFn =
    unsafe extern "system" fn(usize, *const u8, i32, *const c_void, u32, *mut u32, *mut c_void) -> i32;
type CreateProcessWFn = unsafe extern "system" fn(
    *const u16,
    *mut u16,
    *const c_void,
    *const c_void,
    i32,
    u32,
    *const c_void,
    *const u16,
    *const c_void,
    *mut PROCESS_INFORMATION,
) -> i32;

/// Normalized exe path of the rule this process follows
static RULE_KEY: OnceLock<String> = OnceLock::new();
static POLICY: Mutex<Option<(Instant, Policy)>> = Mutex::new(None);
static DIST: OnceLock<PathBuf> = OnceLock::new();
/// Original ConnectEx (handed out by WSAIoctl)
static CONNECTEX_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Number of threads that are currently executing code of this DLL (hook functions).
/// `rerout_unhook` waits until it is zero before the DLL may be unloaded.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct Busy;

impl Busy {
    fn enter() -> Busy {
        ACTIVE.fetch_add(1, SeqCst);
        Busy
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, SeqCst);
    }
}

/// Page with a tiny jump thunk for ConnectEx: [0..8) = target pointer, [8..) = `jmp [target]`.
/// Applications (libuv, Chromium) cache the ConnectEx pointer they get from WSAIoctl. That pointer
/// must stay valid after this DLL is unloaded, so the app gets the address of the thunk (which lives
/// outside the DLL), and on release the thunk is redirected to the real ConnectEx.
static THUNK: OnceLock<usize> = OnceLock::new();

static CONNECT_HOOK: OnceLock<GenericDetour<ConnectFn>> = OnceLock::new();
static WSACONNECT_HOOK: OnceLock<GenericDetour<WsaConnectFn>> = OnceLock::new();
static SENDTO_HOOK: OnceLock<GenericDetour<SendToFn>> = OnceLock::new();
static WSASENDTO_HOOK: OnceLock<GenericDetour<WsaSendToFn>> = OnceLock::new();
static WSAIOCTL_HOOK: OnceLock<GenericDetour<WsaIoctlFn>> = OnceLock::new();
static CREATEPROC_HOOK: OnceLock<GenericDetour<CreateProcessWFn>> = OnceLock::new();

thread_local! {
    // Re-entrancy guards
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static IN_POLICY: Cell<bool> = const { Cell::new(false) };
}

const AF_INET: i32 = 2;
const AF_INET6: i32 = 23;
const IPPROTO_IP: i32 = 0;
const IPPROTO_IPV6: i32 = 41;
const IP_UNICAST_IF: i32 = 31;
const IPV6_UNICAST_IF: i32 = 31;
const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC800_0006;
/// WSAID_CONNECTEX {25a207b9-ddf3-4660-8ee9-76e58c74063e} as laid out in memory
const WSAID_CONNECTEX: [u8; 16] = [
    0xb9, 0x07, 0xa2, 0x25, 0xf3, 0xdd, 0x60, 0x46, 0x8e, 0xe9, 0x76, 0xe5, 0x8c, 0x74, 0x06, 0x3e,
];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// rule off / not configured: do nothing
    Pass,
    /// force traffic through if4 / if6
    Bind,
    /// no allowed interface is available: fail network calls
    Block,
}

#[derive(Clone, Copy)]
struct Policy {
    mode: Mode,
    if4: u32,
    if6: u32,
    allow_loopback: bool,
}

const PASS: Policy = Policy { mode: Mode::Pass, if4: 0, if6: 0, allow_loopback: true };

fn compute_policy() -> Policy {
    let Some(key) = RULE_KEY.get() else { return PASS };
    let cfg = rerout_common::config::load();
    let Some(rule) = cfg
        .rules
        .iter()
        .find(|r| rerout_common::norm_path(&r.exe_path) == *key)
    else {
        return PASS;
    };
    if !rule.enabled || !rule.configured {
        return PASS;
    }
    let adapters = rerout_common::adapters::list();
    match rerout_common::config::resolve(rule, &adapters) {
        Some(a) => Policy {
            mode: Mode::Bind,
            if4: a.if_index,
            if6: a.if_index6,
            allow_loopback: rule.allow_loopback,
        },
        None => Policy { mode: Mode::Block, if4: 0, if6: 0, allow_loopback: rule.allow_loopback },
    }
}

/// Current policy (cached for 1.5 s).
fn policy() -> Policy {
    if IN_POLICY.with(|c| c.get()) {
        return PASS; // re-entrancy while the config is being read
    }
    let Ok(mut g) = POLICY.lock() else { return PASS };
    if let Some((t, p)) = *g {
        if t.elapsed() < Duration::from_millis(1500) {
            return p;
        }
    }
    IN_POLICY.with(|c| c.set(true));
    let p = compute_policy();
    IN_POLICY.with(|c| c.set(false));
    *g = Some((Instant::now(), p));
    p
}

unsafe fn set4(s: usize, idx: u32) {
    if idx != 0 {
        let v = idx.to_be(); // IPv4: interface index in network byte order
        setsockopt(s, IPPROTO_IP, IP_UNICAST_IF, &v as *const u32 as *const u8, 4);
    }
}

unsafe fn set6(s: usize, idx: u32) {
    if idx != 0 {
        let v = idx;
        setsockopt(s, IPPROTO_IPV6, IPV6_UNICAST_IF, &v as *const u32 as *const u8, 4);
    }
}

fn all_zero(a: &[u8]) -> bool {
    a.iter().all(|&b| b == 0)
}

#[derive(PartialEq)]
enum Kind {
    Loopback,
    V4,
    V6,
    V6Mapped4,
}

/// What kind of destination does this sockaddr hold (None: other family)?
unsafe fn dest_kind(name: *const u8, len: i32) -> Option<Kind> {
    if name.is_null() || len < 8 {
        return None;
    }
    let fam = u16::from_ne_bytes([*name, *name.add(1)]) as i32;
    match fam {
        AF_INET => {
            let a = std::slice::from_raw_parts(name.add(4), 4); // sin_addr
            if a[0] == 127 || all_zero(a) {
                Some(Kind::Loopback)
            } else {
                Some(Kind::V4)
            }
        }
        AF_INET6 if len >= 24 => {
            let a = std::slice::from_raw_parts(name.add(8), 16); // sin6_addr
            let loop6 = all_zero(&a[..15]) && a[15] == 1;
            let mapped = all_zero(&a[..10]) && a[10] == 0xff && a[11] == 0xff;
            if all_zero(a) || loop6 {
                Some(Kind::Loopback)
            } else if mapped {
                if a[12] == 127 || all_zero(&a[12..16]) {
                    Some(Kind::Loopback)
                } else {
                    Some(Kind::V6Mapped4)
                }
            } else {
                Some(Kind::V6)
            }
        }
        _ => None,
    }
}

/// Apply the policy to a socket that is about to connect / send to `name`.
/// Returns false when the call must fail (the Winsock error is already set).
unsafe fn gate(s: usize, name: *const u8, len: i32) -> bool {
    let Some(kind) = dest_kind(name, len) else { return true };
    let p = policy();
    if p.mode == Mode::Pass {
        return true;
    }
    if kind == Kind::Loopback {
        if p.allow_loopback {
            return true;
        }
        WSASetLastError(10061); // WSAECONNREFUSED
        return false;
    }
    if p.mode == Mode::Block {
        WSASetLastError(10051); // WSAENETUNREACH
        return false;
    }
    match kind {
        Kind::V4 => set4(s, p.if4),
        Kind::V6 => set6(s, p.if6),
        Kind::V6Mapped4 => {
            set6(s, p.if6);
            set4(s, p.if4);
        }
        Kind::Loopback => {}
    }
    true
}

unsafe extern "system" fn hk_connect(s: usize, name: *const u8, len: i32) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = CONNECT_HOOK.get() else { return -1 };
    if !gate(s, name, len) {
        return -1;
    }
    h.call(s, name, len)
}

unsafe extern "system" fn hk_wsaconnect(
    s: usize,
    name: *const u8,
    len: i32,
    caller: *const c_void,
    callee: *mut c_void,
    sqos: *const c_void,
    gqos: *const c_void,
) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = WSACONNECT_HOOK.get() else { return -1 };
    if !gate(s, name, len) {
        return -1;
    }
    h.call(s, name, len, caller, callee, sqos, gqos)
}

unsafe extern "system" fn hk_sendto(
    s: usize,
    buf: *const u8,
    len: i32,
    flags: i32,
    to: *const u8,
    tolen: i32,
) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = SENDTO_HOOK.get() else { return -1 };
    if !gate(s, to, tolen) {
        return -1;
    }
    h.call(s, buf, len, flags, to, tolen)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hk_wsasendto(
    s: usize,
    bufs: *const c_void,
    count: u32,
    sent: *mut u32,
    flags: u32,
    to: *const u8,
    tolen: i32,
    ov: *mut c_void,
    cr: *mut c_void,
) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = WSASENDTO_HOOK.get() else { return -1 };
    if !gate(s, to, tolen) {
        return -1;
    }
    h.call(s, bufs, count, sent, flags, to, tolen, ov, cr)
}

/// Replacement for ConnectEx (used by libuv/Node, Chromium and others)
unsafe extern "system" fn hk_connectex(
    s: usize,
    name: *const u8,
    len: i32,
    sendbuf: *const c_void,
    sendlen: u32,
    sent: *mut u32,
    ov: *mut c_void,
) -> i32 {
    let _busy = Busy::enter();
    let orig = CONNECTEX_ORIG.load(Relaxed);
    if orig == 0 {
        return 0; // FALSE
    }
    if !gate(s, name, len) {
        return 0; // FALSE
    }
    let f: ConnectExFn = std::mem::transmute(orig);
    f(s, name, len, sendbuf, sendlen, sent, ov)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hk_wsaioctl(
    s: usize,
    code: u32,
    inb: *mut c_void,
    inl: u32,
    outb: *mut c_void,
    outl: u32,
    ret: *mut u32,
    ov: *mut c_void,
    cr: *mut c_void,
) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = WSAIOCTL_HOOK.get() else { return -1 };
    let r = h.call(s, code, inb, inl, outb, outl, ret, ov, cr);

    if r == 0
        && code == SIO_GET_EXTENSION_FUNCTION_POINTER
        && inl == 16
        && !inb.is_null()
        && !outb.is_null()
        && outl as usize >= std::mem::size_of::<usize>()
    {
        let guid = std::slice::from_raw_parts(inb as *const u8, 16);
        if guid == WSAID_CONNECTEX {
            let orig = std::ptr::read_unaligned(outb as *const usize);
            let ours = connectex_entry();
            if orig != 0 && orig != ours {
                CONNECTEX_ORIG.store(orig, Relaxed);
                std::ptr::write_unaligned(outb as *mut usize, ours);
            }
        }
    }
    r
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hk_create_process(
    app: *const u16,
    cmd: *mut u16,
    pa: *const c_void,
    ta: *const c_void,
    inherit: i32,
    flags: u32,
    env: *const c_void,
    cwd: *const u16,
    si: *const c_void,
    pi: *mut PROCESS_INFORMATION,
) -> i32 {
    let _busy = Busy::enter();
    let Some(h) = CREATEPROC_HOOK.get() else { return 0 };
    if pi.is_null() || IN_HOOK.with(|c| c.get()) {
        return h.call(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi);
    }

    // Create the child suspended, inject the DLL, then resume it
    // (unless the caller asked for CREATE_SUSPENDED itself).
    let want_suspended = flags & CREATE_SUSPENDED != 0;
    let ok = h.call(app, cmd, pa, ta, inherit, flags | CREATE_SUSPENDED, env, cwd, si, pi);
    if ok != 0 {
        if let Some(dist) = DIST.get() {
            IN_HOOK.with(|c| c.set(true));
            let _ = rerout_inject::inject((*pi).dwProcessId, dist);
            IN_HOOK.with(|c| c.set(false));
        }
        if !want_suspended {
            ResumeThread((*pi).hThread);
        }
    }
    ok
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn install_hook<T: retour::Function + Copy>(
    module: *mut c_void,
    name: &[u8], // NUL-terminated
    detour: T,
    slot: &'static OnceLock<GenericDetour<T>>,
) {
    let Some(p) = GetProcAddress(module, name.as_ptr()) else { return };
    let target: T = std::mem::transmute_copy(&p);
    if let Ok(h) = GenericDetour::new(target, detour) {
        // Store first, enable second: the hook must never fire before
        // its trampoline is reachable.
        let _ = slot.set(h);
        if let Some(h) = slot.get() {
            let _ = h.enable();
        }
    }
}

unsafe fn install(hinst: *mut c_void) {
    let mut buf = [0u16; 520];
    let n = GetModuleFileNameW(hinst, buf.as_mut_ptr(), buf.len() as u32) as usize;
    let dll_path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    if let Some(dir) = dll_path.parent() {
        let _ = DIST.set(dir.to_path_buf());
    }

    // Which rule applies: from the environment (inherited from the parent)
    // or by the path of our own exe.
    let key = std::env::var("REROUT_RULE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });
    let Some(key) = key else { return };
    let key = rerout_common::norm_path(&key);

    let cfg = rerout_common::config::load();
    let Some(rule) = cfg
        .rules
        .iter()
        .find(|r| rerout_common::norm_path(&r.exe_path) == key)
    else {
        return; // no rule for this process
    };
    let _ = RULE_KEY.set(key);
    // Child processes inherit the rule through the environment
    std::env::set_var("REROUT_RULE", &rule.exe_path);

    // ws2_32 is already loaded: this DLL imports setsockopt from it
    let ws2 = GetModuleHandleW(wide("ws2_32.dll").as_ptr());
    if !ws2.is_null() {
        install_hook::<ConnectFn>(ws2, b"connect\0", hk_connect, &CONNECT_HOOK);
        install_hook::<WsaConnectFn>(ws2, b"WSAConnect\0", hk_wsaconnect, &WSACONNECT_HOOK);
        install_hook::<SendToFn>(ws2, b"sendto\0", hk_sendto, &SENDTO_HOOK);
        install_hook::<WsaSendToFn>(ws2, b"WSASendTo\0", hk_wsasendto, &WSASENDTO_HOOK);
        install_hook::<WsaIoctlFn>(ws2, b"WSAIoctl\0", hk_wsaioctl, &WSAIOCTL_HOOK);
    }
    let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
    if !k32.is_null() {
        install_hook::<CreateProcessWFn>(k32, b"CreateProcessW\0", hk_create_process, &CREATEPROC_HOOK);
    }
}

/// Address the application gets instead of ConnectEx: the thunk (outside the DLL), or, if no
/// executable memory can be allocated, our own function (then the DLL can never be unloaded).
unsafe fn connectex_entry() -> usize {
    let ours = hk_connectex as *const () as usize;
    if let Some(&base) = THUNK.get() {
        return base + 8;
    }
    let base = VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
    if base == 0 {
        return ours;
    }
    std::ptr::write(base as *mut usize, ours);
    let code = (base + 8) as *mut u8;
    #[cfg(target_pointer_width = "64")]
    {
        // jmp qword ptr [rip - 14]  (the slot is at the start of the page)
        let bytes = [0xFFu8, 0x25, 0xF2, 0xFF, 0xFF, 0xFF];
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), code, bytes.len());
    }
    #[cfg(target_pointer_width = "32")]
    {
        // jmp dword ptr [slot]
        let mut bytes = [0xFFu8, 0x25, 0, 0, 0, 0];
        bytes[2..6].copy_from_slice(&(base as u32).to_le_bytes());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), code, bytes.len());
    }
    let _ = THUNK.set(base); // if another thread was faster, its page is used (ours is just a small leak)
    THUNK.get().map(|b| b + 8).unwrap_or(ours)
}

unsafe fn disable_hook<T: retour::Function>(slot: &OnceLock<GenericDetour<T>>) {
    if let Some(h) = slot.get() {
        let _ = h.disable();
    }
}

/// Called by Rerout in a remote thread before it unloads this DLL (FreeLibrary):
/// restores all hooked functions and waits until no thread is inside this DLL.
/// Returns 1 when the DLL can be unloaded safely, 0 otherwise (the hooks are off in both cases).
#[no_mangle]
pub extern "C" fn rerout_unhook(_arg: *mut c_void) -> u32 {
    unsafe {
        // 1. nobody can enter the hook functions any more
        disable_hook(&CREATEPROC_HOOK);
        disable_hook(&WSAIOCTL_HOOK);
        disable_hook(&CONNECT_HOOK);
        disable_hook(&WSACONNECT_HOOK);
        disable_hook(&SENDTO_HOOK);
        disable_hook(&WSASENDTO_HOOK);

        // 2. ConnectEx: the application may have cached our thunk, point it to the real function
        let orig = CONNECTEX_ORIG.load(SeqCst);
        if orig != 0 {
            match THUNK.get() {
                Some(&base) => std::ptr::write_volatile(base as *mut usize, orig),
                None => return 0, // the application holds a pointer into this DLL: never unload
            }
        }

        // 3. wait for threads that are still inside our code
        Sleep(50);
        for _ in 0..100 {
            if ACTIVE.load(SeqCst) == 0 {
                break;
            }
            Sleep(20);
        }
        Sleep(50);
        if ACTIVE.load(SeqCst) == 0 {
            1
        } else {
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn DllMain(hinst: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(hinst);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| install(hinst)));
        }
    }
    1
}
