use rusqlite::{Connection, Result, params};

use super::models::{AgentConfig, CreateAgentRequest, UpdateAgentRequest};

pub fn get_all_agents(conn: &Connection) -> Result<Vec<AgentConfig>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, agent_type, cli_path, enabled, created_at, updated_at
         FROM agent_configs
         ORDER BY created_at ASC",
    )?;

    let agents = stmt
        .query_map([], |row| {
            Ok(AgentConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                agent_type: row.get(2)?,
                cli_path: row.get(3)?,
                enabled: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;

    Ok(agents)
}

pub fn get_agent_by_id(conn: &Connection, id: i64) -> Result<AgentConfig> {
    conn.query_row(
        "SELECT id, name, agent_type, cli_path, enabled, created_at, updated_at
         FROM agent_configs WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                agent_type: row.get(2)?,
                cli_path: row.get(3)?,
                enabled: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        },
    )
}

pub fn create_agent(conn: &Connection, req: &CreateAgentRequest) -> Result<i64> {
    conn.execute(
        "INSERT INTO agent_configs (name, agent_type, cli_path) VALUES (?1, ?2, ?3)",
        params![req.name, req.agent_type, req.cli_path],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_agent(conn: &Connection, id: i64, req: &UpdateAgentRequest) -> Result<()> {
    let mut sets = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref name) = req.name {
        sets.push("name = ?");
        values.push(Box::new(name.clone()));
    }
    if let Some(ref agent_type) = req.agent_type {
        sets.push("agent_type = ?");
        values.push(Box::new(agent_type.clone()));
    }
    if let Some(ref cli_path) = req.cli_path {
        sets.push("cli_path = ?");
        values.push(Box::new(cli_path.clone()));
    }
    if let Some(enabled) = req.enabled {
        sets.push("enabled = ?");
        values.push(Box::new(enabled));
    }

    if sets.is_empty() {
        return Ok(());
    }

    sets.push("updated_at = datetime('now', 'localtime')");
    values.push(Box::new(id));

    let sql = format!(
        "UPDATE agent_configs SET {} WHERE id = ?",
        sets.join(", ")
    );
    let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    conn.execute(&sql, params.as_slice())?;

    Ok(())
}

pub fn delete_agent(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM agent_configs WHERE id = ?1", params![id])?;
    Ok(())
}
