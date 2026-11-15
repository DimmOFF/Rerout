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
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
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
    if menu
        .append_items(&[&open, &auto, &PredefinedMenuItem::separator(), &quit_item])
        .is_err()
    {
        return false;
    }
    let open_id = open.id().0.clone();
    let auto_id = auto.id().0.clone();
    let quit_id = quit_item.id().0.clone();

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

