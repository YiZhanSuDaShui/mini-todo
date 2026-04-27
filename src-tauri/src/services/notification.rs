use crate::db::Database;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::async_runtime;
use tauri::Manager;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri_plugin_notification::NotificationExt;

// 通知窗口计数器（用于生成唯一的窗口标签）
static NOTIFICATION_COUNTER: AtomicU32 = AtomicU32::new(0);
// 当前显示的通知窗口数量（用于堆叠计算）
static ACTIVE_NOTIFICATIONS: AtomicU32 = AtomicU32::new(0);

// 通知窗口尺寸
const NOTIFICATION_WIDTH: u32 = 320;
const NOTIFICATION_HEIGHT: u32 = 120;
const NOTIFICATION_MARGIN: u32 = 20;
const NOTIFICATION_SPACING: u32 = 10;

pub struct NotificationService;

#[derive(Clone, Copy)]
enum AppNotificationPosition {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

impl AppNotificationPosition {
    fn from_setting(value: &str) -> Self {
        match value {
            "bottom_left" => Self::BottomLeft,
            "top_right" => Self::TopRight,
            "top_left" => Self::TopLeft,
            _ => Self::BottomRight,
        }
    }
}

impl NotificationService {
    /// 启动通知调度器，每分钟检查一次待办通知
    pub fn start_scheduler(app_handle: tauri::AppHandle) {
        async_runtime::spawn(async move {
            // 等待应用初始化完成
            tokio::time::sleep(Duration::from_secs(5)).await;

            loop {
                Self::sleep_until_next_minute().await;
                if let Err(e) = Self::check_and_send_notifications(&app_handle) {
                    eprintln!("通知检查失败: {}", e);
                }
            }
        });
    }

    /// 等待到下一个整分（本地时间）
    async fn sleep_until_next_minute() {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0));
        let secs = since_epoch.as_secs();
        let nanos = since_epoch.subsec_nanos();
        let remainder = secs % 60;

        if remainder == 0 && nanos == 0 {
            return;
        }

        let mut wait_secs = 59 - remainder;
        let mut wait_nanos = 1_000_000_000 - nanos;
        if wait_nanos == 1_000_000_000 {
            wait_secs += 1;
            wait_nanos = 0;
        }

