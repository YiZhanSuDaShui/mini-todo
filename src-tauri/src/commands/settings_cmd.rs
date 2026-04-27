use crate::db::Database;
use chrono::{Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tauri::State;

/// 获取通知类型设置
#[tauri::command]
pub fn get_notification_type(db: State<Database>) -> Result<String, String> {
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
    .map_err(|e| e.to_string())
}

/// 设置通知类型
#[tauri::command]
pub fn set_notification_type(db: State<Database>, notification_type: String) -> Result<(), String> {
    // 验证通知类型
    let valid_type = match notification_type.as_str() {
        "system" | "app" => notification_type,
        _ => "system".to_string(),
    };

    db.with_connection(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('notification_type', ?1, datetime('now', 'localtime'))",
            [&valid_type],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

fn normalize_app_notification_position(value: &str) -> String {
    match value {
        "bottom_left" | "top_right" | "top_left" => value.to_string(),
        _ => "bottom_right".to_string(),
    }
}

/// 获取软件通知位置设置
#[tauri::command]
pub fn get_app_notification_position(db: State<Database>) -> Result<String, String> {
    db.with_connection(|conn| {
        let result: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'app_notification_position'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "bottom_right".to_string());
        Ok(normalize_app_notification_position(&result))
    })
    .map_err(|e| e.to_string())
}

/// 设置软件通知位置
#[tauri::command]
pub fn set_app_notification_position(
    db: State<Database>,
    app_notification_position: String,
) -> Result<(), String> {
    let valid_position = normalize_app_notification_position(&app_notification_position);

    db.with_connection(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('app_notification_position', ?1, datetime('now', 'localtime'))",
            [&valid_position],
        )?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

const DEFAULT_AI_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_AI_MODEL: &str = "deepseek-v4-pro";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub thinking_enabled: bool,
    pub reasoning_effort: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPlanResult {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default)]
    pub reminder_times: Vec<String>,
    pub reason: Option<String>,
}

fn read_setting_value(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .ok()
}

fn write_setting_value(
    conn: &rusqlite::Connection,
    key: &str,
    value: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now', 'localtime'))",
        [key, value],
    )?;
    Ok(())
}

fn normalize_reasoning_effort(value: &str) -> String {
    match value.trim() {
        "high" | "max" => value.trim().to_string(),
        _ => "high".to_string(),
    }
}

fn normalize_ai_settings(settings: AiSettings) -> AiSettings {
    let base_url = if settings.base_url.trim().is_empty() {
        DEFAULT_AI_BASE_URL.to_string()
    } else {
        settings.base_url.trim().trim_end_matches('/').to_string()
    };

    let model = if settings.model.trim().is_empty() {
        DEFAULT_AI_MODEL.to_string()
    } else {
        settings.model.trim().to_string()
    };

    AiSettings {
        base_url,
        api_key: settings.api_key.trim().to_string(),
        model,
        thinking_enabled: settings.thinking_enabled,
        reasoning_effort: normalize_reasoning_effort(&settings.reasoning_effort),
    }
}

