use rusqlite::{Connection, Result, params};
use std::collections::HashSet;

use super::models::TaskDependency;

pub fn add_dependency(
    conn: &Connection,
    subtask_id: i64,
    depends_on_id: i64,
    dependency_type: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO task_dependencies (subtask_id, depends_on_id, dependency_type)
         VALUES (?1, ?2, ?3)",
        params![subtask_id, depends_on_id, dependency_type],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn remove_dependency(
    conn: &Connection,
    dependency_id: i64,
) -> Result<()> {
    conn.execute(
        "DELETE FROM task_dependencies WHERE id = ?1",
        params![dependency_id],
    )?;
    Ok(())
}

pub fn get_dependencies(
    conn: &Connection,
    subtask_id: i64,
) -> Result<Vec<TaskDependency>> {
    let mut stmt = conn.prepare(
        "SELECT id, subtask_id, depends_on_id, dependency_type, created_at
         FROM task_dependencies
         WHERE subtask_id = ?1",
    )?;

    let rows = stmt.query_map(params![subtask_id], |row| {
        Ok(TaskDependency {
            id: row.get(0)?,
            subtask_id: row.get(1)?,
            depends_on_id: row.get(2)?,
            dependency_type: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;

    rows.collect()
}

pub fn get_dependents(
    conn: &Connection,
    depends_on_id: i64,
) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT subtask_id FROM task_dependencies WHERE depends_on_id = ?1",
    )?;

    let rows = stmt.query_map(params![depends_on_id], |row| {
        row.get(0)
    })?;

    rows.collect()
}

/// DFS 检测添加依赖后是否形成环
pub fn has_circular_dependency(
    conn: &Connection,
    subtask_id: i64,
    depends_on_id: i64,
) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut stack = vec![depends_on_id];

    while let Some(current) = stack.pop() {
        if current == subtask_id {
            return Ok(true);
        }
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current);

        let deps = get_dependencies(conn, current)?;
        for dep in deps {
            stack.push(dep.depends_on_id);
        }
    }
    Ok(false)
}

/// 检查子任务的所有 finish-to-start 依赖是否满足
pub fn are_dependencies_met(
    conn: &Connection,
    subtask_id: i64,
) -> Result<bool> {
    let deps = get_dependencies(conn, subtask_id)?;

    for dep in deps {
        match dep.dependency_type.as_str() {
            "finish-to-start" => {
                let status: String = conn.query_row(
                    "SELECT schedule_status FROM subtasks WHERE id = ?1",
                    params![dep.depends_on_id],
                    |row| row.get(0),
                )?;
                if status.as_str() != "completed" {
                    return Ok(false);
                }
            }
            "start-to-start" => {
                let status: String = conn.query_row(
                    "SELECT schedule_status FROM subtasks WHERE id = ?1",
                    params![dep.depends_on_id],
                    |row| row.get(0),
                )?;
                if !matches!(status.as_str(), "running" | "completed") {
                    return Ok(false);
                }
            }
            _ => {}
        }
    }
    Ok(true)
}
