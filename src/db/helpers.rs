use thiserror::Error;

use crate::models::task::ConcurrencyPolicy;
use crate::models::{Hook, HookRun, HookRunStatus, HookType, JobRun, JobRunStatus, Task};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database error: {0}")]
    Libsql(#[from] libsql::Error),

    #[error("Not found")]
    NotFound,

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Connection error: {0}")]
    #[allow(dead_code)]
    ConnectionError(String),

    #[error("Query error: {0}")]
    QueryError(String),
}

/// Helper trait for mapping database rows to domain models.
pub trait FromRow: Sized {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError>;
}

impl FromRow for Task {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError> {
        let concurrency_str: String = row.get::<String>(9)?;
        let concurrency_policy: ConcurrencyPolicy = concurrency_str
            .parse()
            .map_err(|e: String| DbError::QueryError(e))?;
        let tags_json = row
            .get::<Option<String>>(12)?
            .unwrap_or_else(|| "[]".to_string());
        let tags = serde_json::from_str::<Vec<String>>(&tags_json)
            .map_err(|e| DbError::QueryError(format!("invalid task tags JSON: {e}")))?;

        Ok(Task {
            id: row.get::<String>(0)?,
            name: row.get::<String>(1)?,
            command: row.get::<String>(2)?,
            schedule: row.get::<String>(3)?,
            description: row.get::<String>(4)?,
            enabled: row.get::<i32>(5)? != 0,
            max_retries: row.get::<i32>(6)?,
            retry_delay_secs: row.get::<i32>(7)?,
            timeout_secs: row.get::<Option<i32>>(8)?,
            concurrency_policy,
            created_at: row.get::<String>(10)?,
            updated_at: row.get::<String>(11)?,
            tags,
        })
    }
}

impl FromRow for Hook {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError> {
        let hook_type_str: String = row.get::<String>(2)?;
        let hook_type: HookType = hook_type_str
            .parse()
            .map_err(|e: String| DbError::QueryError(e))?;

        Ok(Hook {
            id: row.get::<String>(0)?,
            task_id: row.get::<Option<String>>(1)?,
            hook_type,
            command: row.get::<String>(3)?,
            timeout_secs: row.get::<Option<i32>>(4)?,
            run_order: row.get::<i32>(5)?,
            created_at: row.get::<String>(6)?,
        })
    }
}

impl FromRow for JobRun {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError> {
        let status_str: String = row.get::<String>(7)?;
        let status: JobRunStatus = status_str
            .parse()
            .map_err(|e: String| DbError::QueryError(e))?;

        Ok(JobRun {
            id: row.get::<String>(0)?,
            task_id: row.get::<String>(1)?,
            started_at: row.get::<String>(2)?,
            finished_at: row.get::<Option<String>>(3)?,
            exit_code: row.get::<Option<i32>>(4)?,
            stdout: row.get::<String>(5)?,
            stderr: row.get::<String>(6)?,
            status,
            attempt: row.get::<i32>(8)?,
            duration_ms: row.get::<Option<i64>>(9)?,
        })
    }
}

impl FromRow for HookRun {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError> {
        let status_str: String = row.get::<String>(8)?;
        let status: HookRunStatus = status_str
            .parse()
            .map_err(|e: String| DbError::QueryError(e))?;

        Ok(HookRun {
            id: row.get::<String>(0)?,
            job_run_id: row.get::<String>(1)?,
            hook_id: row.get::<String>(2)?,
            exit_code: row.get::<Option<i32>>(3)?,
            stdout: row.get::<String>(4)?,
            stderr: row.get::<String>(5)?,
            started_at: row.get::<String>(6)?,
            finished_at: row.get::<Option<String>>(7)?,
            status,
        })
    }
}

/// Generate a new UUID v4 string.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get the current UTC timestamp as an ISO 8601 string (matching SQLite's datetime format).
pub fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
