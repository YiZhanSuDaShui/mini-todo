use crate::db::Database;
use crate::services::webdav::{RemoteMetadata, WebDavClient};
use chrono::{Local, NaiveDateTime};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rand::{distributions::Alphanumeric, Rng};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use tauri::State;

const REMOTE_DIR: &str = "/mini-todo";
const REMOTE_IMAGES_DIR: &str = "/mini-todo/images";
const SYNC_ARCHIVE_FILE: &str = "/mini-todo/sync-data.json.gz";
const MANIFEST_FILE: &str = "/mini-todo/manifest.json";
const TODOS_FILE: &str = "/mini-todo/todos.json";
const SUBTASKS_FILE: &str = "/mini-todo/subtasks.json";
const REMINDERS_FILE: &str = "/mini-todo/reminders.json";
const SETTINGS_FILE: &str = "/mini-todo/settings.json";
const TOMBSTONES_FILE: &str = "/mini-todo/tombstones.json";
const LOCK_FILE: &str = "/mini-todo/sync.lock";
const LOCK_TTL_SECONDS: i64 = 120;
const SYNC_SCHEMA_VERSION: i32 = 4;
const INCREMENTAL_FINGERPRINT_KEY: &str = "webdav_incremental_manifest_fingerprint";
const ARCHIVE_FINGERPRINT_KEY: &str = "webdav_archive_file_fingerprint";
const LOCAL_DIRTY_KEY: &str = "webdav_local_dirty";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettings {
    pub webdav_url: String,
    pub webdav_username: String,
    pub webdav_password: String,
    pub auto_sync: bool,
    pub sync_interval: i32,
    #[serde(default = "default_sync_mode")]
    pub sync_mode: String,
    #[serde(default)]
    pub startup_sync: bool,
    pub last_sync_at: Option<String>,
    pub device_id: String,
}

