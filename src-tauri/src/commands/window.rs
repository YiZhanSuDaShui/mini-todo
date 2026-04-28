use crate::db::{
    AppSettings, Database, SaveScreenConfigRequest, ScreenConfig, WindowPosition, WindowSize,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use tauri::{AppHandle, Manager, State, WebviewWindow, Window};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, GetClassNameW, GetForegroundWindow, GetMessageW, GetShellWindow,
    GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GWL_EXSTYLE, GWL_STYLE,
    HWND_NOTOPMOST, HWND_TOPMOST, MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, WINEVENT_OUTOFCONTEXT, WS_CAPTION,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_THICKFRAME,
};

/// 全局悬浮球入口状态（沿用旧命名以兼容历史调用）
pub static IS_FIXED_MODE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
static FLOATING_BUBBLE_HWND: AtomicIsize = AtomicIsize::new(0);
#[cfg(target_os = "windows")]
static FLOATING_BUBBLE_HOOK_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn hwnd_from_window<T: raw_window_handle::HasWindowHandle>(window: &T) -> Result<HWND, String> {
    let handle = window.window_handle().map_err(|e| e.to_string())?;
    if let raw_window_handle::RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
        let hwnd = HWND(win32_handle.hwnd.get() as *mut _);
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        if root.0.is_null() {
            Ok(hwnd)
        } else {
            Ok(root)
        }
    } else {
        Err("当前窗口不是 Win32 窗口".to_string())
    }
}

