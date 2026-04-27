use crate::db::{
    AppSettings, Database, SaveScreenConfigRequest, ScreenConfig, WindowPosition, WindowSize,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, State, WebviewWindow, Window};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, IsWindowVisible, SetWindowPos, ShowWindow, GA_ROOT, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SW_HIDE, SW_SHOWNORMAL,
};

/// 全局悬浮球入口状态（沿用旧命名以兼容历史调用）
pub static IS_FIXED_MODE: AtomicBool = AtomicBool::new(false);

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