        tokio::time::sleep(Duration::new(wait_secs, wait_nanos)).await;
    }

    /// 检查并发送到期的通知
    fn check_and_send_notifications(app_handle: &tauri::AppHandle) -> Result<(), String> {
        let db = app_handle.state::<Database>();

        // 获取通知类型设置
        let notification_type = Self::get_notification_type(&db);
        let app_notification_position = Self::get_app_notification_position(&db);

        // 获取需要通知的待办
        let todos = Self::get_pending_notifications(&db)?;

        for todo in todos {
            // 根据设置发送不同类型的通知
            match notification_type.as_str() {
                "app" => {
                    Self::send_app_notification(
                        app_handle,
                        &todo.title,
                        &todo.description,
                        app_notification_position,
                    )?;
                }
                _ => {
                    Self::send_system_notification(app_handle, &todo.title, &todo.description)?;
                }
            }

            // 标记为已通知
            Self::mark_as_notified(&db, todo.reminder_id)?;
        }

        Ok(())
    }

    /// 获取通知类型设置
    fn get_notification_type(db: &Database) -> String {
        db.with_connection(|conn| {
            let result: String = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'notification_type'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| "system".to_string());
            Ok(result)
        })
        .unwrap_or_else(|_| "system".to_string())
    }

    /// 获取软件通知位置设置
    fn get_app_notification_position(db: &Database) -> AppNotificationPosition {
        let position = db
            .with_connection(|conn| {
                let result: String = conn
                    .query_row(
                        "SELECT value FROM settings WHERE key = 'app_notification_position'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| "bottom_right".to_string());
                Ok(result)
            })
            .unwrap_or_else(|_| "bottom_right".to_string());

        AppNotificationPosition::from_setting(&position)
    }

    /// 获取需要发送通知的待办列表
    fn get_pending_notifications(db: &Database) -> Result<Vec<PendingNotification>, String> {
        db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT r.id, t.title, t.description
                FROM todo_reminders r
                JOIN todos t ON t.id = r.todo_id
                WHERE t.completed = 0
                  AND r.notified = 0
                  AND datetime(replace(r.notify_at, 'T', ' ')) <= datetime('now', 'localtime')
                ORDER BY r.notify_at ASC
                "#,
            )?;

            let todos = stmt
                .query_map([], |row| {
                    Ok(PendingNotification {
                        reminder_id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get::<_, Option<String>>(2)?,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok(todos)
        })
        .map_err(|e| e.to_string())
    }

    /// 发送系统通知
    fn send_system_notification(
        app_handle: &tauri::AppHandle,
        title: &str,
        description: &Option<String>,
    ) -> Result<(), String> {
        let body = description.as_deref().unwrap_or("待办事项提醒");

        app_handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// 发送软件通知（创建通知窗口）
    fn send_app_notification(
        app_handle: &tauri::AppHandle,
        title: &str,
        description: &Option<String>,
        position: AppNotificationPosition,
    ) -> Result<(), String> {
        // 生成唯一的窗口标签
        let counter = NOTIFICATION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let window_label = format!("notification_{}", counter);

        // 获取当前活动通知数量，用于计算堆叠位置
        let active_count = ACTIVE_NOTIFICATIONS.fetch_add(1, Ordering::SeqCst);

        // 获取主显示器工作区，以逻辑像素计算通知窗口位置。
        // Tauri 的窗口 position/inner_size 使用逻辑像素，而 monitor 返回物理像素。
        let (x, y) = Self::calculate_notification_position(app_handle, active_count, position);

        // URL 编码标题和描述
        let encoded_title = urlencoding::encode(title);
        let encoded_desc = urlencoding::encode(description.as_deref().unwrap_or("待办事项提醒"));
        let encoded_label = urlencoding::encode(&window_label);

        // 创建通知窗口
        let url = format!(
            "index.html#/notification?title={}&description={}&label={}",
            encoded_title, encoded_desc, encoded_label
        );

        let window_label_clone = window_label.clone();
        let app_handle_clone = app_handle.clone();

        // 在主线程创建窗口
        let mut window_builder =
            WebviewWindowBuilder::new(app_handle, &window_label, WebviewUrl::App(url.into()))
                .title("通知")
                .inner_size(NOTIFICATION_WIDTH as f64, NOTIFICATION_HEIGHT as f64)
                .position(x as f64, y as f64)
                .decorations(false)
                .always_on_top(true)
                .resizable(false)
                .skip_taskbar(true)
                .focused(false)
                .visible(true);

        #[cfg(not(target_os = "macos"))]
        {
            window_builder = window_builder.transparent(true);
        }

        let _ = match window_builder.build() {
            Ok(window) => window,
            Err(e) => {
                Self::decrement_active_notifications();
                return Err(e.to_string());
            }
        };

        // 监听窗口销毁事件
        if let Some(window) = app_handle_clone.get_webview_window(&window_label_clone) {
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Destroyed = event {
                    NotificationService::decrement_active_notifications();
                }
            });
        }

        Ok(())
    }

    /// 获取主显示器尺寸
    fn calculate_notification_position(
        app_handle: &tauri::AppHandle,
        active_count: u32,
        position: AppNotificationPosition,
    ) -> (f64, f64) {
        if let Some(monitor) = app_handle.primary_monitor().ok().flatten() {
            let scale_factor = monitor.scale_factor().max(1.0);
            let work_area = monitor.work_area();

            let (physical_x, physical_y, physical_width, physical_height) =
                if work_area.size.width > 0 && work_area.size.height > 0 {
                    (
                        work_area.position.x,
                        work_area.position.y,
                        work_area.size.width,
                        work_area.size.height,
                    )
                } else {
                    let monitor_position = monitor.position();
                    let monitor_size = monitor.size();
                    (
                        monitor_position.x,
                        monitor_position.y,
                        monitor_size.width,
                        monitor_size.height,
                    )
                };

            let work_x = physical_x as f64 / scale_factor;
            let work_y = physical_y as f64 / scale_factor;
            let work_width = physical_width as f64 / scale_factor;
            let work_height = physical_height as f64 / scale_factor;

            return Self::position_in_work_area(
                work_x,
                work_y,
                work_width,
                work_height,
                active_count,
                position,
            );
        }

        Self::position_in_work_area(0.0, 0.0, 1920.0, 1080.0, active_count, position)
    }

    fn position_in_work_area(
        work_x: f64,
        work_y: f64,
        work_width: f64,
        work_height: f64,
        active_count: u32,
        position: AppNotificationPosition,
    ) -> (f64, f64) {
        let width = NOTIFICATION_WIDTH as f64;
        let height = NOTIFICATION_HEIGHT as f64;
        let margin = NOTIFICATION_MARGIN as f64;
        let stack_step = (NOTIFICATION_HEIGHT + NOTIFICATION_SPACING) as f64;
        let stack_offset = active_count as f64 * stack_step;

        let min_x = work_x + margin;
        let max_x = work_x + work_width - width - margin;
        let min_y = work_y + margin;
        let max_y = work_y + work_height - height - margin;

        let x = match position {
            AppNotificationPosition::BottomLeft | AppNotificationPosition::TopLeft => min_x,
            AppNotificationPosition::BottomRight | AppNotificationPosition::TopRight => max_x,
        };

        let y = match position {
            AppNotificationPosition::TopLeft | AppNotificationPosition::TopRight => {
                min_y + stack_offset
            }
            AppNotificationPosition::BottomLeft | AppNotificationPosition::BottomRight => {
                max_y - stack_offset
            }
        };

        (
            Self::clamp_axis(x, min_x, max_x),
            Self::clamp_axis(y, min_y, max_y),
        )
    }

    fn clamp_axis(value: f64, min: f64, max: f64) -> f64 {
        if max < min {
            min
        } else {
            value.clamp(min, max)
        }
    }

    fn decrement_active_notifications() {
        loop {
            let current = ACTIVE_NOTIFICATIONS.load(Ordering::SeqCst);
            if current == 0 {
                return;
            }

            if ACTIVE_NOTIFICATIONS
                .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    /// 标记单条提醒为已通知
    fn mark_as_notified(db: &Database, reminder_id: i64) -> Result<(), String> {
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE todo_reminders SET notified = 1, updated_at = datetime('now', 'localtime') WHERE id = ?",
                [reminder_id],
            )?;
            Ok(())
        })
        .map_err(|e| e.to_string())
    }
}

/// 待发送通知的待办
struct PendingNotification {
    reminder_id: i64,
    title: String,
    description: Option<String>,
}