#[cfg(target_os = "windows")]
fn hide_window_win32<T: raw_window_handle::HasWindowHandle>(window: &T) -> Result<(), String> {
    let hwnd = hwnd_from_window(window)?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn show_window_win32<T: raw_window_handle::HasWindowHandle>(window: &T) -> Result<(), String> {
    let hwnd = hwnd_from_window(window)?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn is_window_visible_win32<T: raw_window_handle::HasWindowHandle>(
    window: &T,
) -> Result<bool, String> {
    let hwnd = hwnd_from_window(window)?;
    Ok(unsafe { IsWindowVisible(hwnd).as_bool() })
}

#[cfg(target_os = "windows")]
fn set_window_size_win32<T: raw_window_handle::HasWindowHandle>(
    window: &T,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let hwnd = hwnd_from_window(window)?;
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOACTIVATE,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_floating_bubble_tool_window(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let next_ex_style =
            (ex_style | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) & !WS_EX_APPWINDOW.0;
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_ex_style as isize);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

#[cfg(target_os = "windows")]
fn is_same_hwnd(left: HWND, right: HWND) -> bool {
    left.0 == right.0
}

#[cfg(target_os = "windows")]
fn rect_covers_monitor(rect: RECT, monitor: RECT) -> bool {
    const TOLERANCE: i32 = 2;
    rect.left <= monitor.left + TOLERANCE
        && rect.top <= monitor.top + TOLERANCE
        && rect.right >= monitor.right - TOLERANCE
        && rect.bottom >= monitor.bottom - TOLERANCE
}

#[cfg(target_os = "windows")]
fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len <= 0 {
        return String::new();
    }

    String::from_utf16_lossy(&buffer[..len as usize])
}

#[cfg(target_os = "windows")]
fn is_desktop_or_shell_window(hwnd: HWND) -> bool {
    matches!(
        window_class_name(hwnd).as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

#[cfg(target_os = "windows")]
fn foreground_is_fullscreen(foreground: HWND, bubble: HWND) -> bool {
    if foreground.0.is_null() || is_same_hwnd(foreground, bubble) {
        return false;
    }

    unsafe {
        let shell = GetShellWindow();
        if !shell.0.is_null() && is_same_hwnd(foreground, shell) {
            return false;
        }

        if is_desktop_or_shell_window(foreground) {
            return false;
        }

        if !IsWindowVisible(foreground).as_bool() {
            return false;
        }

        let style = GetWindowLongPtrW(foreground, GWL_STYLE) as u32;
        let looks_like_regular_window =
            (style & WS_CAPTION.0) != 0 && (style & WS_THICKFRAME.0) != 0;

        let mut rect = RECT::default();
        if GetWindowRect(foreground, &mut rect).is_err() {
            return false;
        }

        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return false;
        }

        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };

        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return false;
        }

        rect_covers_monitor(rect, monitor_info.rcMonitor) && !looks_like_regular_window
    }
}

#[cfg(target_os = "windows")]
fn apply_floating_bubble_topmost(show_when_allowed: bool) {
    let raw = FLOATING_BUBBLE_HWND.load(Ordering::SeqCst);
    if raw == 0 {
        return;
    }

    let bubble = HWND(raw as *mut _);

    unsafe {
        if !IsWindow(bubble).as_bool() {
            FLOATING_BUBBLE_HWND.store(0, Ordering::SeqCst);
            return;
        }

        let foreground = GetForegroundWindow();
        if foreground_is_fullscreen(foreground, bubble) {
            let _ = ShowWindow(bubble, SW_HIDE);
            let _ = SetWindowPos(
                bubble,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            return;
        }

        if show_when_allowed {
            let _ = ShowWindow(bubble, SW_SHOWNOACTIVATE);
        }

        let _ = SetWindowPos(
            bubble,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn floating_bubble_foreground_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    apply_floating_bubble_topmost(true);
}

#[cfg(target_os = "windows")]
fn ensure_floating_bubble_foreground_hook() {
    if FLOATING_BUBBLE_HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    let spawn_result = std::thread::Builder::new()
        .name("mini-todo-floating-bubble-topmost".to_string())
        .spawn(|| unsafe {
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                HMODULE::default(),
                Some(floating_bubble_foreground_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );

            if hook.0.is_null() {
                FLOATING_BUBBLE_HOOK_STARTED.store(false, Ordering::SeqCst);
                return;
            }

            let mut message = MSG::default();
            while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                let _ = DispatchMessageW(&message);
            }

            let _ = UnhookWinEvent(hook);
            FLOATING_BUBBLE_HOOK_STARTED.store(false, Ordering::SeqCst);
        });

    if spawn_result.is_err() {
        FLOATING_BUBBLE_HOOK_STARTED.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "windows")]
fn reinforce_floating_bubble_topmost_win32(
    window: &WebviewWindow,
    show_when_allowed: bool,
) -> Result<(), String> {
    let hwnd = hwnd_from_window(window)?;
    set_floating_bubble_tool_window(hwnd);
    FLOATING_BUBBLE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    ensure_floating_bubble_foreground_hook();
    apply_floating_bubble_topmost(show_when_allowed);
    Ok(())
}

fn get_auto_hide_enabled_value(db: &State<Database>) -> bool {
    db.with_connection(|conn| {
        let enabled: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'auto_hide_enabled'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(val == "true")
                },
            )
            .unwrap_or(true);
        Ok(enabled)
    })
    .unwrap_or(true)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPersistState {
    pub position: WindowPosition,
    pub size: WindowSize,
}

#[tauri::command]
pub fn get_settings(db: State<Database>) -> Result<AppSettings, String> {
    db.with_connection(|conn| {
        let is_fixed: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'is_fixed'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(val == "true")
                },
            )
            .unwrap_or(false);

        let window_position: Option<WindowPosition> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'window_position'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(serde_json::from_str(&val).ok())
                },
            )
            .unwrap_or(None);

        let window_size: Option<WindowSize> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'window_size'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(serde_json::from_str(&val).ok())
                },
            )
            .unwrap_or(None);

        let auto_hide_enabled: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'auto_hide_enabled'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(val == "true")
                },
            )
            .unwrap_or(true);

        let text_theme: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'text_theme'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "dark".to_string());

        let show_calendar: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'show_calendar'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(val == "true")
                },
            )
            .unwrap_or(false);

        let view_mode: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'view_mode'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "quadrant".to_string());

        let notification_type: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'notification_type'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "system".to_string());

        let app_notification_position: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'app_notification_position'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "bottom_right".to_string());

        Ok(AppSettings {
            is_fixed,
            window_position,
            window_size,
            auto_hide_enabled,
            text_theme,
            show_calendar,
            view_mode,
            notification_type,
            app_notification_position,
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_settings(db: State<Database>, settings: AppSettings) -> Result<(), String> {
    db.with_connection(|conn| {
        // 保存 is_fixed
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('is_fixed', ?, datetime('now', 'localtime'))",
            [if settings.is_fixed { "true" } else { "false" }],
        )?;

        // 保存窗口位置
        if let Some(pos) = &settings.window_position {
            let pos_json = serde_json::to_string(pos).unwrap_or_default();
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('window_position', ?, datetime('now', 'localtime'))",
                [&pos_json],
            )?;
        }

        // 保存窗口尺寸
        if let Some(size) = &settings.window_size {
            let size_json = serde_json::to_string(size).unwrap_or_default();
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('window_size', ?, datetime('now', 'localtime'))",
                [&size_json],
            )?;
        }

        // 保留旧版本字段，避免升级后读取历史配置失败
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('auto_hide_enabled', ?, datetime('now', 'localtime'))",
            [if settings.auto_hide_enabled { "true" } else { "false" }],
        )?;

        // 保存文本主题
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('text_theme', ?, datetime('now', 'localtime'))",
            [&settings.text_theme],
        )?;

        Ok(())
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_window_fixed_mode(
    _window: Window,
    _db: State<Database>,
    fixed: bool,
) -> Result<(), String> {
    // 旧命令现在只同步悬浮球入口状态，不再修改主窗口样式或锁定窗口。
    IS_FIXED_MODE.store(fixed, Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    if !fixed {
        FLOATING_BUBBLE_HWND.store(0, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn set_auto_hide_cursor_inside(_inside: bool) -> Result<(), String> {
    // 旧版贴边隐藏命令保留为空操作，兼容已安装旧前端或历史调用。
    Ok(())
}

#[tauri::command]
pub fn set_exact_window_size(window: Window, width: i32, height: i32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        set_window_size_win32(&window, width, height)?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: width.max(1) as u32,
                height: height.max(1) as u32,
            }))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn set_window_exact_size_by_label(
    app_handle: AppHandle,
    label: String,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let window = app_handle
        .get_webview_window(&label)
        .ok_or_else(|| format!("未找到窗口: {}", label))?;

    #[cfg(target_os = "windows")]
    {
        set_window_size_win32(&window, width, height)?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: width.max(1) as u32,
                height: height.max(1) as u32,
            }))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn reinforce_floating_bubble_topmost(app_handle: AppHandle) -> Result<(), String> {
    let window = app_handle
        .get_webview_window("fixed-bubble")
        .ok_or_else(|| "未找到悬浮球窗口".to_string())?;

    #[cfg(target_os = "windows")]
    {
        reinforce_floating_bubble_topmost_win32(&window, true)?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window.set_always_on_top(true).map_err(|e| e.to_string())?;
        window.set_skip_taskbar(true).map_err(|e| e.to_string())?;
        let _ = window.show();
    }

    Ok(())
}

#[tauri::command]
pub fn clear_floating_bubble_topmost() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        FLOATING_BUBBLE_HWND.store(0, Ordering::SeqCst);
    }

    Ok(())
}

