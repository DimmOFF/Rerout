//! Автозапуск при входе в Windows: HKCU\Software\Microsoft\Windows\CurrentVersion\Run

use std::ptr::{null, null_mut};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Rerout";
const LEGACY_VALUE: &str = "FBind"; // имя до переименования

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn open(access: u32) -> Option<HKEY> {
    let mut key: HKEY = null_mut();
    let r = RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, access, &mut key);
    if r == 0 {
        Some(key)
    } else {
        None
    }
}

pub fn is_enabled() -> bool {
    unsafe {
        let Some(k) = open(KEY_QUERY_VALUE) else { return false };
        let r = RegQueryValueExW(k, wide(VALUE).as_ptr(), null(), null_mut(), null_mut(), null_mut());
        RegCloseKey(k);
        r == 0
    }
}

/// Включить/выключить автозапуск. В Run пишется путь к текущему exe + `--minimized`
/// (запуск сразу в трей).
pub fn set(enable: bool) -> bool {
    unsafe {
        let Some(k) = open(KEY_SET_VALUE) else { return false };
        let ok = if enable {
            match std::env::current_exe() {
                Ok(exe) => {
                    let cmd = wide(&format!("\"{}\" --minimized", exe.display()));
                    RegSetValueExW(
                        k,
                        wide(VALUE).as_ptr(),
                        0,
                        REG_SZ,
                        cmd.as_ptr() as *const u8,
                        (cmd.len() * 2) as u32,
                    ) == 0
                }
                Err(_) => false,
            }
        } else {
            let r = RegDeleteValueW(k, wide(VALUE).as_ptr());
            r == 0 || r == 2 // 2 = ERROR_FILE_NOT_FOUND: значения и так нет
        };
        RegCloseKey(k);
        ok
    }
}

/// Старая запись автозапуска (FBind): удалить, а если она была — включить новую.
pub fn migrate_legacy() {
    unsafe {
        let Some(k) = open(KEY_QUERY_VALUE | KEY_SET_VALUE) else { return };
        let existed =
            RegQueryValueExW(k, wide(LEGACY_VALUE).as_ptr(), null(), null_mut(), null_mut(), null_mut()) == 0;
        if existed {
            RegDeleteValueW(k, wide(LEGACY_VALUE).as_ptr());
        }
        RegCloseKey(k);
        if existed {
            set(true);
        }
    }
}
