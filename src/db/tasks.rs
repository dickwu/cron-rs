use crate::db::helpers::{now_timestamp, new_uuid, DbError, FromRow};
use crate::models::Task;
use libsql::Connection;

/// Insert a new task into the database.
/// Generates an id, created_at, and updated_at if they are empty.
pub async fn create(conn: &Connection, task: &Task) -> Result<Task, DbError> {
    let id = if task.id.is_empty() { new_uuid() } else { task.id.clone() };
    let now = now_timestamp();
    let created_at = if task.created_at.is_empty() { now.clone() } else { task.created_at.clone() };
    let updated_at = if task.updated_at.is_empty() { now } else { task.updated_at.clone() };

    let timeout_val: libsql::Value = match task.timeout_secs {
        Some(v) => libsql::Value::Integer(v as i64),
        None => libsql::Value::Null,
    };

    conn.execute(
        "INSERT INTO tasks (id, name, command, schedule, description, enabled, max_retries, retry_delay_secs, timeout_secs, concurrency_policy, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        libsql::params![
            id.clone(),
            task.name.clone(),
            task.command.clone(),
            task.schedule.clone(),
            task.description.clone(),
            task.enabled as i32,
            task.max_retries,
            task.retry_delay_secs,
            timeout_val,
            task.concurrency_policy.to_string(),
            created_at,
            updated_at
        ],
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE constraint failed") {
            DbError::Conflict(format!("Task with name '{}' already exists", task.name))
        } else {
            DbError::Libsql(e)
        }
    })?;

    get_by_id(conn, &id).await
}

/// Get a task by its id.
pub async fn get_by_id(conn: &Connection, id: &str) -> Result<Task, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, name, command, schedule, description, enabled, max_retries, retry_delay_secs, timeout_secs, concurrency_policy, created_at, updated_at
             FROM tasks WHERE id = ?1",
            [id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Task::from_row(&row),
        None => Err(DbError::NotFound),
    }
}

/// Get a task by its unique name.
pub async fn get_by_name(conn: &Connection, name: &str) -> Result<Task, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, name, command, schedule, description, enabled, max_retries, retry_delay_secs, timeout_secs, concurrency_policy, created_at, updated_at
             FROM tasks WHERE name = ?1",
            [name],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Task::from_row(&row),
        None => Err(DbError::NotFound),
    }
}

/// List all tasks ordered by name.
pub async fn list(conn: &Connection) -> Result<Vec<Task>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, name, command, schedule, description, enabled, max_retries, retry_delay_secs, timeout_secs, concurrency_policy, created_at, updated_at
             FROM tasks ORDER BY name",
            (),
        )
        .await?;

    let mut tasks = Vec::new();
    while let Some(row) = rows.next().await? {
        tasks.push(Task::from_row(&row)?);
    }
    Ok(tasks)
}

/// Update an existing task. Updates all fields and sets updated_at to current time.
pub async fn update(conn: &Connection, task: &Task) -> Result<Task, DbError> {
    let now = now_timestamp();

    let timeout_val: libsql::Value = match task.timeout_secs {
        Some(v) => libsql::Value::Integer(v as i64),
        None => libsql::Value::Null,
    };

    let rows_changed = conn
        .execute(
            "UPDATE tasks SET
                name = ?2,
                command = ?3,
                schedule = ?4,
                description = ?5,
                enabled = ?6,
                max_retries = ?7,
                retry_delay_secs = ?8,
                timeout_secs = ?9,
                concurrency_policy = ?10,
                updated_at = ?11
             WHERE id = ?1",
            libsql::params![
                task.id.clone(),
                task.name.clone(),
                task.command.clone(),
                task.schedule.clone(),
                task.description.clone(),
                task.enabled as i32,
                task.max_retries,
                task.retry_delay_secs,
                timeout_val,
                task.concurrency_policy.to_string(),
                now
            ],
        )
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                DbError::Conflict(format!("Task with name '{}' already exists", task.name))
            } else {
                DbError::Libsql(e)
            }
        })?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    get_by_id(conn, &task.id).await
}

/// Delete a task by id. CASCADE will handle associated hooks and runs.
pub async fn delete(conn: &Connection, id: &str) -> Result<(), DbError> {
    // Enable foreign keys so CASCADE works. Use query since PRAGMAs may return rows.
    let mut pragma_rows = conn.query("PRAGMA foreign_keys = ON", ()).await?;
    let _ = pragma_rows.next().await;

    let rows_changed = conn
        .execute("DELETE FROM tasks WHERE id = ?1", [id])
        .await?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    Ok(())
}