#[tauri::command]
pub fn hide_main_window(app_handle: AppHandle) -> Result<(), String> {
    let window = app_handle
        .get_webview_window("main")
        .ok_or_else(|| "未找到主窗口".to_string())?;

    #[cfg(target_os = "windows")]
    {
        hide_window_win32(&window)?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window.hide().map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn show_main_window(app_handle: AppHandle) -> Result<(), String> {
    let window = app_handle
        .get_webview_window("main")
        .ok_or_else(|| "未找到主窗口".to_string())?;

    let _ = window.unminimize();
    ensure_main_window_onscreen(&window)?;

    #[cfg(target_os = "windows")]
    {
        show_window_win32(&window)?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window.show().map_err(|e| e.to_string())?;
    }

    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
pub fn toggle_main_window(app_handle: AppHandle) -> Result<bool, String> {
    let window = app_handle
        .get_webview_window("main")
        .ok_or_else(|| "未找到主窗口".to_string())?;

    #[cfg(target_os = "windows")]
    let visible = is_window_visible_win32(&window)?;

    #[cfg(not(target_os = "windows"))]
    let visible = window.is_visible().map_err(|e| e.to_string())?;

    let onscreen = is_window_usable_on_screen(&window).unwrap_or(false);

    if visible && onscreen {
        #[cfg(target_os = "windows")]
        {
            hide_window_win32(&window)?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            window.hide().map_err(|e| e.to_string())?;
        }

        Ok(false)
    } else {
        let _ = window.unminimize();
        ensure_main_window_onscreen(&window)?;

        #[cfg(target_os = "windows")]
        {
            show_window_win32(&window)?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            window.show().map_err(|e| e.to_string())?;
        }

        let _ = window.set_focus();
        Ok(true)
    }
}

fn is_window_usable_on_screen(window: &WebviewWindow) -> Result<bool, String> {
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;

    if size.width < 320 || size.height < 400 {
        return Ok(false);
    }

    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    if monitors.is_empty() {
        return Ok(true);
    }

    let left = position.x;
    let top = position.y;
    let right = position.x + size.width as i32;
    let bottom = position.y + size.height as i32;

    Ok(monitors.iter().any(|monitor| {
        let monitor_left = monitor.position().x;
        let monitor_top = monitor.position().y;
        let monitor_right = monitor_left + monitor.size().width as i32;
        let monitor_bottom = monitor_top + monitor.size().height as i32;

        right > monitor_left && left < monitor_right && bottom > monitor_top && top < monitor_bottom
    }))
}

fn ensure_main_window_onscreen(window: &WebviewWindow) -> Result<(), String> {
    if is_window_usable_on_screen(window).unwrap_or(false) {
        return Ok(());
    }

    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| {
            window
                .available_monitors()
                .ok()
                .and_then(|mut items| items.pop())
        });

    let default_width = 380.0;
    let default_height = 600.0;
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: default_width,
            height: default_height,
        }))
        .map_err(|e| e.to_string())?;

    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let width = (default_width * scale).round() as i32;
        let height = (default_height * scale).round() as i32;
        let x = monitor.position().x + (monitor.size().width as i32 - width) / 2;
        let y = monitor.position().y + (monitor.size().height as i32 - height) / 2;

        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn exit_app(app_handle: AppHandle) -> Result<(), String> {
    app_handle.exit(0);
    Ok(())
}

