use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;

use super::AppState;
use crate::db;
use crate::db::helpers::{parse_run_ts, parse_since, DbError};
use crate::models::{HookRun, JobRun, JobRunStatus, JobRunSummary};

// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    pub task_id: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// When false, list responses omit stdout/stderr and return summary rows.
    /// Detail endpoints always include captured output.
    pub include_output: Option<bool>,
    /// Optional time-window filter like `24h`, `7d`, `30d`. Drops rows whose
    /// `started_at` is older than `now - since`.
    pub since: Option<String>,
}

/// Cap for the underlying SQL `LIMIT` when a `since` filter is active. Lets us
/// fetch enough recent rows to reach the cutoff on busy installs while still
/// bounding worst-case response size.
const SINCE_FETCH_CAP: i64 = 50_000;

/// Apply the optional `since` cutoff in Rust. Necessary because legacy and new
/// timestamp formats don't sort correctly with raw lex comparison in SQL.
fn apply_since(runs: Vec<JobRun>, since: Option<&str>) -> Vec<JobRun> {
    let Some(spec) = since else { return runs };
    let Some(duration) = parse_since(spec) else {
        return runs;
    };
    let cutoff = chrono::Utc::now() - duration;
    runs.into_iter()
        .filter(|r| match parse_run_ts(&r.started_at) {
            Some(ts) => ts >= cutoff,
            None => false,
        })
        .collect()
}

fn apply_since_summary(runs: Vec<JobRunSummary>, since: Option<&str>) -> Vec<JobRunSummary> {
    let Some(spec) = since else { return runs };
    let Some(duration) = parse_since(spec) else {
        return runs;
    };
    let cutoff = chrono::Utc::now() - duration;
    runs.into_iter()
        .filter(|r| match parse_run_ts(&r.started_at) {
            Some(ts) => ts >= cutoff,
            None => false,
        })
        .collect()
}

/// Choose an effective SQL `LIMIT`: when a `since` filter is in play, fetch up
/// to `SINCE_FETCH_CAP` rows so the cutoff filter has enough data to work with.
fn effective_limit(query_limit: Option<i64>, since: Option<&str>) -> Option<i64> {
    if since.is_some() {
        Some(query_limit.unwrap_or(SINCE_FETCH_CAP).min(SINCE_FETCH_CAP))
    } else {
        query_limit
    }
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub status: JobRunStatus,
    pub attempt: i32,
    pub duration_ms: Option<i64>,
}

