use tauri::State;

use crate::db::agent_db;
use crate::db::models::{AgentConfig, AgentHealthStatus, CreateAgentRequest, UpdateAgentRequest};
use crate::db::Database;
use crate::services::agent::{encrypt_api_key, AgentManager};
use crate::services::agent::runner::ExecutionState;

#[tauri::command]
pub fn get_agents(db: State<'_, Database>) -> Result<Vec<AgentConfig>, String> {
    db.with_connection(|conn| agent_db::get_all_agents(conn))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_agent(db: State<'_, Database>, id: i64) -> Result<AgentConfig, String> {
    db.with_connection(|conn| agent_db::get_agent_by_id(conn, id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_agent(
    db: State<'_, Database>,
    request: CreateAgentRequest,
) -> Result<i64, String> {
    let encrypted_key = if let Some(ref key) = request.api_key {
        encrypt_api_key(key)?
    } else {
        String::new()
    };

    db.with_connection(|conn| agent_db::create_agent(conn, &request, &encrypted_key))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_agent(
    db: State<'_, Database>,
    id: i64,
    request: UpdateAgentRequest,
) -> Result<(), String> {
    let encrypted_key = match &request.api_key {
        Some(key) if !key.is_empty() => Some(encrypt_api_key(key)?),
        Some(_) => Some(String::new()),
        None => None,
    };

    db.with_connection(|conn| agent_db::update_agent(conn, id, &request, encrypted_key.as_deref()))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_agent(db: State<'_, Database>, id: i64) -> Result<(), String> {
    db.with_connection(|conn| agent_db::delete_agent(conn, id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_agent_health(
    db: State<'_, Database>,
    agent_manager: State<'_, AgentManager>,
    id: i64,
) -> Result<AgentHealthStatus, String> {
    let config = db
        .with_connection(|conn| agent_db::get_agent_by_id(conn, id))
        .map_err(|e| e.to_string())?;

    Ok(agent_manager.check_health(&config).await)
}

#[tauri::command]
pub async fn check_all_agents_health(
    db: State<'_, Database>,
    agent_manager: State<'_, AgentManager>,
) -> Result<Vec<AgentHealthStatus>, String> {
    let agents = db
        .with_connection(|conn| agent_db::get_all_agents(conn))
        .map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    for agent in &agents {
        results.push(agent_manager.check_health(agent).await);
    }
    Ok(results)
}

#[tauri::command]
pub async fn start_agent_execution(
    app: tauri::AppHandle,
    db: State<'_, Database>,
    agent_manager: State<'_, AgentManager>,
    agent_id: i64,
    prompt: String,
    project_path: String,
    task_id: String,
) -> Result<(), String> {
    let config = db
        .with_connection(|conn| agent_db::get_agent_by_id(conn, agent_id))
        .map_err(|e| e.to_string())?;

    if !config.enabled {
        return Err("Agent 已禁用".to_string());
    }

    agent_manager
        .start_background_execution(config, prompt, project_path, task_id, app)
        .await
}

#[tauri::command]
pub async fn get_agent_execution_state(
    agent_manager: State<'_, AgentManager>,
    task_id: String,
) -> Result<Option<ExecutionState>, String> {
    Ok(agent_manager.get_execution_state(&task_id).await)
}

#[tauri::command]
pub async fn cancel_agent_execution(
    agent_manager: State<'_, AgentManager>,
    task_id: String,
) -> Result<(), String> {
    agent_manager.cancel_execution(&task_id).await
}