#[tauri::command]
pub fn get_auto_hide_enabled(db: State<Database>) -> Result<bool, String> {
    Ok(get_auto_hide_enabled_value(&db))
}

#[tauri::command]
pub fn set_auto_hide_enabled(
    _app_handle: tauri::AppHandle,
    db: State<Database>,
    enabled: bool,
) -> Result<(), String> {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('auto_hide_enabled', ?, datetime('now', 'localtime'))",
            [if enabled { "true" } else { "false" }],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn get_window_persist_state(window: Window) -> Result<WindowPersistState, String> {
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;

    let persist = WindowPersistState {
        position: WindowPosition { x: pos.x, y: pos.y },
        size: WindowSize {
            width: size.width,
            height: size.height,
        },
    };

    Ok(persist)
}

/// 重置窗口位置和大小（用于 Tauri 命令）
#[tauri::command]
pub fn reset_window(window: Window) -> Result<(), String> {
    reset_window_impl(&window)
}

/// 内部重置窗口实现
fn reset_window_impl<T: tauri::Runtime>(window: &impl WindowExt<T>) -> Result<(), String> {
    // 重置到屏幕左上角（10%边距），默认大小 380x600
    let default_width = 380.0;
    let default_height = 600.0;

    // 获取主显示器信息并计算 10% 边距位置
    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let size = monitor.size();
        let position = monitor.position();

        // 计算 10% 边距
        let margin_x = (size.width as f64 * 0.1 / scale) as i32;
        let margin_y = (size.height as f64 * 0.1 / scale) as i32;

        let x = position.x + margin_x;
        let y = position.y + margin_y;

        // 设置位置
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));

        // 设置大小
        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: default_width,
            height: default_height,
        }));

        // 确保可调整大小
        let _ = window.set_resizable(true);
    }

    Ok(())
}

/// 窗口扩展 trait
trait WindowExt<R: tauri::Runtime> {
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>>;
    fn set_position(&self, position: tauri::Position) -> tauri::Result<()>;
    fn set_size(&self, size: tauri::Size) -> tauri::Result<()>;
    fn set_resizable(&self, resizable: bool) -> tauri::Result<()>;
}

impl<R: tauri::Runtime> WindowExt<R> for Window<R> {
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>> {
        Window::primary_monitor(self)
    }
    fn set_position(&self, position: tauri::Position) -> tauri::Result<()> {
        Window::set_position(self, position)
    }
    fn set_size(&self, size: tauri::Size) -> tauri::Result<()> {
        Window::set_size(self, size)
    }
    fn set_resizable(&self, resizable: bool) -> tauri::Result<()> {
        Window::set_resizable(self, resizable)
    }
}

impl<R: tauri::Runtime> WindowExt<R> for WebviewWindow<R> {
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>> {
        WebviewWindow::primary_monitor(self)
    }
    fn set_position(&self, position: tauri::Position) -> tauri::Result<()> {
        WebviewWindow::set_position(self, position)
    }
    fn set_size(&self, size: tauri::Size) -> tauri::Result<()> {
        WebviewWindow::set_size(self, size)
    }
    fn set_resizable(&self, resizable: bool) -> tauri::Result<()> {
        WebviewWindow::set_resizable(self, resizable)
    }
}

// ============ 屏幕配置相关命令 ============