fn default_sync_mode() -> String {
    "incremental".to_string()
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            webdav_url: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
            auto_sync: false,
            sync_interval: 15,
            sync_mode: default_sync_mode(),
            startup_sync: false,
            last_sync_at: None,
            device_id: generate_device_id(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub updated_at: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncManifest {
    pub version: String,
    pub schema_version: i32,
    pub updated_at: String,
    pub updated_by_device_id: String,
    pub files: BTreeMap<String, ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTodo {
    pub sync_id: String,
    pub title: String,
    pub description: Option<String>,
    pub color: String,
    pub quadrant: i32,
    pub completed: bool,
    pub sort_order: i32,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by_device_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSubtask {
    pub sync_id: String,
    pub parent_todo_sync_id: String,
    pub title: String,
    pub content: Option<String>,
    pub completed: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by_device_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReminder {
    pub sync_id: String,
    pub todo_sync_id: String,
    pub notify_at: String,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by_device_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SharedSettings {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTombstone {
    pub entity: String,
    pub sync_id: String,
    pub deleted_at: String,
    pub deleted_by_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncData {
    pub version: String,
    pub device_id: String,
    pub updated_at: String,
    pub manifest: SyncManifest,
    pub todos: Vec<SyncTodo>,
    pub subtasks: Vec<SyncSubtask>,
    pub reminders: Vec<SyncReminder>,
    pub settings: SharedSettings,
    pub tombstones: Vec<SyncTombstone>,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDownloadResult {
    pub has_remote: bool,
    pub remote_data: Option<SyncData>,
    pub local_updated_at: Option<String>,
    pub remote_updated_at: Option<String>,
    pub has_conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRunResult {
    pub status: String,
    pub synced_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncLock {
    device_id: String,
    #[serde(default)]
    token: String,
    created_at: i64,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct UploadParts {
    todos_json: String,
    subtasks_json: String,
    reminders_json: String,
    settings_json: String,
    tombstones_json: String,
}

fn generate_device_id() -> String {
    format!("dev_{}", Local::now().timestamp_millis())
}

fn now_db_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn get_images_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mini-todo")
        .join("images")
}

fn get_setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .ok()
}

fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at)
         VALUES (?1, ?2, datetime('now', 'localtime'))",
        [key, value],
    )?;
    Ok(())
}

pub(crate) fn mark_webdav_local_dirty(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    set_setting(conn, LOCAL_DIRTY_KEY, "true")
}

fn clear_webdav_local_dirty(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    set_setting(conn, LOCAL_DIRTY_KEY, "false")
}

fn is_webdav_local_dirty(conn: &rusqlite::Connection) -> bool {
    get_setting(conn, LOCAL_DIRTY_KEY)
        .map(|value| value == "true")
        .unwrap_or(false)
}

fn normalize_sync_interval(value: i32) -> i32 {
    match value {
        1 | 90 | 2 | 5 | 10 | 15 | 30 => value,
        _ => 15,
    }
}

fn parse_time_score(value: &str) -> i64 {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return dt.timestamp();
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    0
}

fn is_time_newer(a: &str, b: &str) -> bool {
    let a_score = parse_time_score(a);
    let b_score = parse_time_score(b);
    if a_score == 0 || b_score == 0 {
        a > b
    } else {
        a_score > b_score
    }
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn json_pretty<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

fn metadata_fingerprint(meta: &Option<RemoteMetadata>) -> Option<String> {
    meta.as_ref().map(|m| {
        format!(
            "{}|{}|{}",
            m.etag.clone().unwrap_or_default(),
            m.last_modified.clone().unwrap_or_default(),
            m.content_length.unwrap_or_default()
        )
    })
}

fn generate_lock_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn get_stored_remote_fingerprint(db: &Database, key: &str) -> Result<Option<String>, String> {
    db.with_connection(|conn| Ok(get_setting(conn, key)))
        .map_err(|e| e.to_string())
}

fn remote_fingerprint_changed(
    db: &Database,
    key: &str,
    meta: &Option<RemoteMetadata>,
) -> Result<bool, String> {
    let stored = get_stored_remote_fingerprint(db, key)?;
    let current = metadata_fingerprint(meta);
    Ok(match (stored, current) {
        (Some(stored), Some(current)) => stored != current,
        _ => true,
    })
}

fn update_sync_state(
    db: &Database,
    synced_at: &str,
    fingerprint_key: &str,
    remote_meta: &Option<RemoteMetadata>,
) -> Result<String, String> {
    let fingerprint = metadata_fingerprint(remote_meta);
    db.with_connection(|conn| {
        set_setting(conn, "webdav_last_sync_at", synced_at)?;
        if let Some(value) = fingerprint {
            set_setting(conn, fingerprint_key, &value)?;
        }
        clear_webdav_local_dirty(conn)?;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(synced_at.to_string())
}

fn ensure_remote_dirs(client: &WebDavClient) -> Result<(), String> {
    client.ensure_dir(REMOTE_DIR)?;
    client.ensure_dir(REMOTE_IMAGES_DIR)?;
    Ok(())
}

fn get_client(settings: &SyncSettings) -> WebDavClient {
    WebDavClient::new(
        &settings.webdav_url,
        &settings.webdav_username,
        &settings.webdav_password,
    )
}

#[tauri::command]
pub fn get_sync_settings(db: State<Database>) -> Result<SyncSettings, String> {
    read_sync_settings(&db)
}

#[tauri::command]
pub fn save_sync_settings(db: State<Database>, settings: SyncSettings) -> Result<(), String> {
    let sync_mode = if settings.sync_mode == "incremental" {
        "incremental"
    } else {
        "archive"
    };
    let sync_interval = normalize_sync_interval(settings.sync_interval);

    db.with_connection(|conn| {
        set_setting(conn, "webdav_url", &settings.webdav_url)?;
        set_setting(conn, "webdav_username", &settings.webdav_username)?;
        set_setting(conn, "webdav_password", &settings.webdav_password)?;
        set_setting(
            conn,
            "webdav_auto_sync",
            if settings.auto_sync { "true" } else { "false" },
        )?;
        set_setting(conn, "webdav_sync_interval", &sync_interval.to_string())?;
        set_setting(conn, "webdav_sync_mode", sync_mode)?;
        set_setting(
            conn,
            "webdav_startup_sync",
            if settings.startup_sync {
                "true"
            } else {
                "false"
            },
        )?;
        if let Some(ref last) = settings.last_sync_at {
            set_setting(conn, "webdav_last_sync_at", last)?;
        }
        set_setting(conn, "webdav_device_id", &settings.device_id)?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn webdav_test_connection(
    url: String,
    username: String,
    password: String,
) -> Result<bool, String> {
    let client = WebDavClient::new(&url, &username, &password);
    client.test_connection()
}

#[tauri::command]
pub fn webdav_sync_now(db: State<Database>) -> Result<SyncRunResult, String> {
    let settings = read_sync_settings(&db)?;
    if settings.webdav_url.trim().is_empty() {
        return Err("未配置 WebDAV 服务器".to_string());
    }

    let sync_result = if settings.sync_mode == "incremental" {
        run_incremental_sync(&db, &settings)?
    } else {
        run_archive_sync(&db, &settings)?
    };

    if sync_result == "no_changes" {
        return Ok(SyncRunResult {
            status: "no_changes".to_string(),
            synced_at: None,
            message: "暂无需要同步的变化".to_string(),
        });
    }

    Ok(SyncRunResult {
        status: "synced".to_string(),
        synced_at: Some(sync_result),
        message: "同步完成".to_string(),
    })
}

#[tauri::command]
pub fn webdav_upload_sync(db: State<Database>) -> Result<String, String> {
    let settings = read_sync_settings(&db)?;
    if settings.webdav_url.trim().is_empty() {
        return Err("未配置 WebDAV 服务器".to_string());
    }

    if settings.sync_mode == "incremental" {
        run_incremental_sync(&db, &settings)
    } else {
        upload_archive_snapshot(&db, &settings)
    }
}

#[tauri::command]
pub fn webdav_download_sync(db: State<Database>) -> Result<SyncDownloadResult, String> {
    let settings = read_sync_settings(&db)?;
    if settings.webdav_url.trim().is_empty() {
        return Err("未配置 WebDAV 服务器".to_string());
    }

    let client = get_client(&settings);
    let remote_data = if settings.sync_mode == "incremental" {
        download_incremental_snapshot(&client)?
            .or_else(|| download_archive_snapshot(&client).ok().flatten())
    } else {
        download_archive_snapshot(&client)?
    };

    let Some(remote_data) = remote_data else {
        return Ok(SyncDownloadResult {
            has_remote: false,
            remote_data: None,
            local_updated_at: settings.last_sync_at.clone(),
            remote_updated_at: None,
            has_conflict: false,
        });
    };

    let has_local_changes = check_local_changes(&db, &settings)?;
    let remote_is_newer = settings
        .last_sync_at
        .as_deref()
        .map(|last| is_time_newer(&remote_data.updated_at, last))
        .unwrap_or(true);

    Ok(SyncDownloadResult {
        has_remote: true,
        remote_updated_at: Some(remote_data.updated_at.clone()),
        local_updated_at: settings.last_sync_at.clone(),
        remote_data: Some(remote_data),
        has_conflict: has_local_changes && remote_is_newer,
    })
}

#[tauri::command]
pub fn webdav_apply_remote(db: State<Database>, sync_data_json: String) -> Result<String, String> {
    let remote_data: SyncData =
        serde_json::from_str(&sync_data_json).map_err(|e| format!("解析数据失败: {}", e))?;
    apply_snapshot_to_local(&db, &remote_data)?;
    download_missing_images(&read_sync_settings(&db)?, &remote_data.images)?;
    update_last_sync_at(&db, &now_db_time())
}

#[tauri::command]
pub fn webdav_auto_sync(db: State<Database>) -> Result<String, String> {
    let settings = read_sync_settings(&db)?;
    if settings.webdav_url.trim().is_empty() || !settings.auto_sync {
        return Err("自动同步未启用".to_string());
    }

    if settings.sync_mode == "incremental" {
        run_incremental_sync(&db, &settings)
    } else {
        run_archive_sync(&db, &settings)
    }
}

fn read_sync_settings(db: &Database) -> Result<SyncSettings, String> {
    db.with_connection(|conn| {
        let mut device_id = get_setting(conn, "webdav_device_id").unwrap_or_default();
        if device_id.trim().is_empty() {
            device_id = generate_device_id();
            set_setting(conn, "webdav_device_id", &device_id)?;
        }

        Ok(SyncSettings {
            webdav_url: get_setting(conn, "webdav_url").unwrap_or_default(),
            webdav_username: get_setting(conn, "webdav_username").unwrap_or_default(),
            webdav_password: get_setting(conn, "webdav_password").unwrap_or_default(),
            auto_sync: get_setting(conn, "webdav_auto_sync")
                .map(|v| v == "true")
                .unwrap_or(false),
            sync_interval: normalize_sync_interval(
                get_setting(conn, "webdav_sync_interval")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(15),
            ),
            sync_mode: get_setting(conn, "webdav_sync_mode")
                .filter(|v| v == "archive" || v == "incremental")
                .unwrap_or_else(default_sync_mode),
            startup_sync: get_setting(conn, "webdav_startup_sync")
                .map(|v| v == "true")
                .unwrap_or(false),
            last_sync_at: get_setting(conn, "webdav_last_sync_at"),
            device_id,
        })
    })
    .map_err(|e| e.to_string())
}

fn ensure_table_sync_ids(
    conn: &rusqlite::Connection,
    table: &str,
    prefix: &str,
    device_id: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM {} WHERE sync_id IS NULL OR sync_id = ''",
        table
    ))?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for id in ids {
        let sync_id = format!("{}:{}:{}", prefix, device_id, id);
        conn.execute(
            &format!("UPDATE {} SET sync_id = ?1 WHERE id = ?2", table),
            params![sync_id, id],
        )?;
    }
    Ok(())
}

fn collect_image_files() -> Vec<String> {
    let images_dir = get_images_dir();
    let mut image_files = Vec::new();
    if images_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(images_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    image_files.push(name.to_string());
                }
            }
        }
    }
    image_files.sort();
    image_files
}

fn collect_local_snapshot(db: &Database, settings: &SyncSettings) -> Result<SyncData, String> {
    let now = now_db_time();
    let result = db.with_connection(|conn| {
        ensure_table_sync_ids(conn, "todos", "todo", &settings.device_id)?;
        ensure_table_sync_ids(conn, "subtasks", "subtask", &settings.device_id)?;
        ensure_table_sync_ids(conn, "todo_reminders", "reminder", &settings.device_id)?;

        let mut todos_stmt = conn.prepare(
            "SELECT sync_id, title, description, color, quadrant, completed, sort_order,
                    start_time, end_time, created_at, updated_at
             FROM todos
             ORDER BY completed ASC, sort_order ASC, created_at DESC",
        )?;
        let todos = todos_stmt
            .query_map([], |row| {
                Ok(SyncTodo {
                    sync_id: row.get(0)?,
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
                    updated_by_device_id: settings.device_id.clone(),
                    revision: 1,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut subtask_stmt = conn.prepare(
            "SELECT s.sync_id, t.sync_id, s.title, s.content, s.completed, s.sort_order,
                    s.created_at, s.updated_at
             FROM subtasks s
             JOIN todos t ON t.id = s.parent_id
             ORDER BY s.parent_id ASC, s.sort_order ASC, s.id ASC",
        )?;
        let subtasks = subtask_stmt
            .query_map([], |row| {
                Ok(SyncSubtask {
                    sync_id: row.get(0)?,
                    parent_todo_sync_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    completed: row.get::<_, i32>(4)? != 0,
                    sort_order: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    updated_by_device_id: settings.device_id.clone(),
                    revision: 1,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut reminder_stmt = conn.prepare(
            "SELECT r.sync_id, t.sync_id, r.notify_at, r.sort_order, r.created_at, r.updated_at
             FROM todo_reminders r
             JOIN todos t ON t.id = r.todo_id
             ORDER BY r.todo_id ASC, r.sort_order ASC, r.notify_at ASC",
        )?;
        let reminders = reminder_stmt
            .query_map([], |row| {
                Ok(SyncReminder {
                    sync_id: row.get(0)?,
                    todo_sync_id: row.get(1)?,
                    notify_at: row.get(2)?,
                    sort_order: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    updated_by_device_id: settings.device_id.clone(),
                    revision: 1,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let shared_settings = SharedSettings::default();

        let mut tombstone_stmt = conn.prepare(
            "SELECT entity, sync_id, deleted_at, deleted_by_device_id
             FROM sync_tombstones
             ORDER BY deleted_at ASC",
        )?;
        let tombstones = tombstone_stmt
            .query_map([], |row| {
                Ok(SyncTombstone {
                    entity: row.get(0)?,
                    sync_id: row.get(1)?,
                    deleted_at: row.get(2)?,
                    deleted_by_device_id: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok::<_, rusqlite::Error>((todos, subtasks, reminders, shared_settings, tombstones))
    });

    let (todos, subtasks, reminders, shared_settings, tombstones) =
        result.map_err(|e| e.to_string())?;
    let images = collect_image_files();

    let mut data = SyncData {
        version: "4.0".to_string(),
        device_id: settings.device_id.clone(),
        updated_at: now.clone(),
        manifest: SyncManifest::default(),
        todos,
        subtasks,
        reminders,
        settings: shared_settings,
        tombstones,
        images,
    };
    data.manifest = build_manifest(&data)?;
    Ok(data)
}

fn build_manifest(data: &SyncData) -> Result<SyncManifest, String> {
    let parts = build_upload_parts(data)?;
    let mut files = BTreeMap::new();
    files.insert(
        "todos.json".to_string(),
        ManifestFile {
            path: "todos.json".to_string(),
            sha256: hash_text(&parts.todos_json),
            updated_at: data.updated_at.clone(),
            count: data.todos.len(),
        },
    );
    files.insert(
        "subtasks.json".to_string(),
        ManifestFile {
            path: "subtasks.json".to_string(),
            sha256: hash_text(&parts.subtasks_json),
            updated_at: data.updated_at.clone(),
            count: data.subtasks.len(),
        },
    );
    files.insert(
        "reminders.json".to_string(),
        ManifestFile {
            path: "reminders.json".to_string(),
            sha256: hash_text(&parts.reminders_json),
            updated_at: data.updated_at.clone(),
            count: data.reminders.len(),
        },
    );
    files.insert(
        "settings.json".to_string(),
        ManifestFile {
            path: "settings.json".to_string(),
            sha256: hash_text(&parts.settings_json),
            updated_at: data.updated_at.clone(),
            count: 1,
        },
    );
    files.insert(
        "tombstones.json".to_string(),
        ManifestFile {
            path: "tombstones.json".to_string(),
            sha256: hash_text(&parts.tombstones_json),
            updated_at: data.updated_at.clone(),
            count: data.tombstones.len(),
        },
    );

    Ok(SyncManifest {
        version: data.version.clone(),
        schema_version: SYNC_SCHEMA_VERSION,
        updated_at: data.updated_at.clone(),
        updated_by_device_id: data.device_id.clone(),
        files,
    })
}

fn build_upload_parts(data: &SyncData) -> Result<UploadParts, String> {
    Ok(UploadParts {
        todos_json: json_pretty(&data.todos)?,
        subtasks_json: json_pretty(&data.subtasks)?,
        reminders_json: json_pretty(&data.reminders)?,
        settings_json: json_pretty(&data.settings)?,
        tombstones_json: json_pretty(&data.tombstones)?,
    })
}

fn merge_by_updated_at<T, F, G>(local: Vec<T>, remote: Vec<T>, key_fn: F, updated_fn: G) -> Vec<T>
where
    T: Clone,
    F: Fn(&T) -> &str,
    G: Fn(&T) -> &str,
{
    let mut map: HashMap<String, T> = HashMap::new();
    for item in local {
        map.insert(key_fn(&item).to_string(), item);
    }
    for item in remote {
        let key = key_fn(&item).to_string();
        let should_replace = map
            .get(&key)
            .map(|existing| is_time_newer(updated_fn(&item), updated_fn(existing)))
            .unwrap_or(true);
        if should_replace {
            map.insert(key, item);
        }
    }
    map.into_values().collect()
}

fn merge_tombstones(local: Vec<SyncTombstone>, remote: Vec<SyncTombstone>) -> Vec<SyncTombstone> {
    let mut map: HashMap<String, SyncTombstone> = HashMap::new();
    for item in local.into_iter().chain(remote) {
        let key = format!("{}:{}", item.entity, item.sync_id);
        let should_replace = map
            .get(&key)
            .map(|existing| is_time_newer(&item.deleted_at, &existing.deleted_at))
            .unwrap_or(true);
        if should_replace {
            map.insert(key, item);
        }
    }
    map.into_values().collect()
}

fn apply_tombstone_filters(data: &mut SyncData) {
    let mut todo_deletes: HashMap<String, String> = HashMap::new();
    let mut subtask_deletes: HashMap<String, String> = HashMap::new();
    let mut reminder_deletes: HashMap<String, String> = HashMap::new();
    for tombstone in &data.tombstones {
        match tombstone.entity.as_str() {
            "todo" => {
                todo_deletes.insert(tombstone.sync_id.clone(), tombstone.deleted_at.clone());
            }
            "subtask" => {
                subtask_deletes.insert(tombstone.sync_id.clone(), tombstone.deleted_at.clone());
            }
            "reminder" => {
                reminder_deletes.insert(tombstone.sync_id.clone(), tombstone.deleted_at.clone());
            }
            _ => {}
        }
    }

    data.todos.retain(|todo| {
        todo_deletes
            .get(&todo.sync_id)
            .map(|deleted_at| is_time_newer(&todo.updated_at, deleted_at))
            .unwrap_or(true)
    });
    let active_todo_ids: HashSet<String> = data.todos.iter().map(|t| t.sync_id.clone()).collect();

    data.subtasks.retain(|subtask| {
        if !active_todo_ids.contains(&subtask.parent_todo_sync_id) {
            return false;
        }
        subtask_deletes
            .get(&subtask.sync_id)
            .map(|deleted_at| is_time_newer(&subtask.updated_at, deleted_at))
            .unwrap_or(true)
    });

    data.reminders.retain(|reminder| {
        if !active_todo_ids.contains(&reminder.todo_sync_id) {
            return false;
        }
        reminder_deletes
            .get(&reminder.sync_id)
            .map(|deleted_at| is_time_newer(&reminder.updated_at, deleted_at))
            .unwrap_or(true)
    });
}

fn merge_snapshots(
    mut local: SyncData,
    remote: Option<SyncData>,
    device_id: &str,
) -> Result<SyncData, String> {
    let Some(remote) = remote else {
        local.updated_at = now_db_time();
        local.device_id = device_id.to_string();
        local.manifest = build_manifest(&local)?;
        return Ok(local);
    };

    let mut merged = SyncData {
        version: "4.0".to_string(),
        device_id: device_id.to_string(),
        updated_at: now_db_time(),
        manifest: SyncManifest::default(),
        todos: merge_by_updated_at(local.todos, remote.todos, |t| &t.sync_id, |t| &t.updated_at),
        subtasks: merge_by_updated_at(
            local.subtasks,
            remote.subtasks,
            |s| &s.sync_id,
            |s| &s.updated_at,
        ),
        reminders: merge_by_updated_at(
            local.reminders,
            remote.reminders,
            |r| &r.sync_id,
            |r| &r.updated_at,
        ),
        settings: SharedSettings::default(),
        tombstones: merge_tombstones(local.tombstones, remote.tombstones),
        images: {
            let mut images = local.images;
            images.extend(remote.images);
            images.sort();
            images.dedup();
            images
        },
    };

    apply_tombstone_filters(&mut merged);
    merged.manifest = build_manifest(&merged)?;
    Ok(merged)
}

fn apply_snapshot_to_local(db: &Database, data: &SyncData) -> Result<(), String> {
    db.with_connection(|conn| {
        for tombstone in &data.tombstones {
            conn.execute(
                "INSERT INTO sync_tombstones (entity, sync_id, deleted_at, deleted_by_device_id)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(entity, sync_id) DO UPDATE SET
                    deleted_at = excluded.deleted_at,
                    deleted_by_device_id = excluded.deleted_by_device_id",
                params![
                    &tombstone.entity,
                    &tombstone.sync_id,
                    &tombstone.deleted_at,
                    &tombstone.deleted_by_device_id
                ],
            )?;

            match tombstone.entity.as_str() {
                "todo" => {
                    conn.execute(
                        "DELETE FROM todo_reminders
                         WHERE todo_id IN (SELECT id FROM todos WHERE sync_id = ?1)",
                        [&tombstone.sync_id],
                    )?;
                    conn.execute("DELETE FROM todos WHERE sync_id = ?1", [&tombstone.sync_id])?;
                }
                "subtask" => {
                    conn.execute("DELETE FROM subtasks WHERE sync_id = ?1", [&tombstone.sync_id])?;
                }
                "reminder" => {
                    conn.execute(
                        "DELETE FROM todo_reminders WHERE sync_id = ?1",
                        [&tombstone.sync_id],
                    )?;
                }
                _ => {}
            }
        }

        for todo in &data.todos {
            let exists: Option<i64> = conn
                .query_row("SELECT id FROM todos WHERE sync_id = ?1", [&todo.sync_id], |row| {
                    row.get(0)
                })
                .optional()?;

            if let Some(id) = exists {
                conn.execute(
                    "UPDATE todos SET
                        title = ?1,
                        description = ?2,
                        color = ?3,
                        quadrant = ?4,
                        completed = ?5,
                        sort_order = ?6,
                        start_time = ?7,
                        end_time = ?8,
                        created_at = ?9,
                        updated_at = ?10
                     WHERE id = ?11",
                    params![
                        &todo.title,
                        &todo.description,
                        &todo.color,
                        todo.quadrant,
                        if todo.completed { 1 } else { 0 },
                        todo.sort_order,
                        &todo.start_time,
                        &todo.end_time,
                        &todo.created_at,
                        &todo.updated_at,
                        id
                    ],
                )?;
            } else {
                conn.execute(
                    "INSERT INTO todos
                        (sync_id, title, description, color, quadrant, completed, sort_order,
                         start_time, end_time, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        &todo.sync_id,
                        &todo.title,
                        &todo.description,
                        &todo.color,
                        todo.quadrant,
                        if todo.completed { 1 } else { 0 },
                        todo.sort_order,
                        &todo.start_time,
                        &todo.end_time,
                        &todo.created_at,
                        &todo.updated_at
                    ],
                )?;
            }
        }

        let todo_id_by_sync = load_todo_id_map(conn)?;

        for subtask in &data.subtasks {
            let Some(parent_id) = todo_id_by_sync.get(&subtask.parent_todo_sync_id) else {
                continue;
            };
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT id FROM subtasks WHERE sync_id = ?1",
                    [&subtask.sync_id],
                    |row| row.get(0),
                )
                .optional()?;

            if let Some(id) = exists {
                conn.execute(
                    "UPDATE subtasks SET
                        parent_id = ?1,
                        title = ?2,
                        content = ?3,
                        completed = ?4,
                        sort_order = ?5,
                        created_at = ?6,
                        updated_at = ?7
                     WHERE id = ?8",
                    params![
                        parent_id,
                        &subtask.title,
                        &subtask.content,
                        if subtask.completed { 1 } else { 0 },
                        subtask.sort_order,
                        &subtask.created_at,
                        &subtask.updated_at,
                        id
                    ],
                )?;
            } else {
                conn.execute(
                    "INSERT INTO subtasks
                        (sync_id, parent_id, title, content, completed, sort_order, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        &subtask.sync_id,
                        parent_id,
                        &subtask.title,
                        &subtask.content,
                        if subtask.completed { 1 } else { 0 },
                        subtask.sort_order,
                        &subtask.created_at,
                        &subtask.updated_at
                    ],
                )?;
            }
        }

        sync_reminders(conn, &todo_id_by_sync, &data.reminders)?;
        write_shared_settings(conn, &data.settings)?;
        Ok::<_, rusqlite::Error>(())
    })
    .map_err(|e| e.to_string())
}

fn load_todo_id_map(conn: &rusqlite::Connection) -> rusqlite::Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT sync_id, id FROM todos WHERE sync_id IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (sync_id, id) = row?;
        map.insert(sync_id, id);
    }
    Ok(map)
}

fn sync_reminders(
    conn: &rusqlite::Connection,
    todo_id_by_sync: &HashMap<String, i64>,
    reminders: &[SyncReminder],
) -> rusqlite::Result<()> {
    let mut desired_by_todo: HashMap<i64, HashSet<String>> = HashMap::new();
    for reminder in reminders {
        if let Some(todo_id) = todo_id_by_sync.get(&reminder.todo_sync_id) {
            desired_by_todo
                .entry(*todo_id)
                .or_default()
                .insert(reminder.sync_id.clone());
        }
    }

    for (todo_id, desired_ids) in &desired_by_todo {
        let mut stmt = conn.prepare(
            "SELECT sync_id FROM todo_reminders
             WHERE todo_id = ?1 AND sync_id IS NOT NULL",
        )?;
        let existing = stmt
            .query_map([todo_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for sync_id in existing {
            if !desired_ids.contains(&sync_id) {
                conn.execute(
                    "DELETE FROM todo_reminders WHERE todo_id = ?1 AND sync_id = ?2",
                    params![todo_id, sync_id],
                )?;
            }
        }
    }

    for reminder in reminders {
        let Some(todo_id) = todo_id_by_sync.get(&reminder.todo_sync_id) else {
            continue;
        };
        let exists: Option<i64> = conn
            .query_row(
                "SELECT id FROM todo_reminders WHERE sync_id = ?1",
                [&reminder.sync_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = exists {
            conn.execute(
                "UPDATE todo_reminders SET
                    todo_id = ?1,
                    notify_at = ?2,
                    sort_order = ?3,
                    created_at = ?4,
                    updated_at = ?5
                 WHERE id = ?6",
                params![
                    todo_id,
                    &reminder.notify_at,
                    reminder.sort_order,
                    &reminder.created_at,
                    &reminder.updated_at,
                    id
                ],
            )?;
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO todo_reminders
                    (sync_id, todo_id, notify_at, notified, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
                params![
                    &reminder.sync_id,
                    todo_id,
                    &reminder.notify_at,
                    reminder.sort_order,
                    &reminder.created_at,
                    &reminder.updated_at
                ],
            )?;
        }
    }
    Ok(())
}

fn write_shared_settings(
    _conn: &rusqlite::Connection,
    _settings: &SharedSettings,
) -> rusqlite::Result<()> {
    Ok(())
}

fn acquire_lock(client: &WebDavClient, device_id: &str) -> Result<SyncLock, String> {
    let now = Local::now().timestamp();
    if let Some(lock_text) = client.download_text(LOCK_FILE)? {
        if let Ok(lock) = serde_json::from_str::<SyncLock>(&lock_text) {
            if lock.device_id != device_id && lock.expires_at > now {
                return Err("云端正在同步，请稍后再试".to_string());
            }
        }
    }

    let lock = SyncLock {
        device_id: device_id.to_string(),
        token: generate_lock_token(),
        created_at: now,
        expires_at: now + LOCK_TTL_SECONDS,
    };
    client.upload_text(LOCK_FILE, &json_pretty(&lock)?)?;

    let confirmed = client
        .download_text(LOCK_FILE)?
        .and_then(|text| serde_json::from_str::<SyncLock>(&text).ok())
        .map(|remote_lock| {
            remote_lock.device_id == lock.device_id && remote_lock.token == lock.token
        })
        .unwrap_or(false);

    if !confirmed {
        return Err("云端同步锁竞争，请稍后再试".to_string());
    }

    Ok(lock)
}

fn release_lock(client: &WebDavClient, lock: &SyncLock) {
    let should_release = client
        .download_text(LOCK_FILE)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<SyncLock>(&text).ok())
        .map(|remote_lock| {
            remote_lock.device_id == lock.device_id && remote_lock.token == lock.token
        })
        .unwrap_or(false);

    if should_release {
        let _ = client.delete(LOCK_FILE);
    }
}

fn run_incremental_sync(db: &Database, settings: &SyncSettings) -> Result<String, String> {
    let client = get_client(settings);
    ensure_remote_dirs(&client)?;
    let initial_meta = client.metadata(MANIFEST_FILE)?;
    let has_local_changes = check_local_changes(db, settings)?;
    let remote_changed =
        remote_fingerprint_changed(db, INCREMENTAL_FINGERPRINT_KEY, &initial_meta)?;

    if !has_local_changes && !remote_changed {
        return Ok("no_changes".to_string());
    }

    if !has_local_changes && initial_meta.is_some() {
        let Some(remote) = download_incremental_snapshot(&client)? else {
            return Ok("no_changes".to_string());
        };
        apply_snapshot_to_local(db, &remote)?;
        download_missing_images(settings, &remote.images)?;
        let latest_meta = client.metadata(MANIFEST_FILE)?;
        return update_sync_state(
            db,
            &remote.updated_at,
            INCREMENTAL_FINGERPRINT_KEY,
            &latest_meta,
        );
    }

    let lock = acquire_lock(&client, &settings.device_id)?;
    let result = run_incremental_sync_locked(db, settings, &client, initial_meta);
    release_lock(&client, &lock);
    result
}

fn run_incremental_sync_locked(
    db: &Database,
    settings: &SyncSettings,
    client: &WebDavClient,
    initial_meta: Option<RemoteMetadata>,
) -> Result<String, String> {
    let mut local = collect_local_snapshot(db, settings)?;
    let remote = download_incremental_snapshot(client)?
        .or_else(|| download_archive_snapshot(client).ok().flatten());
    let mut merged = merge_snapshots(local, remote, &settings.device_id)?;

    let before_upload_meta = client.metadata(MANIFEST_FILE)?;
    if metadata_fingerprint(&initial_meta) != metadata_fingerprint(&before_upload_meta) {
        apply_snapshot_to_local(db, &merged)?;
        local = collect_local_snapshot(db, settings)?;
        let latest_remote = download_incremental_snapshot(client)?;
        merged = merge_snapshots(local, latest_remote, &settings.device_id)?;
    }

    apply_snapshot_to_local(db, &merged)?;
    download_missing_images(settings, &merged.images)?;
    upload_images(client, &merged.images)?;
    upload_incremental_snapshot(client, &merged)?;
    let uploaded_meta = client.metadata(MANIFEST_FILE)?;
    update_sync_state(
        db,
        &merged.updated_at,
        INCREMENTAL_FINGERPRINT_KEY,
        &uploaded_meta,
    )
}

fn run_archive_sync(db: &Database, settings: &SyncSettings) -> Result<String, String> {
    let client = get_client(settings);
    ensure_remote_dirs(&client)?;
    let initial_meta = client.metadata(SYNC_ARCHIVE_FILE)?;
    let has_local_changes = check_local_changes(db, settings)?;
    let remote_changed = remote_fingerprint_changed(db, ARCHIVE_FINGERPRINT_KEY, &initial_meta)?;

    if !has_local_changes && !remote_changed {
        return Ok("no_changes".to_string());
    }

    if !has_local_changes && initial_meta.is_some() {
        let Some(remote) = download_archive_snapshot(&client)? else {
            return Ok("no_changes".to_string());
        };
        apply_snapshot_to_local(db, &remote)?;
        download_missing_images(settings, &remote.images)?;
        let latest_meta = client.metadata(SYNC_ARCHIVE_FILE)?;
        return update_sync_state(
            db,
            &remote.updated_at,
            ARCHIVE_FINGERPRINT_KEY,
            &latest_meta,
        );
    }

    let lock = acquire_lock(&client, &settings.device_id)?;
    let result = (|| {
        let local = collect_local_snapshot(db, settings)?;
        let remote = download_archive_snapshot(&client)?;
        let merged = merge_snapshots(local, remote, &settings.device_id)?;
        apply_snapshot_to_local(db, &merged)?;
        download_missing_images(settings, &merged.images)?;
        upload_images(&client, &merged.images)?;
        upload_archive_data(&client, &merged)?;
        let uploaded_meta = client.metadata(SYNC_ARCHIVE_FILE)?;
        update_sync_state(
            db,
            &merged.updated_at,
            ARCHIVE_FINGERPRINT_KEY,
            &uploaded_meta,
        )
    })();
    release_lock(&client, &lock);
    result
}

fn upload_archive_snapshot(db: &Database, settings: &SyncSettings) -> Result<String, String> {
    let client = get_client(settings);
    ensure_remote_dirs(&client)?;
    let lock = acquire_lock(&client, &settings.device_id)?;
    let result = (|| {
        let data = collect_local_snapshot(db, settings)?;
        upload_images(&client, &data.images)?;
        upload_archive_data(&client, &data)?;
        let uploaded_meta = client.metadata(SYNC_ARCHIVE_FILE)?;
        update_sync_state(
            db,
            &data.updated_at,
            ARCHIVE_FINGERPRINT_KEY,
            &uploaded_meta,
        )
    })();
    release_lock(&client, &lock);
    result
}

fn upload_archive_data(client: &WebDavClient, data: &SyncData) -> Result<(), String> {
    let sync_json = json_pretty(data)?;
    let compressed = gzip_compress(sync_json.as_bytes())?;
    client.upload_bytes(SYNC_ARCHIVE_FILE, &compressed, "application/gzip")
}

fn upload_incremental_snapshot(client: &WebDavClient, data: &SyncData) -> Result<(), String> {
    let mut data = data.clone();
    data.manifest = build_manifest(&data)?;
    let parts = build_upload_parts(&data)?;

    client.upload_text(TODOS_FILE, &parts.todos_json)?;
    client.upload_text(SUBTASKS_FILE, &parts.subtasks_json)?;
    client.upload_text(REMINDERS_FILE, &parts.reminders_json)?;
    client.upload_text(SETTINGS_FILE, &parts.settings_json)?;
    client.upload_text(TOMBSTONES_FILE, &parts.tombstones_json)?;
    client.upload_text(MANIFEST_FILE, &json_pretty(&data.manifest)?)?;
    Ok(())
}

fn download_incremental_snapshot(client: &WebDavClient) -> Result<Option<SyncData>, String> {
    let Some(manifest_text) = client.download_text(MANIFEST_FILE)? else {
        return Ok(None);
    };
    let manifest: SyncManifest = serde_json::from_str(&manifest_text)
        .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

    let todos: Vec<SyncTodo> = download_json_file(client, TODOS_FILE)?.unwrap_or_default();
    let subtasks: Vec<SyncSubtask> = download_json_file(client, SUBTASKS_FILE)?.unwrap_or_default();
    let reminders: Vec<SyncReminder> =
        download_json_file(client, REMINDERS_FILE)?.unwrap_or_default();
    let settings: SharedSettings = download_json_file(client, SETTINGS_FILE)?.unwrap_or_default();
    let tombstones: Vec<SyncTombstone> =
        download_json_file(client, TOMBSTONES_FILE)?.unwrap_or_default();

    let images = client.list_files(REMOTE_IMAGES_DIR).unwrap_or_default();

    Ok(Some(SyncData {
        version: manifest.version.clone(),
        device_id: manifest.updated_by_device_id.clone(),
        updated_at: manifest.updated_at.clone(),
        manifest,
        todos,
        subtasks,
        reminders,
        settings,
        tombstones,
        images,
    }))
}

fn download_json_file<T>(client: &WebDavClient, path: &str) -> Result<Option<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(text) = client.download_text(path)? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("解析 {} 失败: {}", path, e))
}

fn download_archive_snapshot(client: &WebDavClient) -> Result<Option<SyncData>, String> {
    let Some(bytes) = client.download_bytes(SYNC_ARCHIVE_FILE)? else {
        return Ok(None);
    };
    let json_text = gzip_decompress(&bytes)?;
    parse_sync_data_or_legacy(&json_text)
}

fn parse_sync_data_or_legacy(json_text: &str) -> Result<Option<SyncData>, String> {
    if let Ok(data) = serde_json::from_str::<SyncData>(json_text) {
        return Ok(Some(data));
    }

    let value: serde_json::Value =
        serde_json::from_str(json_text).map_err(|e| format!("解析远程数据失败: {}", e))?;
    let Some(todos_value) = value.get("todos").and_then(|v| v.as_array()) else {
        return Ok(None);
    };

    let device_id = value
        .get("deviceId")
        .and_then(|v| v.as_str())
        .unwrap_or("legacy")
        .to_string();
    let fallback_updated_at = now_db_time();
    let updated_at = value
        .get("updatedAt")
        .and_then(|v| v.as_str())
        .unwrap_or(&fallback_updated_at)
        .to_string();

    let mut todos = Vec::new();
    let mut subtasks = Vec::new();
    let mut reminders = Vec::new();

    for todo in todos_value {
        let id = todo.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let todo_sync_id = format!("todo:{}:{}", device_id, id);
        let todo_updated_at = todo
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or(&updated_at)
            .to_string();
        todos.push(SyncTodo {
            sync_id: todo_sync_id.clone(),
            title: todo
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            description: todo
                .get("description")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            color: todo
                .get("color")
                .and_then(|v| v.as_str())
                .unwrap_or("#10B981")
                .to_string(),
            quadrant: todo.get("quadrant").and_then(|v| v.as_i64()).unwrap_or(4) as i32,
            completed: todo
                .get("completed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            sort_order: todo.get("sortOrder").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            start_time: todo
                .get("startTime")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            end_time: todo
                .get("endTime")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            created_at: todo
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or(&todo_updated_at)
                .to_string(),
            updated_at: todo_updated_at.clone(),
            updated_by_device_id: device_id.clone(),
            revision: 1,
        });

        if let Some(items) = todo.get("subtasks").and_then(|v| v.as_array()) {
            for subtask in items {
                let sub_id = subtask.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                subtasks.push(SyncSubtask {
                    sync_id: format!("subtask:{}:{}", device_id, sub_id),
                    parent_todo_sync_id: todo_sync_id.clone(),
                    title: subtask
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    content: subtask
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string()),
                    completed: subtask
                        .get("completed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    sort_order: subtask
                        .get("sortOrder")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0) as i32,
                    created_at: subtask
                        .get("createdAt")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&todo_updated_at)
                        .to_string(),
                    updated_at: subtask
                        .get("updatedAt")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&todo_updated_at)
                        .to_string(),
                    updated_by_device_id: device_id.clone(),
                    revision: 1,
                });
            }
        }

        if let Some(times) = todo.get("reminderTimes").and_then(|v| v.as_array()) {
            for (index, time) in times.iter().filter_map(|v| v.as_str()).enumerate() {
                reminders.push(SyncReminder {
                    sync_id: format!("reminder:{}:{}:{}", device_id, id, index),
                    todo_sync_id: todo_sync_id.clone(),
                    notify_at: time.to_string(),
                    sort_order: index as i32,
                    created_at: todo_updated_at.clone(),
                    updated_at: todo_updated_at.clone(),
                    updated_by_device_id: device_id.clone(),
                    revision: 1,
                });
            }
        }
    }

    let settings = SharedSettings::default();
    let mut data = SyncData {
        version: "4.0".to_string(),
        device_id: device_id.clone(),
        updated_at: updated_at.clone(),
        manifest: SyncManifest::default(),
        todos,
        subtasks,
        reminders,
        settings,
        tombstones: Vec::new(),
        images: value
            .get("images")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };
    data.manifest = build_manifest(&data)?;
    Ok(Some(data))
}

fn upload_images(client: &WebDavClient, image_files: &[String]) -> Result<(), String> {
    let images_dir = get_images_dir();
    for img_name in image_files {
        let local_path = images_dir.join(img_name);
        if local_path.exists() {
            let remote_path = format!("{}/{}", REMOTE_IMAGES_DIR, img_name);
            if !client.exists(&remote_path).unwrap_or(false) {
                client.upload_file(&remote_path, &local_path)?;
            }
        }
    }
    Ok(())
}

fn download_missing_images(settings: &SyncSettings, image_files: &[String]) -> Result<(), String> {
    let client = get_client(settings);
    let images_dir = get_images_dir();
    std::fs::create_dir_all(&images_dir).ok();

    for img_name in image_files {
        let local_path = images_dir.join(img_name);
        if !local_path.exists() {
            let remote_path = format!("{}/{}", REMOTE_IMAGES_DIR, img_name);
            let _ = client.download_file(&remote_path, &local_path);
        }
    }
    Ok(())
}

fn update_last_sync_at(db: &Database, synced_at: &str) -> Result<String, String> {
    db.with_connection(|conn| {
        set_setting(conn, "webdav_last_sync_at", synced_at)?;
        clear_webdav_local_dirty(conn)?;
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(synced_at.to_string())
}

fn check_local_changes(db: &Database, settings: &SyncSettings) -> Result<bool, String> {
    let Some(last_sync) = settings.last_sync_at.as_ref() else {
        return Ok(true);
    };

    db.with_connection(|conn| {
        if is_webdav_local_dirty(conn) {
            return Ok(true);
        }

        let todo_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM todos WHERE updated_at > ?1",
                [last_sync],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let subtask_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE updated_at > ?1",
                [last_sync],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let reminder_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM todo_reminders WHERE updated_at > ?1",
                [last_sync],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let tombstone_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_tombstones WHERE deleted_at > ?1",
                [last_sync],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(todo_count > 0 || subtask_count > 0 || reminder_count > 0 || tombstone_count > 0)
    })
    .map_err(|e| e.to_string())
}

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| format!("压缩失败: {}", e))?;
    encoder.finish().map_err(|e| format!("压缩完成失败: {}", e))
}

fn gzip_decompress(data: &[u8]) -> Result<String, String> {
    let mut decoder = GzDecoder::new(data);
    let mut result = String::new();
    decoder
        .read_to_string(&mut result)
        .map_err(|e| format!("解压失败: {}", e))?;
    Ok(result)
}
