use rusqlite::{Connection, Row};
use serde::{Deserialize, Serialize};

pub const SUBTASK_COLUMNS: &str =
    "id, parent_id, title, content, completed, sort_order, created_at, updated_at";

pub const TODO_COLUMNS: &str = "id, title, description, color, quadrant, completed, sort_order,
     start_time, end_time, created_at, updated_at";

pub fn subtask_from_row(row: &Row) -> rusqlite::Result<SubTask> {
    Ok(SubTask {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        content: row.get(3)?,
        completed: row.get::<_, i32>(4)? != 0,
        sort_order: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn todo_from_row(row: &Row) -> rusqlite::Result<Todo> {
    Ok(Todo {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        color: row.get(3)?,
        quadrant: row.get(4)?,
        completed: row.get::<_, i32>(5)? != 0,
        sort_order: row.get(6)?,
        start_time: row.get(7)?,
        end_time: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        reminder_times: Vec::new(),
        legacy_notify_at: None,
        legacy_notify_before: None,
        legacy_notified: false,
        subtasks: Vec::new(),
    })
}

pub fn load_reminder_times(conn: &Connection, todo_id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT notify_at FROM todo_reminders
         WHERE todo_id = ?
         ORDER BY sort_order ASC, notify_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([todo_id], |row| row.get::<_, String>(0))?;
    rows.collect()
}

pub fn normalize_reminder_times(reminder_times: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for value in reminder_times {
        let trimmed = value.trim();
        if trimmed.is_empty() || result.iter().any(|item| item == trimmed) {
            continue;
        }
        result.push(trimmed.to_string());
    }
    result
}

pub fn replace_reminder_times(
    conn: &Connection,
    todo_id: i64,
    reminder_times: &[String],
) -> rusqlite::Result<()> {
    replace_reminder_times_with_notified(conn, todo_id, reminder_times, false)
}

pub fn replace_reminder_times_with_notified(
    conn: &Connection,
    todo_id: i64,
    reminder_times: &[String],
    notified: bool,
) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM todo_reminders WHERE todo_id = ?", [todo_id])?;
    let normalized = normalize_reminder_times(reminder_times);
    for (index, notify_at) in normalized.iter().enumerate() {
        conn.execute(
            "INSERT INTO todo_reminders (todo_id, notify_at, notified, sort_order)
             VALUES (?1, ?2, ?3, ?4)",
            (
                todo_id,
                notify_at,
                if notified { 1 } else { 0 },
                index as i32,
            ),
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    /// 颜色（HEX 格式，如 #EF4444）
    pub color: String,
    /// 四象限：1=重要紧急, 2=重要不紧急, 3=紧急不重要, 4=不紧急不重要
    pub quadrant: i32,
    pub completed: bool,
    pub sort_order: i32,
    /// 开始时间（可为空，空则使用 created_at）
    pub start_time: Option<String>,
    /// 截止时间（可为空）
    pub end_time: Option<String>,
    #[serde(default)]
    pub reminder_times: Vec<String>,
    #[serde(default, rename = "notifyAt", skip_serializing)]
    pub legacy_notify_at: Option<String>,
    #[serde(default, rename = "notifyBefore", skip_serializing)]
    pub legacy_notify_before: Option<i32>,
    #[serde(default, rename = "notified", skip_serializing)]
    pub legacy_notified: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub subtasks: Vec<SubTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubTask {
    pub id: i64,
    pub parent_id: i64,
    pub title: String,
    pub content: Option<String>,
    pub completed: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTodoRequest {
    pub title: String,
    pub description: Option<String>,
    /// 颜色（HEX 格式，如 #EF4444）
    pub color: String,
    /// 四象限：1=重要紧急, 2=重要不紧急, 3=紧急不重要, 4=不紧急不重要
    #[serde(default = "default_quadrant")]
    pub quadrant: i32,
    #[serde(default)]
    pub reminder_times: Vec<String>,
    /// 开始时间（可为空）
    pub start_time: Option<String>,
    /// 截止时间（可为空）
    pub end_time: Option<String>,
}

fn default_quadrant() -> i32 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTodoRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    /// 颜色（HEX 格式，如 #EF4444）
    pub color: Option<String>,
    /// 四象限：1=重要紧急, 2=重要不紧急, 3=紧急不重要, 4=不紧急不重要
    pub quadrant: Option<i32>,
    pub reminder_times: Option<Vec<String>>,
    pub completed: Option<bool>,
    pub sort_order: Option<i32>,
    /// 是否明确清除提醒时间
    #[serde(default)]
    pub clear_reminder_times: bool,
    /// 开始时间
    pub start_time: Option<String>,
    /// 截止时间
    pub end_time: Option<String>,
    /// 是否明确清除开始时间
    #[serde(default)]
    pub clear_start_time: bool,
    /// 是否明确清除截止时间
    #[serde(default)]
    pub clear_end_time: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSubTaskRequest {
    pub parent_id: i64,
    pub title: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSubTaskRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub completed: Option<bool>,
    pub sort_order: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub is_fixed: bool,
    pub window_position: Option<WindowPosition>,
    pub window_size: Option<WindowSize>,
    /// 是否启用贴边自动隐藏
    #[serde(default = "default_auto_hide_enabled")]
    pub auto_hide_enabled: bool,
    /// 文本主题：light（浅色文字，适配深色背景）或 dark（深色文字，适配浅色背景）
    #[serde(default = "default_text_theme")]
    pub text_theme: String,
    /// 是否显示日历面板
    #[serde(default)]
    pub show_calendar: bool,
    /// 视图模式：list 或 quadrant
    #[serde(default = "default_view_mode")]
    pub view_mode: String,
    /// 通知类型：system 或 app
    #[serde(default = "default_notification_type")]
    pub notification_type: String,
    /// 软件通知位置：bottom_right、bottom_left、top_right、top_left
    #[serde(default = "default_app_notification_position")]
    pub app_notification_position: String,
}

fn default_text_theme() -> String {
    "dark".to_string()
}

fn default_auto_hide_enabled() -> bool {
    true
}

fn default_view_mode() -> String {
    "quadrant".to_string()
}

fn default_notification_type() -> String {
    "system".to_string()
}

fn default_app_notification_position() -> String {
    "bottom_right".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    pub todos: Vec<Todo>,
    pub settings: AppSettings,
}

/// 屏幕配置记录，用于存储不同屏幕组合下的窗口状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenConfig {
    pub id: i64,
    /// 屏幕配置唯一标识（如 "2_2560x1440@125_1920x1080@100"）
    pub config_id: String,
    /// 显示名称（用户可编辑）
    pub display_name: Option<String>,
    /// 窗口 X 坐标
    pub window_x: i32,
    /// 窗口 Y 坐标
    pub window_y: i32,
    /// 窗口宽度
    pub window_width: i32,
    /// 窗口高度
    pub window_height: i32,
    /// 是否固定模式
    pub is_fixed: bool,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
}

/// 保存/更新屏幕配置的请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveScreenConfigRequest {
    pub config_id: String,
    pub display_name: Option<String>,
    pub window_x: i32,
    pub window_y: i32,
    pub window_width: i32,
    pub window_height: i32,
    pub is_fixed: bool,
}
