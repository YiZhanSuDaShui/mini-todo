mod commands;
mod db;
mod services;
#[cfg(target_os = "windows")]
mod windows_tray;

use db::Database;
use services::NotificationService;
#[cfg(not(target_os = "windows"))]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
#[cfg(not(target_os = "windows"))]
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

use commands::{
    clear_floating_bubble_topmost, close_all_notification_windows, close_notification_window,
    create_subtask, create_todo, delete_screen_config, delete_subtask, delete_todo, exit_app,
    export_data, export_data_to_file, fetch_holidays, get_ai_settings,
    get_app_notification_position, get_auto_hide_enabled, get_images_dir, get_notification_type,
    get_screen_config, get_settings, get_show_calendar, get_subtask, get_sync_settings, get_todos,
    get_window_persist_state, hide_main_window, import_data, import_data_from_file,
    import_subtasks_from_paths, list_ai_models, list_screen_configs, plan_todo_with_ai,
    reinforce_floating_bubble_topmost, reorder_todos, reset_window, save_ai_settings,
    save_screen_config, save_settings, save_subtask_image, save_sync_settings,
    set_app_notification_position, set_auto_hide_cursor_inside, set_auto_hide_enabled,
    set_exact_window_size, set_notification_type, set_show_calendar,
    set_window_exact_size_by_label, set_window_fixed_mode, show_main_window, toggle_main_window,
    update_screen_config_name, update_subtask, update_todo, webdav_apply_remote, webdav_auto_sync,
    webdav_download_sync, webdav_sync_now, webdav_test_connection, webdav_upload_sync,
};

#[cfg(not(target_os = "windows"))]
fn show_main_window_from_tray(app: &tauri::AppHandle) {
    if let Some(webview_window) = app.get_webview_window("main") {
        let _ = webview_window.unminimize();
        let _ = webview_window.show();
        let _ = webview_window.set_focus();
    }
}

#[cfg(target_os = "windows")]
fn setup_window_rounded_corners(window: &tauri::WebviewWindow) {
    use raw_window_handle::HasWindowHandle;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    if let Ok(handle) = window.window_handle() {
        if let raw_window_handle::RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
            let hwnd = HWND(win32_handle.hwnd.get() as *mut _);
            unsafe {
                let preference = DWMWCP_ROUND;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &preference as *const _ as *const _,
                    std::mem::size_of_val(&preference) as u32,
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn setup_macos_transparent_webview(window: &tauri::WebviewWindow) {
    use tauri::webview::Color;

    // 把 WKWebView 底色置空，让 CSS 控制最终显示：深色模式透明透出桌面，浅色模式由 .app-container 填白。
    if let Err(e) = window.set_background_color(Some(Color(0, 0, 0, 0))) {
        eprintln!(
            "Failed to set macOS webview background transparent: {:?}",
            e
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化数据库
    let database = Database::new().expect("Failed to initialize database");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(database)
        .setup(|app| {
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    setup_window_rounded_corners(&window);
                }
            }

            #[cfg(target_os = "macos")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    setup_macos_transparent_webview(&window);
                }
            }

            #[cfg(target_os = "windows")]
            {
                let tray = windows_tray::install(app.handle().clone())
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                app.manage(tray);
            }

            #[cfg(not(target_os = "windows"))]
            {
                let title = MenuItem::with_id(app, "title", "Mini Todo", false, None::<&str>)?;
                let separator = PredefinedMenuItem::separator(app)?;
                let show_main =
                    MenuItem::with_id(app, "show_main", "展开界面", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let tray_menu = Menu::with_items(app, &[&title, &separator, &show_main, &quit])?;

                let _tray = TrayIconBuilder::new()
                    .icon(app.default_window_icon().unwrap().clone())
                    .tooltip("Mini Todo")
                    .menu(&tray_menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app: &tauri::AppHandle, event| match event.id().as_ref() {
                        "show_main" => show_main_window_from_tray(app),
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .build(app)?;
            }

            // 启动通知调度器
            NotificationService::start_scheduler(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // TODO 命令
            get_todos,
            create_todo,
            update_todo,
            delete_todo,
            reorder_todos,
            // 子任务命令
            create_subtask,
            update_subtask,
            delete_subtask,
            import_subtasks_from_paths,
            // 图片命令
            get_images_dir,
            get_subtask,
            save_subtask_image,
            // 窗口设置命令
            get_settings,
            save_settings,
            set_window_fixed_mode,
            get_auto_hide_enabled,
            set_auto_hide_enabled,
            set_auto_hide_cursor_inside,
            set_exact_window_size,
            set_window_exact_size_by_label,
            reinforce_floating_bubble_topmost,
            clear_floating_bubble_topmost,
            hide_main_window,
            show_main_window,
            toggle_main_window,
            exit_app,
            get_window_persist_state,
            reset_window,
            // 屏幕配置命令
            get_screen_config,
            save_screen_config,
            list_screen_configs,
            delete_screen_config,
            update_screen_config_name,
            // 日历设置命令
            get_show_calendar,
            set_show_calendar,
            // 数据导入导出命令
            export_data,
            import_data,
            export_data_to_file,
            import_data_from_file,
            // 节假日命令
            fetch_holidays,
            // 通知设置命令
            get_notification_type,
            set_notification_type,
            get_app_notification_position,
            set_app_notification_position,
            // 本地 AI 设置与时间规划命令
            get_ai_settings,
            save_ai_settings,
            list_ai_models,
            plan_todo_with_ai,
            // 通知窗口命令
            close_notification_window,
            close_all_notification_windows,
            // WebDAV 同步命令
            get_sync_settings,
            save_sync_settings,
            webdav_test_connection,
            webdav_upload_sync,
            webdav_download_sync,
            webdav_apply_remote,
            webdav_auto_sync,
            webdav_sync_now,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, _event| {
            // 事件监听（保留空实现）
        });
}