fn ai_endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn read_ai_settings(db: &Database) -> Result<AiSettings, String> {
    db.with_connection(|conn| {
        Ok(AiSettings {
            base_url: read_setting_value(conn, "ai_base_url")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_AI_BASE_URL.to_string()),
            api_key: read_setting_value(conn, "ai_api_key").unwrap_or_default(),
            model: read_setting_value(conn, "ai_model")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_AI_MODEL.to_string()),
            thinking_enabled: read_setting_value(conn, "ai_thinking_enabled")
                .map(|v| v == "true")
                .unwrap_or(false),
            reasoning_effort: read_setting_value(conn, "ai_reasoning_effort")
                .map(|v| normalize_reasoning_effort(&v))
                .unwrap_or_else(|| "high".to_string()),
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_ai_settings(db: State<Database>) -> Result<AiSettings, String> {
    read_ai_settings(&db)
}

#[tauri::command]
pub fn save_ai_settings(db: State<Database>, settings: AiSettings) -> Result<(), String> {
    let settings = normalize_ai_settings(settings);
    db.with_connection(|conn| {
        write_setting_value(conn, "ai_base_url", &settings.base_url)?;
        write_setting_value(conn, "ai_api_key", &settings.api_key)?;
        write_setting_value(conn, "ai_model", &settings.model)?;
        write_setting_value(
            conn,
            "ai_thinking_enabled",
            if settings.thinking_enabled {
                "true"
            } else {
                "false"
            },
        )?;
        write_setting_value(conn, "ai_reasoning_effort", &settings.reasoning_effort)?;
        Ok(())
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_ai_models(settings: AiSettings) -> Result<Vec<String>, String> {
    let settings = normalize_ai_settings(settings);
    if settings.api_key.is_empty() {
        return Err("请先填写 API Key".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(ai_endpoint(&settings.base_url, "models"))
        .bearer_auth(&settings.api_key)
        .send()
        .await
        .map_err(|e| format!("读取模型列表失败: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取模型响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("读取模型列表失败: HTTP {} {}", status, body));
    }

    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("解析模型列表失败: {}", e))?;
    let mut models = value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    models.sort();
    models.dedup();

    if models.is_empty() {
        return Err("模型列表为空，请检查 Base URL 是否为兼容接口地址".to_string());
    }

    Ok(models)
}

#[tauri::command]
pub async fn plan_todo_with_ai(
    db: State<'_, Database>,
    title: String,
    description: Option<String>,
    current_start_time: Option<String>,
    current_end_time: Option<String>,
    current_reminder_times: Vec<String>,
) -> Result<AiPlanResult, String> {
    let settings = normalize_ai_settings(read_ai_settings(&db)?);
    if settings.api_key.is_empty() {
        return Err("请先在设置中填写 AI API Key".to_string());
    }
    if settings.model.is_empty() {
        return Err("请先在设置中选择 AI 模型".to_string());
    }

    let title = title.trim().to_string();
    let description = description.unwrap_or_default().trim().to_string();
    if title.is_empty() && description.is_empty() {
        return Err("请先填写标题或描述".to_string());
    }

    let now = Local::now();
    let system_prompt = r#"你是一个中文待办日程规划助手。用户会提供待办标题、描述、当前时间和已有时间字段。请推断合理的时间范围与提醒设置，并输出合法 json。

EXAMPLE INPUT:
当前本地时间：2026-04-27T14:30:00
当前星期：Monday
已有开始时间：无
已有截止时间：无
已有提醒时间：[]
标题：华为ICT全球赛
描述：华为ICT全球赛6月2日举行，提前3天设立日程并提醒

EXAMPLE JSON OUTPUT:
{
  "startTime": "2026-06-02T09:00:00",
  "endTime": "2026-06-02T10:00:00",
  "reminderTimes": ["2026-05-30T09:00:00"],
  "reason": "用户说明活动在 6 月 2 日举行，并要求提前 3 天提醒"
}
规则：
1. 只输出一个 json 对象，不要输出 Markdown，不要输出代码块，不要输出解释性段落。
2. json 字段必须固定为 startTime、endTime、reminderTimes、reason。
3. startTime、endTime 必须是 YYYY-MM-DDTHH:mm:ss 字符串或 null。
4. reminderTimes 必须是字符串数组，每一项格式为 YYYY-MM-DDTHH:mm:ss；没有可靠提醒时间时返回空数组。
5. 日期和时间必须基于用户当前本地时间推断。
6. 如果描述给出明确时间，优先使用描述。
7. startTime 表示事情开始时间，endTime 表示结束或截止时间。
8. reminderTimes 是实际提醒响铃时间，用户说提前 N 天/小时/分钟提醒时，直接把提醒时间往前推。
9. 如果用户需要多次提醒，可以返回多个 reminderTimes，按时间从早到晚排列。
10. 如果用户只说“提醒我”但没有提前量，默认在 startTime 提醒；没有 startTime 但有 endTime 时在 endTime 提醒。
11. 如果无法可靠推断某个时间字段，返回 null 或空数组，不要编造过于具体的时间。
12. endTime 存在时必须晚于 startTime。"#;

    let user_prompt = format!(
        "当前本地时间：{}\n当前星期：{}\n已有开始时间：{}\n已有截止时间：{}\n已有提醒时间：{}\n标题：{}\n描述：{}\n请输出合法 json。",
        now.format("%Y-%m-%dT%H:%M:%S"),
        now.format("%A"),
        current_start_time.unwrap_or_else(|| "无".to_string()),
        current_end_time.unwrap_or_else(|| "无".to_string()),
        serde_json::to_string(&current_reminder_times).unwrap_or_else(|_| "[]".to_string()),
        title,
        if description.is_empty() { "无" } else { &description },
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;

    let retry_prompt = format!(
        "{}\n\n上一次输出无法被解析。请严格只返回一个 json 对象，不能包含任何额外文字。",
        user_prompt
    );

    let mut last_error: Option<String> = None;
    for prompt in [&user_prompt, &retry_prompt] {
        let mut request_body = json!({
            "model": settings.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": prompt }
            ],
            "temperature": 0.1,
            "max_tokens": 1024,
            "response_format": { "type": "json_object" }
        });

        if settings.thinking_enabled {
            request_body["thinking"] = json!({ "type": "enabled" });
            request_body["reasoning_effort"] = json!(settings.reasoning_effort);
        } else if settings.base_url.to_ascii_lowercase().contains("deepseek") {
            request_body["thinking"] = json!({ "type": "disabled" });
        }

        let response = client
            .post(ai_endpoint(&settings.base_url, "chat/completions"))
            .bearer_auth(&settings.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("AI 请求失败: {}", e))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| format!("读取 AI 响应失败: {}", e))?;

        if !status.is_success() {
            return Err(format!("AI 请求失败: HTTP {} {}", status, body));
        }

        let value: Value =
            serde_json::from_str(&body).map_err(|e| format!("解析 AI 响应失败: {}", e))?;

        match parse_plan_response(&value) {
            Ok(plan) => return Ok(plan),
            Err(e) => {
                last_error = Some(format!("{}；响应摘要：{}", e, response_preview(&body)));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "AI 响应不是有效 json".to_string()))
}

fn parse_plan_response(value: &Value) -> Result<AiPlanResult, String> {
    if let Some(arguments) = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|tool_call| tool_call.get("function"))
        .and_then(|function| function.get("arguments"))
        .and_then(|arguments| arguments.as_str())
    {
        return parse_plan_content(arguments);
    }

    let content = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .ok_or_else(|| "AI 响应缺少工具调用和文本内容".to_string())?;

    parse_plan_content(content)
}

fn response_preview(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(240).collect()
}

fn parse_plan_content(content: &str) -> Result<AiPlanResult, String> {
    let trimmed = content.trim();
    let cleaned = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string()
    } else {
        trimmed.to_string()
    };

    let json_text = if cleaned.starts_with('{') {
        cleaned
    } else {
        let start = cleaned
            .find('{')
            .ok_or_else(|| "AI 响应不是 JSON".to_string())?;
        let end = cleaned
            .rfind('}')
            .ok_or_else(|| "AI 响应不是完整 JSON".to_string())?;
        cleaned[start..=end].to_string()
    };

    let value: Value =
        serde_json::from_str(&json_text).map_err(|e| format!("解析 AI 规划 JSON 失败: {}", e))?;

    Ok(AiPlanResult {
        start_time: read_datetime_field(&value, "startTime"),
        end_time: read_datetime_field(&value, "endTime"),
        reminder_times: read_datetime_array_field(&value, "reminderTimes"),
        reason: value
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
    })
}

fn read_datetime_array_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(|v| v.as_array())
        .map(|items| {
            let mut result = Vec::new();
            for item in items {
                if let Some(raw) = item.as_str() {
                    let single = json!({ field: raw });
                    if let Some(normalized) = read_datetime_field(&single, field) {
                        if !result.contains(&normalized) {
                            result.push(normalized);
                        }
                    }
                }
            }
            result.sort();
            result
        })
        .unwrap_or_else(|| {
            read_datetime_field(value, "notifyAt")
                .map(|value| vec![value])
                .unwrap_or_default()
        })
}

fn read_datetime_field(value: &Value, field: &str) -> Option<String> {
    let raw = value.get(field)?.as_str()?.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return None;
    }

    let normalized = raw.replace(' ', "T");
    let with_seconds = if normalized.len() == 16 {
        format!("{}:00", normalized)
    } else {
        normalized
    };

    if NaiveDateTime::parse_from_str(&with_seconds, "%Y-%m-%dT%H:%M:%S").is_ok() {
        Some(with_seconds)
    } else {
        None
    }
}
