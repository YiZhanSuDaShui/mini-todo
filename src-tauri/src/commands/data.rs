use crate::commands::sync_cmd::mark_webdav_local_dirty;
use crate::db::{
    load_reminder_times, normalize_reminder_times, replace_reminder_times_with_notified,
    subtask_from_row, todo_from_row, AppSettings, Database, ExportData, Todo, WindowPosition,
    WindowSize, SUBTASK_COLUMNS, TODO_COLUMNS,
};
use chrono::{Duration, Local, NaiveDateTime};
use rusqlite::params;
use std::io::{Read as _, Write as _};
use tauri::State;

/// 从 settings 表读取字符串值的辅助函数
fn get_setting_string(conn: &rusqlite::Connection, key: &str, default: &str) -> String {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .unwrap_or_else(|_| default.to_string())
}

/// 从 settings 表读取布尔值的辅助函数
fn get_setting_bool(conn: &rusqlite::Connection, key: &str, default: bool) -> bool {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        let val: String = row.get(0)?;
        Ok(val == "true")
    })
    .unwrap_or(default)
}

fn now_db_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn get_or_create_sync_device_id(conn: &rusqlite::Connection) -> rusqlite::Result<String> {
    if let Ok(device_id) = conn.query_row(
        "SELECT value FROM settings WHERE key = 'webdav_device_id'",
        [],
        |row| row.get::<_, String>(0),
    ) {
        if !device_id.trim().is_empty() {
            return Ok(device_id);
        }
    }

    let device_id = format!("dev_{}", Local::now().timestamp_millis());
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at)
         VALUES ('webdav_device_id', ?1, datetime('now', 'localtime'))",
        [&device_id],
    )?;
    Ok(device_id)
}

fn record_tombstone(
    conn: &rusqlite::Connection,
    entity: &str,
    sync_id: &str,
    deleted_at: &str,
    device_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sync_tombstones (entity, sync_id, deleted_at, deleted_by_device_id)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(entity, sync_id) DO UPDATE SET
            deleted_at = excluded.deleted_at,
            deleted_by_device_id = excluded.deleted_by_device_id",
        (entity, sync_id, deleted_at, device_id),
    )?;
    Ok(())
}

fn record_existing_tombstones(
    conn: &rusqlite::Connection,
    deleted_at: &str,
    device_id: &str,
) -> rusqlite::Result<()> {
    for (table, entity) in [
        ("todo_reminders", "reminder"),
        ("subtasks", "subtask"),
        ("todos", "todo"),
    ] {
        let mut stmt = conn.prepare(&format!(
            "SELECT sync_id FROM {} WHERE sync_id IS NOT NULL AND sync_id <> ''",
            table
        ))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            record_tombstone(conn, entity, &row?, deleted_at, device_id)?;
        }
    }
    Ok(())
}

fn clear_sync_fingerprints(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM settings
         WHERE key IN (
            'webdav_incremental_manifest_fingerprint',
            'webdav_archive_file_fingerprint'
         )",
        [],
    )?;
    Ok(())
}

fn read_app_settings(conn: &rusqlite::Connection) -> AppSettings {
    let is_fixed = get_setting_bool(conn, "is_fixed", false);
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
    let text_theme = get_setting_string(conn, "text_theme", "dark");
    let auto_hide_enabled = get_setting_bool(conn, "auto_hide_enabled", true);
    let show_calendar = get_setting_bool(conn, "show_calendar", false);
    let view_mode = get_setting_string(conn, "view_mode", "quadrant");
    let notification_type = get_setting_string(conn, "notification_type", "system");
    let app_notification_position =
        get_setting_string(conn, "app_notification_position", "bottom_right");

    AppSettings {
        is_fixed,
        window_position,
        window_size,
        auto_hide_enabled,
        text_theme,
        show_calendar,
        view_mode,
        notification_type,
        app_notification_position,
    }
}

