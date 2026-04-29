use crate::db::helpers::{now_timestamp, new_uuid, DbError, FromRow};
use crate::models::{Hook, HookType};
use libsql::Connection;

fn opt_string_to_value(value: &Option<String>) -> libsql::Value {
    match value {
        Some(inner) => libsql::Value::Text(inner.clone()),
        None => libsql::Value::Null,
    }
}

/// Insert a new hook into the database.
/// Generates an id and created_at if they are empty.
pub async fn create(conn: &Connection, hook: &Hook) -> Result<Hook, DbError> {
    let id = if hook.id.is_empty() { new_uuid() } else { hook.id.clone() };
    let now = now_timestamp();
    let created_at = if hook.created_at.is_empty() { now } else { hook.created_at.clone() };

    let timeout_val: libsql::Value = match hook.timeout_secs {
        Some(v) => libsql::Value::Integer(v as i64),
        None => libsql::Value::Null,
    };

    conn.execute(
        "INSERT INTO hooks (id, task_id, hook_type, command, timeout_secs, run_order, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        libsql::params![
            id.clone(),
            opt_string_to_value(&hook.task_id),
            hook.hook_type.to_string(),
            hook.command.clone(),
            timeout_val,
            hook.run_order,
            created_at
        ],
    )
    .await?;

    get_by_id(conn, &id).await
}

/// Get a hook by its id.
pub async fn get_by_id(conn: &Connection, id: &str) -> Result<Hook, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks WHERE id = ?1",
            [id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Hook::from_row(&row),
        None => Err(DbError::NotFound),
    }
}

/// List all hooks for a task, ordered by run_order.
pub async fn list_for_task(conn: &Connection, task_id: &str) -> Result<Vec<Hook>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks WHERE task_id = ?1 ORDER BY run_order",
            [task_id],
        )
        .await?;

    let mut hooks = Vec::new();
    while let Some(row) = rows.next().await? {
        hooks.push(Hook::from_row(&row)?);
    }
    Ok(hooks)
}

/// List all global hooks ordered by run_order.
pub async fn list_global(conn: &Connection) -> Result<Vec<Hook>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks WHERE task_id IS NULL ORDER BY run_order, created_at",
            (),
        )
        .await?;

    let mut hooks = Vec::new();
    while let Some(row) = rows.next().await? {
        hooks.push(Hook::from_row(&row)?);
    }
    Ok(hooks)
}

/// List all hooks across all tasks, ordered by task then run order.
pub async fn list_all(conn: &Connection) -> Result<Vec<Hook>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks
             ORDER BY CASE WHEN task_id IS NULL THEN 0 ELSE 1 END, task_id, run_order, created_at",
            (),
        )
        .await?;

    let mut hooks = Vec::new();
    while let Some(row) = rows.next().await? {
        hooks.push(Hook::from_row(&row)?);
    }
    Ok(hooks)
}

/// Get hooks for a task filtered by hook type, ordered by run_order.
pub async fn get_by_type(
    conn: &Connection,
    task_id: &str,
    hook_type: &HookType,
) -> Result<Vec<Hook>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks WHERE task_id = ?1 AND hook_type = ?2 ORDER BY run_order",
            libsql::params![task_id.to_string(), hook_type.to_string()],
        )
        .await?;

    let mut hooks = Vec::new();
    while let Some(row) = rows.next().await? {
        hooks.push(Hook::from_row(&row)?);
    }
    Ok(hooks)
}

/// Get global hooks filtered by hook type, ordered by run_order.
pub async fn get_global_by_type(
    conn: &Connection,
    hook_type: &HookType,
) -> Result<Vec<Hook>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
             FROM hooks WHERE task_id IS NULL AND hook_type = ?1 ORDER BY run_order",
            libsql::params![hook_type.to_string()],
        )
        .await?;

    let mut hooks = Vec::new();
    while let Some(row) = rows.next().await? {
        hooks.push(Hook::from_row(&row)?);
    }
    Ok(hooks)
}

/// Update an existing hook. Returns the updated hook.
pub async fn update(conn: &Connection, hook: &Hook) -> Result<Hook, DbError> {
    let timeout_val: libsql::Value = match hook.timeout_secs {
        Some(v) => libsql::Value::Integer(v as i64),
        None => libsql::Value::Null,
    };

    let rows_changed = conn
        .execute(
            "UPDATE hooks SET
                task_id = ?2,
                hook_type = ?3,
                command = ?4,
                timeout_secs = ?5,
                run_order = ?6
             WHERE id = ?1",
            libsql::params![
                hook.id.clone(),
                opt_string_to_value(&hook.task_id),
                hook.hook_type.to_string(),
                hook.command.clone(),
                timeout_val,
                hook.run_order
            ],
        )
        .await?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    get_by_id(conn, &hook.id).await
}

/// Delete a hook by id.
pub async fn delete(conn: &Connection, id: &str) -> Result<(), DbError> {
    let rows_changed = conn
        .execute("DELETE FROM hooks WHERE id = ?1", [id])
        .await?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    Ok(())
}
