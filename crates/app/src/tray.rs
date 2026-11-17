//! Иконка в трее, меню, скрытие/показ главного окна, единственный экземпляр.
//!
//! Показ и скрытие окна делаются напрямую через Win32 (ShowWindow), а не через
//! egui: пока окно скрыто, egui может не вызывать update(), и команды
//! не обрабатывались бы.

use std::cell::RefCell;
use std::ptr::{null, null_mut};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, MessageBoxW, SetForegroundWindow,
    ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
};

pub const WINDOW_TITLE: &str = "Rerout";

struct TrayState {
    _icon: TrayIcon,
    auto: CheckMenuItem,
}

thread_local! {
    // TrayIcon и пункты меню не Send: живут только в главном потоке
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// true — это первый экземпляр. Иначе приложение уже запущено (возможно, в трее).
pub fn acquire_single_instance() -> bool {
    unsafe {
        let h = CreateMutexW(null(), 1, wide("Local\\Rerout.SingleInstance").as_ptr());
        let already = GetLastError() == 183; // ERROR_ALREADY_EXISTS
        // хэндл намеренно не закрываем: мьютекс живёт до конца процесса
        !(h.is_null() || already)
    }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == WINDOW_TITLE {
        // Заголовок "Rerout" может быть и у папки в Проводнике — сверяем exe процесса
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mine = std::env::current_exe()
            .ok()
            .map(|p| rerout_common::norm_path(&p.to_string_lossy()));
        let theirs = crate::watcher::full_path(pid).map(|p| rerout_common::norm_path(&p));
        if mine.is_some() && mine == theirs {
            *(lparam as *mut HWND) = hwnd;
            return 0; // нашли — остановить перебор
        }
    }
    1
}

fn main_hwnd() -> HWND {
    let mut found: HWND = null_mut();
    unsafe {
        EnumWindows(Some(enum_cb), &mut found as *mut HWND as LPARAM);
    }
    found
}

pub fn show_main_window() {
    unsafe {
        let h = main_hwnd();
        if h.is_null() {
            return;
        }
        ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(h);
    }
}

pub fn hide_main_window() {
    unsafe {
        let h = main_hwnd();
        if !h.is_null() {
            ShowWindow(h, SW_HIDE);
        }
    }
}

fn message_box(text: &str, style: u32) -> i32 {
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide("Rerout").as_ptr(), style) }
}

/// "Quit and release hooks": switch the hooks off in every application, unload Rerout's DLL
/// from them (so the files are free again), then exit.
fn release_and_quit() {
    const MB_YESNO: u32 = 0x4;
    const MB_ICONQUESTION: u32 = 0x20;
    const MB_ICONINFORMATION: u32 = 0x40;
    const MB_TOPMOST: u32 = 0x40000;
    const IDYES: i32 = 6;

    let ask = "Stop controlling all applications and unload Rerout's DLL from them?\n\n\
               Their traffic will no longer follow your rules. Applications that cannot be released \
               safely keep an inactive copy of the DLL until they are restarted.\n\nContinue?";
    if message_box(ask, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) != IDYES {
        return;
    }

    // the watcher must not inject again while we release
    crate::watcher::STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    let dist = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    // two passes: processes started by a still-hooked parent in the meantime are caught by the second one
    let mut rep = rerout_inject::release_all(&dist);
    let second = rerout_inject::release_all(&dist);
    rep.released += second.released;
    rep.busy = second.busy;
    rep.unsupported = second.unsupported;
    rep.errors = second.errors;

    let mut lines = vec![format!("Released the hook from {} process(es).", rep.released)];
    if rep.busy > 0 {
        lines.push(format!(
            "{} process(es) could not be released right now (busy); the hooks in them are switched off.",
            rep.busy
        ));
    }
    if rep.unsupported > 0 {
        lines.push(format!(
            "{} process(es) run an older hook version: restart them to release it.",
            rep.unsupported
        ));
    }
    if rep.errors > 0 {
        lines.push(format!(
            "{} process(es) could not be accessed (run Rerout as administrator?).",
            rep.errors
        ));
    }
    message_box(&lines.join("\n"), MB_ICONINFORMATION | MB_TOPMOST);
    quit();
}

pub fn quit() -> ! {
    // Drop TrayIcon убирает значок из трея (иначе он «зависнет» до наведения мыши)
    TRAY.with(|t| {
        t.borrow_mut().take();
    });
    std::process::exit(0);
}

fn toggle_autostart() {
    let want = !crate::autostart::is_enabled();
    crate::autostart::set(want);
    // Синхронизируем галочку с реальным состоянием реестра
    TRAY.with(|t| {
        if let Some(st) = t.borrow().as_ref() {
            st.auto.set_checked(crate::autostart::is_enabled());
        }
    });
}

/// Создаёт иконку в трее и меню. false — не удалось (тогда окно закрывается как обычно).
pub fn create(autostart_enabled: bool) -> bool {
    let menu = Menu::new();
    let open = MenuItem::new("Open Rerout", true, None);
    let auto = CheckMenuItem::new("Start with Windows", true, autostart_enabled, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let release_item = MenuItem::new("Quit and release hooks", true, None);
    if menu
        .append_items(&[&open, &auto, &PredefinedMenuItem::separator(), &quit_item, &release_item])
        .is_err()
    {
        return false;
    }
    let open_id = open.id().0.clone();
    let auto_id = auto.id().0.clone();
    let quit_id = quit_item.id().0.clone();
    let release_id = release_item.id().0.clone();

    let Ok(icon) = Icon::from_rgba(crate::logo::icon_rgba(64), 64, 64) else { return false };
    let Ok(tray) = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false) // ЛКМ — открыть окно, ПКМ — меню
        .with_tooltip(format!("Rerout {}", crate::version::VERSION))
        .with_icon(icon)
        .build()
    else {
        return false;
    };

    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let id = e.id.0.as_str();
        if id == open_id {
            show_main_window();
        } else if id == auto_id {
            toggle_autostart();
        } else if id == quit_id {
            quit();
        } else if id == release_id {
            release_and_quit();
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            show_main_window();
        }
    }));

    TRAY.with(|t| {
        *t.borrow_mut() = Some(TrayState { _icon: tray, auto });
    });
    true
}