fn write_app_settings(conn: &rusqlite::Connection, settings: &AppSettings) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('is_fixed', ?1, datetime('now', 'localtime'))",
        [if settings.is_fixed { "true" } else { "false" }],
    )?;
    if let Some(pos) = &settings.window_position {
        let pos_json = serde_json::to_string(pos).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('window_position', ?1, datetime('now', 'localtime'))",
            [&pos_json],
        )?;
    }
    if let Some(size) = &settings.window_size {
        let size_json = serde_json::to_string(size).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('window_size', ?1, datetime('now', 'localtime'))",
            [&size_json],
        )?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('auto_hide_enabled', ?1, datetime('now', 'localtime'))",
        [if settings.auto_hide_enabled { "true" } else { "false" }],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('text_theme', ?1, datetime('now', 'localtime'))",
        [&settings.text_theme],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('show_calendar', ?1, datetime('now', 'localtime'))",
        [if settings.show_calendar { "true" } else { "false" }],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('view_mode', ?1, datetime('now', 'localtime'))",
        [&settings.view_mode],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('notification_type', ?1, datetime('now', 'localtime'))",
        [&settings.notification_type],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('app_notification_position', ?1, datetime('now', 'localtime'))",
        [&settings.app_notification_position],
    )?;
    Ok(())
}