/// 根据屏幕配置标识获取保存的窗口配置
#[tauri::command]
pub fn get_screen_config(
    db: State<Database>,
    config_id: String,
) -> Result<Option<ScreenConfig>, String> {
    db.with_connection(|conn| {
        let result = conn.query_row(
            "SELECT id, config_id, display_name, window_x, window_y, window_width, window_height, 
                    is_fixed, created_at, updated_at 
             FROM screen_configs WHERE config_id = ?",
            [&config_id],
            |row| {
                Ok(ScreenConfig {
                    id: row.get(0)?,
                    config_id: row.get(1)?,
                    display_name: row.get(2)?,
                    window_x: row.get(3)?,
                    window_y: row.get(4)?,
                    window_width: row.get(5)?,
                    window_height: row.get(6)?,
                    is_fixed: row.get::<_, i32>(7)? != 0,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        );

        match result {
            Ok(config) => Ok(Some(config)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .map_err(|e| e.to_string())
}

/// 保存或更新屏幕配置
#[tauri::command]
pub fn save_screen_config(
    db: State<Database>,
    config: SaveScreenConfigRequest,
) -> Result<ScreenConfig, String> {
    db.with_connection(|conn| {
        // 使用 INSERT OR REPLACE 来保存或更新
        conn.execute(
            "INSERT INTO screen_configs 
             (config_id, display_name, window_x, window_y, window_width, window_height, is_fixed, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now', 'localtime'))
             ON CONFLICT(config_id) DO UPDATE SET
                display_name = COALESCE(?2, display_name),
                window_x = ?3,
                window_y = ?4,
                window_width = ?5,
                window_height = ?6,
                is_fixed = ?7,
                updated_at = datetime('now', 'localtime')",
            (
                &config.config_id,
                &config.display_name,
                config.window_x,
                config.window_y,
                config.window_width,
                config.window_height,
                if config.is_fixed { 1 } else { 0 },
            ),
        )?;

        // 返回保存后的配置
        conn.query_row(
            "SELECT id, config_id, display_name, window_x, window_y, window_width, window_height, 
                    is_fixed, created_at, updated_at 
             FROM screen_configs WHERE config_id = ?",
            [&config.config_id],
            |row| {
                Ok(ScreenConfig {
                    id: row.get(0)?,
                    config_id: row.get(1)?,
                    display_name: row.get(2)?,
                    window_x: row.get(3)?,
                    window_y: row.get(4)?,
                    window_width: row.get(5)?,
                    window_height: row.get(6)?,
                    is_fixed: row.get::<_, i32>(7)? != 0,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
    })
    .map_err(|e| e.to_string())
}

/// 获取所有屏幕配置列表
#[tauri::command]
pub fn list_screen_configs(db: State<Database>) -> Result<Vec<ScreenConfig>, String> {
    db.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, config_id, display_name, window_x, window_y, window_width, window_height, 
                    is_fixed, created_at, updated_at 
             FROM screen_configs ORDER BY updated_at DESC",
        )?;

        let configs = stmt.query_map([], |row| {
            Ok(ScreenConfig {
                id: row.get(0)?,
                config_id: row.get(1)?,
                display_name: row.get(2)?,
                window_x: row.get(3)?,
                window_y: row.get(4)?,
                window_width: row.get(5)?,
                window_height: row.get(6)?,
                is_fixed: row.get::<_, i32>(7)? != 0,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;

        configs.collect::<Result<Vec<_>, _>>()
    })
    .map_err(|e| e.to_string())
}

/// 删除屏幕配置
#[tauri::command]
pub fn delete_screen_config(db: State<Database>, config_id: String) -> Result<(), String> {
    db.with_connection(|conn| {
        conn.execute(
            "DELETE FROM screen_configs WHERE config_id = ?",
            [&config_id],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

/// 更新屏幕配置的显示名称
#[tauri::command]
pub fn update_screen_config_name(
    db: State<Database>,
    config_id: String,
    display_name: String,
) -> Result<(), String> {
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE screen_configs SET display_name = ?, updated_at = datetime('now', 'localtime') WHERE config_id = ?",
            [&display_name, &config_id],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

// ============ 日历设置相关命令 ============

/// 获取是否显示日历
#[tauri::command]
pub fn get_show_calendar(db: State<Database>) -> Result<bool, String> {
    db.with_connection(|conn| {
        let show: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'show_calendar'",
                [],
                |row| {
                    let val: String = row.get(0)?;
                    Ok(val == "true")
                },
            )
            .unwrap_or(false);
        Ok(show)
    })
    .map_err(|e| e.to_string())
}

/// 设置是否显示日历
#[tauri::command]
pub fn set_show_calendar(db: State<Database>, show: bool) -> Result<(), String> {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('show_calendar', ?, datetime('now', 'localtime'))",
            [if show { "true" } else { "false" }],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}
