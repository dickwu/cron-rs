use thiserror::Error;

use crate::models::task::ConcurrencyPolicy;
use crate::models::{
    Hook, HookRun, HookRunStatus, HookType, JobRun, JobRunStatus, JobRunSummary, Task,
};

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
        let lock_key = row.get::<Option<String>>(13)?;
        let sandbox_profile = row.get::<Option<String>>(14)?;

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
            lock_key,
            sandbox_profile,
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

impl FromRow for JobRunSummary {
    fn from_row(row: &libsql::Row) -> Result<Self, DbError> {
        let status_str: String = row.get::<String>(5)?;
        let status: JobRunStatus = status_str
            .parse()
            .map_err(|e: String| DbError::QueryError(e))?;

        Ok(JobRunSummary {
            id: row.get::<String>(0)?,
            task_id: row.get::<String>(1)?,
            started_at: row.get::<String>(2)?,
            finished_at: row.get::<Option<String>>(3)?,
            exit_code: row.get::<Option<i32>>(4)?,
            status,
            attempt: row.get::<i32>(6)?,
            duration_ms: row.get::<Option<i64>>(7)?,
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

/// Current UTC timestamp as an RFC 3339 string with explicit `Z` suffix.
///
/// The `Z` makes the value self-describing so any consumer (JS `new Date(...)`,
/// Python `datetime.fromisoformat`, `chrono::DateTime::parse_from_rfc3339`) parses
/// it as UTC instead of guessing the local zone. Stored alongside legacy rows
/// that lack a timezone marker — both sort correctly because the `T` separator
/// (0x54) is greater than the space separator (0x20).
pub fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Parse a stored `started_at` / `finished_at` value as a UTC instant.
///
/// Accepts both the current RFC 3339 form (`2026-05-12T03:51:13Z`) and the
/// legacy naïve form (`2026-05-12 03:51:13`) written by older binaries before
/// the Z-suffix change. Both encode UTC; the legacy form just lacks an
/// explicit zone marker, so we attach UTC manually.
pub fn parse_run_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|nd| chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(nd, chrono::Utc))
}

/// Parse a "since" query string like `24h`, `7d`, `30m`, `2w` into a Duration.
///
/// Bare integers are treated as seconds. Returns `None` for empty / malformed
/// input so the caller can decide whether to error or fall through to no-filter.
pub fn parse_since(s: &str) -> Option<chrono::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, unit) = match s.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&s[..s.len() - 1], Some(c.to_ascii_lowercase())),
        _ => (s, None),
    };
    let n: i64 = num_part.parse().ok()?;
    if n < 0 {
        return None;
    }
    match unit {
        None | Some('s') => Some(chrono::Duration::seconds(n)),
        Some('m') => Some(chrono::Duration::minutes(n)),
        Some('h') => Some(chrono::Duration::hours(n)),
        Some('d') => Some(chrono::Duration::days(n)),
        Some('w') => Some(chrono::Duration::weeks(n)),
        _ => None,
    }
}