pub fn export_data_internal(db: &Database, _include_executions: bool) -> Result<String, String> {
    let result = db.with_connection(|conn| {
        let todo_sql = format!("SELECT {} FROM todos ORDER BY sort_order ASC", TODO_COLUMNS);
        let mut stmt = conn.prepare(&todo_sql)?;
        let todo_iter = stmt.query_map([], |row| todo_from_row(row))?;

        let mut todos: Vec<Todo> = todo_iter.filter_map(|t| t.ok()).collect();

        for todo in &mut todos {
            let subtask_sql = format!(
                "SELECT {} FROM subtasks WHERE parent_id = ? ORDER BY sort_order ASC",
                SUBTASK_COLUMNS
            );
            let mut subtask_stmt = conn.prepare(&subtask_sql)?;
            let subtask_iter = subtask_stmt.query_map([todo.id], |row| subtask_from_row(row))?;

            todo.reminder_times = load_reminder_times(conn, todo.id)?;
            todo.subtasks = subtask_iter.filter_map(|s| s.ok()).collect();
        }

        let settings = read_app_settings(conn);

        Ok((todos, settings))
    });

    match result {
        Ok((todos, settings)) => {
            let export_data = ExportData {
                version: "3.0".to_string(),
                exported_at: Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
                todos,
                settings,
            };
            serde_json::to_string_pretty(&export_data).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn import_data_raw(db: &Database, json_data: &str) -> Result<(), String> {
    let import: ExportData =
        serde_json::from_str(json_data).map_err(|e| format!("Invalid JSON format: {}", e))?;

    db.with_connection(|conn| {
        let import_time = now_db_time();
        let device_id = get_or_create_sync_device_id(conn)?;
        record_existing_tombstones(conn, &import_time, &device_id)?;

        conn.execute("DELETE FROM todo_reminders", [])?;
        conn.execute("DELETE FROM subtasks", [])?;
        conn.execute("DELETE FROM todos", [])?;

        // 1. Import todos with subtasks
        for todo in &import.todos {
            let completed_i = if todo.completed { 1i32 } else { 0 };

            conn.execute(
                "INSERT INTO todos (title, description, color, quadrant, completed, sort_order,
                                   start_time, end_time, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    todo.title,
                    todo.description,
                    todo.color,
                    todo.quadrant,
                    completed_i,
                    todo.sort_order,
                    todo.start_time,
                    todo.end_time,
                    todo.created_at,
                    &import_time,
                ],
            )?;

            let new_todo_id = conn.last_insert_rowid();
            let todo_sync_id = format!("todo:{}:{}", device_id, new_todo_id);
            conn.execute(
                "UPDATE todos SET sync_id = ?1 WHERE id = ?2",
                params![&todo_sync_id, new_todo_id],
            )?;

            let reminder_times = import_reminder_times(todo);
            replace_reminder_times_with_notified(
                conn,
                new_todo_id,
                &reminder_times,
                todo.legacy_notified,
            )?;
            let mut reminder_stmt =
                conn.prepare("SELECT id FROM todo_reminders WHERE todo_id = ?1")?;
            let reminder_ids = reminder_stmt
                .query_map([new_todo_id], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for reminder_id in reminder_ids {
                let reminder_sync_id = format!("reminder:{}:{}", device_id, reminder_id);
                conn.execute(
                    "UPDATE todo_reminders SET sync_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![&reminder_sync_id, &import_time, reminder_id],
                )?;
            }

            // 2. Import subtasks
            for subtask in &todo.subtasks {
                let sub_completed_i = if subtask.completed { 1i32 } else { 0 };
                conn.execute(
                    "INSERT INTO subtasks (parent_id, title, content, completed, sort_order, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        new_todo_id, subtask.title, subtask.content,
                        sub_completed_i,
                        subtask.sort_order, subtask.created_at, &import_time,
                    ],
                )?;
                let new_subtask_id = conn.last_insert_rowid();
                let subtask_sync_id = format!("subtask:{}:{}", device_id, new_subtask_id);
                conn.execute(
                    "UPDATE subtasks SET sync_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![&subtask_sync_id, &import_time, new_subtask_id],
                )?;
            }
        }

        // 3. Import settings
        write_app_settings(conn, &import.settings)?;
        clear_sync_fingerprints(conn)?;
        mark_webdav_local_dirty(conn)?;

        Ok(())
    })
    .map_err(|e| e.to_string())
}

fn import_reminder_times(todo: &Todo) -> Vec<String> {
    let reminder_times = normalize_reminder_times(&todo.reminder_times);
    if !reminder_times.is_empty() {
        return reminder_times;
    }

    todo.legacy_notify_at
        .as_ref()
        .map(|value| vec![legacy_reminder_time(value, todo.legacy_notify_before)])
        .unwrap_or_default()
}

fn legacy_reminder_time(notify_at: &str, notify_before: Option<i32>) -> String {
    let minutes = notify_before.unwrap_or(0).max(0) as i64;
    if minutes == 0 {
        return notify_at.to_string();
    }

    let normalized = notify_at.replace('T', " ");
    NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S")
        .map(|time| {
            (time - Duration::minutes(minutes))
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| notify_at.to_string())
}

#[tauri::command]
pub fn export_data(db: State<Database>) -> Result<String, String> {
    export_data_internal(&*db, false)
}

#[tauri::command]
pub fn import_data(db: State<Database>, json_data: String) -> Result<(), String> {
    import_data_raw(&*db, &json_data)
}

#[tauri::command]
pub fn export_data_to_file(
    db: State<Database>,
    file_path: String,
    include_executions: bool,
) -> Result<(), String> {
    let json_data = export_data_internal(&*db, include_executions)?;

    let file = std::fs::File::create(&file_path).map_err(|e| format!("创建文件失败: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("data.json", options)
        .map_err(|e| format!("写入 ZIP 失败: {}", e))?;
    zip.write_all(json_data.as_bytes())
        .map_err(|e| format!("写入数据失败: {}", e))?;

    zip.finish().map_err(|e| format!("完成 ZIP 失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn import_data_from_file(db: State<Database>, file_path: String) -> Result<(), String> {
    let file_bytes = std::fs::read(&file_path).map_err(|e| format!("读取文件失败: {}", e))?;

    // ZIP magic bytes: PK (0x50, 0x4B)
    let is_zip = file_bytes.len() >= 2 && file_bytes[0] == 0x50 && file_bytes[1] == 0x4B;

    if is_zip {
        let cursor = std::io::Cursor::new(&file_bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("解析 ZIP 失败: {}", e))?;

        let mut json_data = String::new();
        let mut data_file = archive
            .by_name("data.json")
            .map_err(|e| format!("ZIP 中未找到 data.json: {}", e))?;
        data_file
            .read_to_string(&mut json_data)
            .map_err(|e| format!("读取 data.json 失败: {}", e))?;

        import_data_raw(&*db, &json_data)
    } else {
        let json_data =
            String::from_utf8(file_bytes).map_err(|e| format!("文件编码错误: {}", e))?;
        import_data_raw(&*db, &json_data)
    }
}