impl From<JobRun> for RunResponse {
    fn from(r: JobRun) -> Self {
        RunResponse {
            id: r.id,
            task_id: r.task_id,
            started_at: r.started_at,
            finished_at: r.finished_at,
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            status: r.status,
            attempt: r.attempt,
            duration_ms: r.duration_ms,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunSummaryResponse {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub status: JobRunStatus,
    pub attempt: i32,
    pub duration_ms: Option<i64>,
}

impl From<JobRunSummary> for RunSummaryResponse {
    fn from(r: JobRunSummary) -> Self {
        RunSummaryResponse {
            id: r.id,
            task_id: r.task_id,
            started_at: r.started_at,
            finished_at: r.finished_at,
            exit_code: r.exit_code,
            status: r.status,
            attempt: r.attempt,
            duration_ms: r.duration_ms,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HookRunResponse {
    pub id: String,
    pub job_run_id: String,
    pub hook_id: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: crate::models::HookRunStatus,
}

impl From<HookRun> for HookRunResponse {
    fn from(run: HookRun) -> Self {
        HookRunResponse {
            id: run.id,
            job_run_id: run.job_run_id,
            hook_id: run.hook_id,
            exit_code: run.exit_code,
            stdout: run.stdout,
            stderr: run.stderr,
            started_at: run.started_at,
            finished_at: run.finished_at,
            status: run.status,
        }
    }
}

// --- Handlers ---

/// GET /api/v1/runs
pub async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> impl IntoResponse {
    let conn = match state.db.connect().await {
        Ok(c) => c,
        Err(e) => {
            error!("Database connection error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    };

    let since = query.since.as_deref();
    if query.include_output == Some(false) {
        return match db::runs::list_job_run_summaries(
            &conn,
            query.task_id.as_deref(),
            query.status.as_deref(),
            effective_limit(query.limit, since),
            query.offset,
        )
        .await
        {
            Ok(runs) => {
                let responses: Vec<RunSummaryResponse> = apply_since_summary(runs, since)
                    .into_iter()
                    .map(RunSummaryResponse::from)
                    .collect();
                (StatusCode::OK, Json(json!(responses))).into_response()
            }
            Err(e) => {
                error!("Failed to list run summaries: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Internal server error"})),
                )
                    .into_response()
            }
        };
    }

    match db::runs::list_job_runs(
        &conn,
        query.task_id.as_deref(),
        query.status.as_deref(),
        effective_limit(query.limit, since),
        query.offset,
    )
    .await
    {
        Ok(runs) => {
            let responses: Vec<RunResponse> = apply_since(runs, since)
                .into_iter()
                .map(RunResponse::from)
                .collect();
            (StatusCode::OK, Json(json!(responses))).into_response()
        }
        Err(e) => {
            error!("Failed to list runs: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/runs/:id
pub async fn get_run(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let conn = match state.db.connect().await {
        Ok(c) => c,
        Err(e) => {
            error!("Database connection error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    };

    match db::runs::get_job_run_by_id(&conn, &id).await {
        Ok(run) => (StatusCode::OK, Json(json!(RunResponse::from(run)))).into_response(),
        Err(DbError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Run not found"})),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get run: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/runs/:id/hooks
pub async fn list_hook_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = match state.db.connect().await {
        Ok(c) => c,
        Err(e) => {
            error!("Database connection error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    };

    match db::runs::get_job_run_by_id(&conn, &id).await {
        Ok(_) => {}
        Err(DbError::NotFound) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Run not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!("Failed to get run: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    }

    match db::runs::list_hook_runs(&conn, &id).await {
        Ok(runs) => {
            let responses: Vec<HookRunResponse> =
                runs.into_iter().map(HookRunResponse::from).collect();
            (StatusCode::OK, Json(json!(responses))).into_response()
        }
        Err(e) => {
            error!("Failed to list hook runs: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/tasks/:task_id/runs
pub async fn list_task_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(query): Query<ListRunsQuery>,
) -> impl IntoResponse {
    let conn = match state.db.connect().await {
        Ok(c) => c,
        Err(e) => {
            error!("Database connection error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    };

    // Verify task exists
    if let Err(e) = db::tasks::get_by_id(&conn, &task_id).await {
        return match e {
            DbError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Task not found"})),
            )
                .into_response(),
            other => {
                error!("Database error: {}", other);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Internal server error"})),
                )
                    .into_response()
            }
        };
    }

    let since = query.since.as_deref();
    if query.include_output == Some(false) {
        return match db::runs::list_job_run_summaries(
            &conn,
            Some(&task_id),
            query.status.as_deref(),
            effective_limit(query.limit, since),
            query.offset,
        )
        .await
        {
            Ok(runs) => {
                let responses: Vec<RunSummaryResponse> = apply_since_summary(runs, since)
                    .into_iter()
                    .map(RunSummaryResponse::from)
                    .collect();
                (StatusCode::OK, Json(json!(responses))).into_response()
            }
            Err(e) => {
                error!("Failed to list task run summaries: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Internal server error"})),
                )
                    .into_response()
            }
        };
    }

    match db::runs::list_job_runs(
        &conn,
        Some(&task_id),
        query.status.as_deref(),
        effective_limit(query.limit, since),
        query.offset,
    )
    .await
    {
        Ok(runs) => {
            let responses: Vec<RunResponse> = apply_since(runs, since)
                .into_iter()
                .map(RunResponse::from)
                .collect();
            (StatusCode::OK, Json(json!(responses))).into_response()
        }
        Err(e) => {
            error!("Failed to list task runs: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}
