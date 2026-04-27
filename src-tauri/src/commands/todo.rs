use crate::commands::sync_cmd::mark_webdav_local_dirty;
use crate::db::{
    load_reminder_times, normalize_reminder_times, replace_reminder_times, subtask_from_row,
    todo_from_row, CreateSubTaskRequest, CreateTodoRequest, Database, SubTask, Todo,
    UpdateSubTaskRequest, UpdateTodoRequest, SUBTASK_COLUMNS, TODO_COLUMNS,
};
use base64::{engine::general_purpose, Engine};
use chrono::Local;
use std::path::{Path, PathBuf};
use tauri::State;

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

fn ensure_row_sync_id(
    conn: &rusqlite::Connection,
    table: &str,
    prefix: &str,
    id: i64,
) -> rusqlite::Result<String> {
    let current: Option<String> = conn
        .query_row(
            &format!("SELECT sync_id FROM {} WHERE id = ?1", table),
            [id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    if let Some(sync_id) = current {
        if !sync_id.trim().is_empty() {
            return Ok(sync_id);
        }
    }

    let device_id = get_or_create_sync_device_id(conn)?;
    let sync_id = format!("{}:{}:{}", prefix, device_id, id);
    conn.execute(
        &format!("UPDATE {} SET sync_id = ?1 WHERE id = ?2", table),
        (&sync_id, id),
    )?;
    Ok(sync_id)
}

fn record_tombstone(
    conn: &rusqlite::Connection,
    entity: &str,
    sync_id: &str,
) -> rusqlite::Result<()> {
    let device_id = get_or_create_sync_device_id(conn)?;
    let deleted_at = now_db_time();
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

fn record_removed_reminder_tombstones(
    conn: &rusqlite::Connection,
    todo_id: i64,
    next_reminder_times: &[String],
) -> rusqlite::Result<()> {
    let next = normalize_reminder_times(next_reminder_times);
    let mut stmt = conn.prepare("SELECT id, notify_at FROM todo_reminders WHERE todo_id = ?1")?;
    let rows = stmt.query_map([todo_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (reminder_id, notify_at) = row?;
        if next.iter().any(|item| item == &notify_at) {
            continue;
        }
        let sync_id = ensure_row_sync_id(conn, "todo_reminders", "reminder", reminder_id)?;
        record_tombstone(conn, "reminder", &sync_id)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_todos(db: State<Database>) -> Result<Vec<Todo>, String> {
    db.with_connection(|conn| {
        // 获取所有待办
        let sql = format!(
            "SELECT {} FROM todos ORDER BY completed ASC, sort_order ASC, created_at DESC",
            TODO_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let todo_iter = stmt.query_map([], |row| todo_from_row(row))?;

        let mut todos: Vec<Todo> = todo_iter.filter_map(|t| t.ok()).collect();

        // 获取每个待办的子任务
        for todo in &mut todos {
            todo.reminder_times = load_reminder_times(conn, todo.id)?;
            let subtask_sql = format!(
                "SELECT {} FROM subtasks WHERE parent_id = ? ORDER BY sort_order ASC",
                SUBTASK_COLUMNS
            );
            let mut subtask_stmt = conn.prepare(&subtask_sql)?;

            let subtask_iter = subtask_stmt.query_map([todo.id], |row| subtask_from_row(row))?;

            todo.subtasks = subtask_iter.filter_map(|s| s.ok()).collect();
        }

        Ok(todos)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_todo(db: State<Database>, data: CreateTodoRequest) -> Result<Todo, String> {
    db.with_connection(|conn| {
        // 获取最大排序值
        let max_order: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM todos WHERE completed = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(-1);

        conn.execute(
            "INSERT INTO todos (title, description, color, quadrant, start_time, end_time, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &data.title,
                &data.description,
                &data.color,
                data.quadrant,
                &data.start_time,
                &data.end_time,
                max_order + 1,
            ),
        )?;

        let id = conn.last_insert_rowid();
        replace_reminder_times(conn, id, &data.reminder_times)?;
        mark_webdav_local_dirty(conn)?;

        let sql = format!("SELECT {} FROM todos WHERE id = ?", TODO_COLUMNS);
        let mut todo = conn.query_row(&sql, [id], |row| todo_from_row(row))?;
        todo.reminder_times = load_reminder_times(conn, id)?;
        Ok(todo)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_todo(db: State<Database>, id: i64, data: UpdateTodoRequest) -> Result<Todo, String> {
    db.with_connection(|conn| {
        let mut updates = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref title) = data.title {
            updates.push("title = ?");
            params.push(Box::new(title.clone()));
        }
        if let Some(ref desc) = data.description {
            updates.push("description = ?");
            params.push(Box::new(desc.clone()));
        }
        if let Some(ref color) = data.color {
            updates.push("color = ?");
            params.push(Box::new(color.clone()));
        }
        if let Some(quadrant) = data.quadrant {
            updates.push("quadrant = ?");
            params.push(Box::new(quadrant));
        }
        let reminder_times_update = if data.clear_reminder_times {
            Some(Vec::new())
        } else {
            data.reminder_times.clone()
        };
        if let Some(completed) = data.completed {
            updates.push("completed = ?");
            params.push(Box::new(if completed { 1 } else { 0 }));
        }
        if let Some(sort_order) = data.sort_order {
            updates.push("sort_order = ?");
            params.push(Box::new(sort_order));
        }
        // 开始时间
        if data.clear_start_time {
            updates.push("start_time = NULL");
        } else if let Some(ref start_time) = data.start_time {
            updates.push("start_time = ?");
            params.push(Box::new(start_time.clone()));
        }
        // 截止时间
        if data.clear_end_time {
            updates.push("end_time = NULL");
        } else if let Some(ref end_time) = data.end_time {
            updates.push("end_time = ?");
            params.push(Box::new(end_time.clone()));
        }
        if updates.is_empty() && reminder_times_update.is_none() {
            return Err(rusqlite::Error::InvalidParameterName(
                "No fields to update".to_string(),
            ));
        }

        updates.push("updated_at = datetime('now', 'localtime')");

        if !updates.is_empty() {
            let sql = format!("UPDATE todos SET {} WHERE id = ?", updates.join(", "));
            params.push(Box::new(id));

            let params_refs: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            conn.execute(&sql, params_refs.as_slice())?;
        }

        if let Some(reminder_times) = reminder_times_update {
            record_removed_reminder_tombstones(conn, id, &reminder_times)?;
            replace_reminder_times(conn, id, &reminder_times)?;
        }
        mark_webdav_local_dirty(conn)?;

        let todo_sql = format!("SELECT {} FROM todos WHERE id = ?", TODO_COLUMNS);
        let mut todo = conn.query_row(&todo_sql, [id], |row| todo_from_row(row))?;
        todo.reminder_times = load_reminder_times(conn, id)?;

        let subtask_sql = format!(
            "SELECT {} FROM subtasks WHERE parent_id = ? ORDER BY sort_order ASC",
            SUBTASK_COLUMNS
        );
        let mut subtask_stmt = conn.prepare(&subtask_sql)?;
        let subtask_iter = subtask_stmt.query_map([id], |row| subtask_from_row(row))?;
        todo.subtasks = subtask_iter.filter_map(|s| s.ok()).collect();

        Ok(todo)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_todo(db: State<Database>, id: i64) -> Result<(), String> {
    db.with_connection(|conn| {
        let sync_id = ensure_row_sync_id(conn, "todos", "todo", id)?;
        record_tombstone(conn, "todo", &sync_id)?;
        conn.execute("DELETE FROM todo_reminders WHERE todo_id = ?", [id])?;
        conn.execute("DELETE FROM todos WHERE id = ?", [id])?;
        mark_webdav_local_dirty(conn)?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reorder_todos(db: State<Database>, ids: Vec<i64>) -> Result<(), String> {
    db.with_connection(|conn| {
        for (index, id) in ids.iter().enumerate() {
            conn.execute(
                "UPDATE todos SET sort_order = ?, updated_at = datetime('now', 'localtime') WHERE id = ?",
                (index as i32, id),
            )?;
        }
        if !ids.is_empty() {
            mark_webdav_local_dirty(conn)?;
        }
        Ok(())
    })
    .map_err(|e| e.to_string())
}

// 子任务操作
#[tauri::command]
pub fn create_subtask(db: State<Database>, data: CreateSubTaskRequest) -> Result<SubTask, String> {
    db.with_connection(|conn| {
        let max_order: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM subtasks WHERE parent_id = ?",
                [data.parent_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);

        conn.execute(
            "INSERT INTO subtasks (parent_id, title, content, sort_order) VALUES (?1, ?2, ?3, ?4)",
            (data.parent_id, &data.title, &data.content, max_order + 1),
        )?;

        let id = conn.last_insert_rowid();
        mark_webdav_local_dirty(conn)?;

        let sql = format!("SELECT {} FROM subtasks WHERE id = ?", SUBTASK_COLUMNS);
        conn.query_row(&sql, [id], |row| subtask_from_row(row))
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_subtask(
    db: State<Database>,
    id: i64,
    data: UpdateSubTaskRequest,
) -> Result<SubTask, String> {
    db.with_connection(|conn| {
        let mut updates = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref title) = data.title {
            updates.push("title = ?");
            params.push(Box::new(title.clone()));
        }
        if let Some(ref content) = data.content {
            updates.push("content = ?");
            params.push(Box::new(content.clone()));
        }
        if let Some(completed) = data.completed {
            updates.push("completed = ?");
            params.push(Box::new(if completed { 1 } else { 0 }));
        }
        if let Some(sort_order) = data.sort_order {
            updates.push("sort_order = ?");
            params.push(Box::new(sort_order));
        }

        if updates.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "No fields to update".to_string(),
            ));
        }

        updates.push("updated_at = datetime('now', 'localtime')");

        let sql = format!("UPDATE subtasks SET {} WHERE id = ?", updates.join(", "));
        params.push(Box::new(id));

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        conn.execute(&sql, params_refs.as_slice())?;

        let sql = format!("SELECT {} FROM subtasks WHERE id = ?", SUBTASK_COLUMNS);
        mark_webdav_local_dirty(conn)?;
        conn.query_row(&sql, [id], |row| subtask_from_row(row))
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_subtask(db: State<Database>, id: i64) -> Result<SubTask, String> {
    db.with_connection(|conn| {
        let sql = format!("SELECT {} FROM subtasks WHERE id = ?", SUBTASK_COLUMNS);
        conn.query_row(&sql, [id], |row| subtask_from_row(row))
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_subtask(db: State<Database>, id: i64) -> Result<(), String> {
    db.with_connection(|conn| {
        let sync_id = ensure_row_sync_id(conn, "subtasks", "subtask", id)?;
        record_tombstone(conn, "subtask", &sync_id)?;
        conn.execute("DELETE FROM subtasks WHERE id = ?", [id])?;
        mark_webdav_local_dirty(conn)?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

fn get_images_dir_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mini-todo")
        .join("images")
}

#[tauri::command]
pub fn get_images_dir() -> Result<String, String> {
    let dir = get_images_dir_path();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    dir.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid path".to_string())
}

#[tauri::command]
pub fn save_subtask_image(image_data: String, file_name: String) -> Result<String, String> {
    use std::io::Write;

    let dir = get_images_dir_path();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let raw = if image_data.contains(',') {
        image_data.splitn(2, ',').nth(1).unwrap_or("").to_string()
    } else {
        image_data
    };
    let bytes = general_purpose::STANDARD
        .decode(&raw)
        .map_err(|e| e.to_string())?;

    let file_path = dir.join(&file_name);
    let mut file = std::fs::File::create(&file_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;

    file_path
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid path".to_string())
}

#[tauri::command]
pub fn import_subtasks_from_paths(
    db: State<Database>,
    parent_id: i64,
    paths: Vec<String>,
) -> Result<Vec<SubTask>, String> {
    let allowed_exts = ["md", "txt"];
    let mut files: Vec<PathBuf> = Vec::new();

    for p in &paths {
        let path = Path::new(p);
        if path.is_dir() {
            collect_files_recursive(path, &allowed_exts, &mut files);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if allowed_exts.contains(&ext.to_lowercase().as_str()) {
                    files.push(path.to_path_buf());
                }
            }
        }
    }

    if files.is_empty() {
        return Err("未找到 .md 或 .txt 文件".to_string());
    }

    files.sort();

    db.with_connection(|conn| {
        let mut max_order: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM subtasks WHERE parent_id = ?",
                [parent_id],
                |row| row.get(0),
            )
            .unwrap_or(-1);

        let mut created = Vec::new();
        for file in &files {
            let title = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_string();

            let content = std::fs::read_to_string(file).unwrap_or_default();
            if title.is_empty() {
                continue;
            }

            max_order += 1;
            conn.execute(
                "INSERT INTO subtasks (parent_id, title, content, sort_order) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![parent_id, &title, &content, max_order],
            )?;

            let id = conn.last_insert_rowid();
            let sql = format!("SELECT {} FROM subtasks WHERE id = ?", SUBTASK_COLUMNS);
            let subtask = conn.query_row(&sql, [id], |row| subtask_from_row(row))?;
            created.push(subtask);
        }

        if !created.is_empty() {
            mark_webdav_local_dirty(conn)?;
        }
        Ok(created)
    })
    .map_err(|e| e.to_string())
}

fn collect_files_recursive(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, exts, out);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if exts.contains(&ext.to_lowercase().as_str()) {
                    out.push(path);
                }
            }
        }
    }
}
