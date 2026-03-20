use crate::db::helpers::{new_uuid, now_timestamp, DbError, FromRow};
use crate::models::{HookRun, JobRun};
use libsql::Connection;

/// Convert an Option<i32> to a libsql::Value (Integer or Null).
fn opt_i32_to_value(v: Option<i32>) -> libsql::Value {
    match v {
        Some(val) => libsql::Value::Integer(val as i64),
        None => libsql::Value::Null,
    }
}

/// Convert an Option<i64> to a libsql::Value (Integer or Null).
fn opt_i64_to_value(v: Option<i64>) -> libsql::Value {
    match v {
        Some(val) => libsql::Value::Integer(val),
        None => libsql::Value::Null,
    }
}

/// Convert an Option<String> to a libsql::Value (Text or Null).
fn opt_string_to_value(v: &Option<String>) -> libsql::Value {
    match v {
        Some(s) => libsql::Value::Text(s.clone()),
        None => libsql::Value::Null,
    }
}

/// Insert a new job run.
/// Generates an id and started_at if they are empty.
pub async fn create_job_run(conn: &Connection, run: &JobRun) -> Result<JobRun, DbError> {
    let id = if run.id.is_empty() { new_uuid() } else { run.id.clone() };
    let now = now_timestamp();
    let started_at = if run.started_at.is_empty() { now } else { run.started_at.clone() };

    conn.execute(
        "INSERT INTO job_runs (id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            id.clone(),
            run.task_id.clone(),
            started_at,
            opt_string_to_value(&run.finished_at),
            opt_i32_to_value(run.exit_code),
            run.stdout.clone(),
            run.stderr.clone(),
            run.status.to_string(),
            run.attempt,
            opt_i64_to_value(run.duration_ms)
        ],
    )
    .await?;

    get_job_run_by_id(conn, &id).await
}

/// Get a job run by its id.
pub async fn get_job_run_by_id(conn: &Connection, id: &str) -> Result<JobRun, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
             FROM job_runs WHERE id = ?1",
            [id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => JobRun::from_row(&row),
        None => Err(DbError::NotFound),
    }
}

/// List job runs with optional filters.
/// filter by task_id (if Some), status (if Some), with limit and offset.
pub async fn list_job_runs(
    conn: &Connection,
    task_id: Option<&str>,
    status: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<JobRun>, DbError> {
    let limit_val = limit.unwrap_or(100);
    let offset_val = offset.unwrap_or(0);

    let mut runs = Vec::new();

    match (task_id, status) {
        (Some(tid), Some(st)) => {
            let mut rows = conn
                .query(
                    "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
                     FROM job_runs WHERE task_id = ?1 AND status = ?2 ORDER BY started_at DESC LIMIT ?3 OFFSET ?4",
                    libsql::params![tid.to_string(), st.to_string(), limit_val, offset_val],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                runs.push(JobRun::from_row(&row)?);
            }
        }
        (Some(tid), None) => {
            let mut rows = conn
                .query(
                    "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
                     FROM job_runs WHERE task_id = ?1 ORDER BY started_at DESC LIMIT ?2 OFFSET ?3",
                    libsql::params![tid.to_string(), limit_val, offset_val],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                runs.push(JobRun::from_row(&row)?);
            }
        }
        (None, Some(st)) => {
            let mut rows = conn
                .query(
                    "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
                     FROM job_runs WHERE status = ?1 ORDER BY started_at DESC LIMIT ?2 OFFSET ?3",
                    libsql::params![st.to_string(), limit_val, offset_val],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                runs.push(JobRun::from_row(&row)?);
            }
        }
        (None, None) => {
            let mut rows = conn
                .query(
                    "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
                     FROM job_runs ORDER BY started_at DESC LIMIT ?1 OFFSET ?2",
                    libsql::params![limit_val, offset_val],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                runs.push(JobRun::from_row(&row)?);
            }
        }
    }

    Ok(runs)
}

/// Update an existing job run (status, exit_code, stdout, stderr, finished_at, duration_ms).
pub async fn update_job_run(conn: &Connection, run: &JobRun) -> Result<(), DbError> {
    let rows_changed = conn
        .execute(
            "UPDATE job_runs SET
                status = ?2,
                exit_code = ?3,
                stdout = ?4,
                stderr = ?5,
                finished_at = ?6,
                duration_ms = ?7,
                attempt = ?8
             WHERE id = ?1",
            libsql::params![
                run.id.clone(),
                run.status.to_string(),
                opt_i32_to_value(run.exit_code),
                run.stdout.clone(),
                run.stderr.clone(),
                opt_string_to_value(&run.finished_at),
                opt_i64_to_value(run.duration_ms),
                run.attempt
            ],
        )
        .await?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    Ok(())
}

/// Get all running runs for a given task (for concurrency checks).
#[allow(dead_code)]
pub async fn get_running_runs_for_task(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<JobRun>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms
             FROM job_runs WHERE task_id = ?1 AND status IN ('running', 'retrying') ORDER BY started_at DESC",
            [task_id],
        )
        .await?;

    let mut runs = Vec::new();
    while let Some(row) = rows.next().await? {
        runs.push(JobRun::from_row(&row)?);
    }
    Ok(runs)
}

