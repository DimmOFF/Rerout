//! Фоновое слежение: внедряет hook-DLL в процессы, которые соответствуют
//! включённым правилам (в т.ч. запущенные не через Rerout).
//! Процесс, стартовавший раньше внедрения, может успеть открыть сокеты
//! без привязки — для гарантии используйте кнопку «Запустить».

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rerout_common::config::Rule;
use rerout_common::norm_path;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// (pid, parent pid, имя файла)
pub fn processes() -> Vec<(u32, u32, String)> {
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
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push((
                e.th32ProcessID,
                e.th32ParentProcessID,
                String::from_utf16_lossy(&e.szExeFile[..n]),
            ));
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

pub fn full_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }
}

/// Set while Rerout is shutting down and releasing its DLL: the watcher must not inject again.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// Messages from the background watcher to the UI.
pub enum Event {
    /// text for the status bar
    Info(String),
    /// a process of this app (normalized exe path = `key`) could not load the DLL:
    /// typically a sandboxed Chromium subprocess. Reported separately so that a
    /// browser with many such processes does not flood the status bar.
    Sandboxed { key: String, name: String },
}

pub fn spawn(
    rules: Arc<Mutex<Vec<Rule>>>,
    seen: Arc<Mutex<HashSet<u32>>>,
    dist: PathBuf,
    tx: Sender<Event>,
    ctx: eframe::egui::Context,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        if STOP.load(Ordering::Relaxed) {
            continue;
        }

        // (имя файла в нижнем регистре, нормализованный путь)
        let active: Vec<(String, String)> = rules
            .lock()
            .map(|r| {
                r.iter()
                    .filter(|r| r.enabled && r.configured)
                    .map(|r| {
                        let p = norm_path(&r.exe_path);
                        let name = p.rsplit('\\').next().unwrap_or("").to_string();
                        (name, p)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if active.is_empty() {
            continue;
        }

        let procs = processes();
        let alive: HashSet<u32> = procs.iter().map(|(p, _, _)| *p).collect();

        for (pid, _, name) in &procs {
            let lname = name.to_lowercase();
            let Some((_, path)) = active.iter().find(|(n, _)| *n == lname) else { continue };
            {
                let mut s = seen.lock().unwrap();
                if !s.insert(*pid) {
                    continue; // уже обработан
                }
            }
            if full_path(*pid).map(|p| norm_path(&p)).as_deref() != Some(path.as_str()) {
                continue;
            }
            let event = match rerout_inject::inject(*pid, &dist) {
                Ok(()) => Event::Info(format!("Attached to {name} (PID {pid})")),
                Err(e) if e.contains("failed to load") => Event::Sandboxed {
                    key: path.clone(),
                    name: name.clone(),
                },
                Err(e) => Event::Info(format!("Could not attach to {name} (PID {pid}): {e}")),
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        }

        seen.lock().unwrap().retain(|p| alive.contains(p));
    });
}

/// Все процессы с данным exe (нормализованный путь) и их потомки: pid -> имя.
pub fn process_tree(exe_norm: &str) -> std::collections::HashMap<u32, String> {
    let procs = processes();
    let file = exe_norm.rsplit('\\').next().unwrap_or("").to_string();
    let mut set = std::collections::HashMap::new();

    for (pid, _, name) in &procs {
        if name.to_lowercase() == file
            && full_path(*pid).map(|p| norm_path(&p)).as_deref() == Some(exe_norm)
        {
            set.insert(*pid, name.clone());
        }
    }
    loop {
        let before = set.len();
        for (pid, ppid, name) in &procs {
            if !set.contains_key(pid) && set.contains_key(ppid) {
                set.insert(*pid, name.clone());
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}

/// For every rule (normalized exe paths in `keys`): which processes belong to it
/// (the exe itself and all its descendants). Result: pid -> index into `keys`.
/// One process snapshot for all rules; `paths` caches pid -> exe path between calls.
pub fn process_trees(
    keys: &[String],
    paths: &mut std::collections::HashMap<u32, Option<String>>,
) -> std::collections::HashMap<u32, usize> {
    let procs = processes();
    let alive: HashSet<u32> = procs.iter().map(|(p, _, _)| *p).collect();
    paths.retain(|p, _| alive.contains(p));

    let names: Vec<String> = keys
        .iter()
        .map(|k| k.rsplit('\\').next().unwrap_or("").to_string())
        .collect();

    let mut owner: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for (pid, _, name) in &procs {
        let lname = name.to_lowercase();
        for (i, n) in names.iter().enumerate() {
            if *n != lname {
                continue;
            }
            let path = paths.entry(*pid).or_insert_with(|| full_path(*pid)).clone();
            if path.map(|p| norm_path(&p)).as_deref() == Some(keys[i].as_str()) {
                owner.insert(*pid, i);
                break;
            }
        }
    }
    // descendants belong to the same rule
    loop {
        let before = owner.len();
        for (pid, ppid, _) in &procs {
            if owner.contains_key(pid) {
                continue;
            }
            let parent_owner = owner.get(ppid).copied();
            if let Some(i) = parent_owner {
                owner.insert(*pid, i);
            }
        }
        if owner.len() == before {
            break;
        }
    }
    owner
}