/// Mark all orphaned runs (status 'running' or 'retrying') as 'crashed'.
/// Returns the number of rows updated.
pub async fn mark_orphaned_runs_crashed(conn: &Connection) -> Result<u64, DbError> {
    let rows_changed = conn
        .execute(
            "UPDATE job_runs SET status = 'crashed', finished_at = ?1
             WHERE status IN ('running', 'retrying')",
            [now_timestamp()],
        )
        .await?;

    Ok(rows_changed)
}

/// Insert a new hook run.
/// Generates an id and started_at if they are empty.
pub async fn create_hook_run(conn: &Connection, run: &HookRun) -> Result<HookRun, DbError> {
    let id = if run.id.is_empty() { new_uuid() } else { run.id.clone() };
    let now = now_timestamp();
    let started_at = if run.started_at.is_empty() { now } else { run.started_at.clone() };

    conn.execute(
        "INSERT INTO hook_runs (id, job_run_id, hook_id, exit_code, stdout, stderr, started_at, finished_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        libsql::params![
            id.clone(),
            run.job_run_id.clone(),
            run.hook_id.clone(),
            opt_i32_to_value(run.exit_code),
            run.stdout.clone(),
            run.stderr.clone(),
            started_at,
            opt_string_to_value(&run.finished_at),
            run.status.to_string()
        ],
    )
    .await?;

    get_hook_run_by_id(conn, &id).await
}

/// Get a hook run by its id.
async fn get_hook_run_by_id(conn: &Connection, id: &str) -> Result<HookRun, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, job_run_id, hook_id, exit_code, stdout, stderr, started_at, finished_at, status
             FROM hook_runs WHERE id = ?1",
            [id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => HookRun::from_row(&row),
        None => Err(DbError::NotFound),
    }
}

/// Update an existing hook run.
pub async fn update_hook_run(conn: &Connection, run: &HookRun) -> Result<(), DbError> {
    let rows_changed = conn
        .execute(
            "UPDATE hook_runs SET
                status = ?2,
                exit_code = ?3,
                stdout = ?4,
                stderr = ?5,
                finished_at = ?6
             WHERE id = ?1",
            libsql::params![
                run.id.clone(),
                run.status.to_string(),
                opt_i32_to_value(run.exit_code),
                run.stdout.clone(),
                run.stderr.clone(),
                opt_string_to_value(&run.finished_at)
            ],
        )
        .await?;

    if rows_changed == 0 {
        return Err(DbError::NotFound);
    }

    Ok(())
}

/// List all hook runs for a given job run.
#[allow(dead_code)]
pub async fn list_hook_runs(conn: &Connection, job_run_id: &str) -> Result<Vec<HookRun>, DbError> {
    let mut rows = conn
        .query(
            "SELECT id, job_run_id, hook_id, exit_code, stdout, stderr, started_at, finished_at, status
             FROM hook_runs WHERE job_run_id = ?1 ORDER BY started_at",
            [job_run_id],
        )
        .await?;

    let mut runs = Vec::new();
    while let Some(row) = rows.next().await? {
        runs.push(HookRun::from_row(&row)?);
    }
    Ok(runs)
}
