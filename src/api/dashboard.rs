use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{Datelike, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;

use super::AppState;
use crate::db;
use crate::db::helpers::{parse_run_ts, parse_since};
use crate::models::{JobRunStatus, JobRunSummary};

const SUMMARY_TTL: Duration = Duration::from_secs(5);
const RECENT_RUNS_TTL: Duration = Duration::from_secs(3);
const ACTIVITY_TTL: Duration = Duration::from_secs(10);
const SUMMARY_FETCH_LIMIT: i64 = 5_000;
const ACTIVITY_FETCH_LIMIT: i64 = 50_000;
const RECENT_RUNS_DEFAULT_LIMIT: i64 = 20;
const RECENT_RUNS_MAX_LIMIT: i64 = 100;

#[derive(Default)]
pub struct DashboardCache {
    summary: Option<Cached<DashboardSummary>>,
    recent_runs: HashMap<i64, Cached<Vec<DashboardRunSummary>>>,
    activity: HashMap<String, Cached<DashboardActivity>>,
}

struct Cached<T> {
    expires_at: Instant,
    value: T,
}

impl<T: Clone> Cached<T> {
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            expires_at: Instant::now() + ttl,
            value,
        }
    }

    fn get(&self) -> Option<T> {
        (Instant::now() < self.expires_at).then(|| self.value.clone())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardSummary {
    pub task_count: usize,
    pub active_timers: u64,
    pub runs_24h: u64,
    pub success_rate: f64,
    pub recent_failures_24h: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardRunSummary {
    pub id: String,
    pub task_id: String,
    pub task_name: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub status: JobRunStatus,
    pub attempt: i32,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityBucket {
    Success,
    Failed,
    Skipped,
    Running,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardCounts {
    pub success: u64,
    pub failed: u64,
    pub skipped: u64,
    pub running: u64,
}

impl DashboardCounts {
    fn add(&mut self, bucket: ActivityBucket) {
        match bucket {
            ActivityBucket::Success => self.success += 1,
            ActivityBucket::Failed => self.failed += 1,
            ActivityBucket::Skipped => self.skipped += 1,
            ActivityBucket::Running => self.running += 1,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardBucket {
    pub label: String,
    pub bucket_start: String,
    pub counts: DashboardCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardTaskBreakdown {
    pub task_id: String,
    pub task_name: Option<String>,
    pub total: u64,
    pub counts: DashboardCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardActivity {
    pub range: String,
    pub buckets: Vec<DashboardBucket>,
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub skipped: u64,
    pub running: u64,
    pub success_rate: Option<f64>,
    pub top_tasks: Vec<DashboardTaskBreakdown>,
    pub failed_runs: Vec<DashboardRunSummary>,
}

#[derive(Debug, Deserialize)]
pub struct RecentRunsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub range: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum DashboardRange {
    H24,
    D7,
    D30,
}

impl DashboardRange {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "24h" => Some(Self::H24),
            "7d" => Some(Self::D7),
            "30d" => Some(Self::D30),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::H24 => "24h",
            Self::D7 => "7d",
            Self::D30 => "30d",
        }
    }

    fn slots(self) -> i64 {
        match self {
            Self::H24 => 24,
            Self::D7 => 7,
            Self::D30 => 30,
        }
    }

    fn since(self) -> &'static str {
        self.key()
    }

    fn is_hourly(self) -> bool {
        matches!(self, Self::H24)
    }
}

/// GET /api/v1/dashboard/summary
pub async fn summary(State(state): State<AppState>) -> impl IntoResponse {
    match summary_data(&state).await {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(e) => internal_error("Failed to load dashboard summary", e),
    }
}

/// GET /api/v1/dashboard/runs
pub async fn recent_runs(
    State(state): State<AppState>,
    Query(query): Query<RecentRunsQuery>,
) -> impl IntoResponse {
    let limit = query
        .limit
        .unwrap_or(RECENT_RUNS_DEFAULT_LIMIT)
        .clamp(1, RECENT_RUNS_MAX_LIMIT);

    if let Some(value) = state
        .dashboard_cache
        .read()
        .await
        .recent_runs
        .get(&limit)
        .and_then(Cached::get)
    {
        return (StatusCode::OK, Json(value)).into_response();
    }

    match load_recent_runs(&state, limit).await {
        Ok(runs) => {
            state
                .dashboard_cache
                .write()
                .await
                .recent_runs
                .insert(limit, Cached::new(runs.clone(), RECENT_RUNS_TTL));
            (StatusCode::OK, Json(runs)).into_response()
        }
        Err(e) => internal_error("Failed to load dashboard runs", e),
    }
}

/// GET /api/v1/dashboard/activity
pub async fn activity(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> impl IntoResponse {
    let requested = query.range.as_deref().unwrap_or("7d");
    let Some(range) = DashboardRange::parse(requested) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "range must be one of 24h, 7d, or 30d"})),
        )
            .into_response();
    };

    if let Some(value) = state
        .dashboard_cache
        .read()
        .await
        .activity
        .get(range.key())
        .and_then(Cached::get)
    {
        return (StatusCode::OK, Json(value)).into_response();
    }

    match load_activity(&state, range).await {
        Ok(activity) => {
            state.dashboard_cache.write().await.activity.insert(
                range.key().to_string(),
                Cached::new(activity.clone(), ACTIVITY_TTL),
            );
            (StatusCode::OK, Json(activity)).into_response()
        }
        Err(e) => internal_error("Failed to load dashboard activity", e),
    }
}

pub async fn summary_data(state: &AppState) -> anyhow::Result<DashboardSummary> {
    if let Some(summary) = state
        .dashboard_cache
        .read()
        .await
        .summary
        .as_ref()
        .and_then(Cached::get)
    {
        return Ok(summary);
    }

    let summary = load_summary(state).await?;
    state.dashboard_cache.write().await.summary = Some(Cached::new(summary.clone(), SUMMARY_TTL));
    Ok(summary)
}

fn internal_error(message: &str, error: anyhow::Error) -> axum::response::Response {
    error!("{}: {}", message, error);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Internal server error"})),
    )
        .into_response()
}

async fn load_summary(state: &AppState) -> anyhow::Result<DashboardSummary> {
    let conn = state.db.connect().await.context("connect database")?;
    let tasks = db::tasks::list(&conn).await.context("list tasks")?;
    let mut active_timers = 0u64;

    for task in tasks.iter().filter(|task| task.enabled) {
        if let Ok(true) = state.systemd.is_timer_active(&task.name).await {
            active_timers += 1;
        }
    }

    let cutoff = Utc::now() - chrono::Duration::hours(24);
    let runs =
        db::runs::list_job_run_summaries(&conn, None, None, Some(SUMMARY_FETCH_LIMIT), Some(0))
            .await
            .context("list run summaries")?;

    let mut runs_24h = 0u64;
    let mut success_24h = 0u64;
    let mut failed_24h = 0u64;

    for run in runs {
        let Some(ts) = parse_run_ts(&run.started_at) else {
            continue;
        };
        if ts < cutoff {
            continue;
        }

        runs_24h += 1;
        match run.status {
            JobRunStatus::Success => success_24h += 1,
            JobRunStatus::Failed | JobRunStatus::Timeout | JobRunStatus::Crashed => failed_24h += 1,
            _ => {}
        }
    }

    let finished_24h = success_24h + failed_24h;
    let success_rate = if finished_24h > 0 {
        (success_24h as f64 / finished_24h as f64) * 100.0
    } else {
        100.0
    };

    Ok(DashboardSummary {
        task_count: tasks.len(),
        active_timers,
        runs_24h,
        success_rate,
        recent_failures_24h: failed_24h,
    })
}

async fn load_recent_runs(
    state: &AppState,
    limit: i64,
) -> anyhow::Result<Vec<DashboardRunSummary>> {
    let conn = state.db.connect().await.context("connect database")?;
    let task_names = task_names(&conn).await?;
    let runs = db::runs::list_job_run_summaries(&conn, None, None, Some(limit), Some(0))
        .await
        .context("list recent run summaries")?;

    Ok(runs
        .into_iter()
        .map(|run| with_task_name(run, &task_names))
        .collect())
}

async fn load_activity(
    state: &AppState,
    range: DashboardRange,
) -> anyhow::Result<DashboardActivity> {
    let conn = state.db.connect().await.context("connect database")?;
    let task_names = task_names(&conn).await?;
    let duration = parse_since(range.since()).context("parse dashboard range")?;
    let cutoff = Utc::now() - duration;
    let runs =
        db::runs::list_job_run_summaries(&conn, None, None, Some(ACTIVITY_FETCH_LIMIT), Some(0))
            .await
            .context("list activity run summaries")?;

    let mut buckets = empty_buckets(range);
    let bucket_index: HashMap<String, usize> = buckets
        .iter()
        .enumerate()
        .map(|(idx, bucket)| (bucket.bucket_start.clone(), idx))
        .collect();

    let mut total = 0u64;
    let mut success = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut running = 0u64;
    let mut failed_runs = Vec::new();
    let mut per_task: HashMap<String, DashboardTaskBreakdown> = HashMap::new();

    for run in runs {
        let Some(ts) = parse_run_ts(&run.started_at) else {
            continue;
        };
        if ts < cutoff {
            continue;
        }
        let Some(bucket) = classify(&run.status) else {
            continue;
        };

        if let Some(idx) = bucket_index.get(&bucket_key(ts, range)) {
            buckets[*idx].counts.add(bucket);
        }

        total += 1;
        match bucket {
            ActivityBucket::Success => success += 1,
            ActivityBucket::Failed => {
                failed += 1;
                failed_runs.push(with_task_name(run.clone(), &task_names));
            }
            ActivityBucket::Skipped => skipped += 1,
            ActivityBucket::Running => running += 1,
        }

        let entry = per_task
            .entry(run.task_id.clone())
            .or_insert_with(|| DashboardTaskBreakdown {
                task_id: run.task_id.clone(),
                task_name: task_names.get(&run.task_id).cloned(),
                total: 0,
                counts: DashboardCounts::default(),
            });
        entry.total += 1;
        entry.counts.add(bucket);
    }

    let finished = success + failed;
    let success_rate = (finished > 0).then(|| (success as f64 / finished as f64) * 100.0);
    let mut top_tasks: Vec<_> = per_task.into_values().collect();
    top_tasks.sort_by(|a, b| {
        b.counts
            .failed
            .cmp(&a.counts.failed)
            .then_with(|| b.total.cmp(&a.total))
    });
    top_tasks.truncate(5);
    failed_runs.truncate(5);

    Ok(DashboardActivity {
        range: range.key().to_string(),
        buckets,
        total,
        success,
        failed,
        skipped,
        running,
        success_rate,
        top_tasks,
        failed_runs,
    })
}

async fn task_names(conn: &libsql::Connection) -> anyhow::Result<HashMap<String, String>> {
    let tasks = db::tasks::list(conn).await.context("list tasks")?;
    Ok(tasks.into_iter().map(|task| (task.id, task.name)).collect())
}

fn with_task_name(run: JobRunSummary, task_names: &HashMap<String, String>) -> DashboardRunSummary {
    DashboardRunSummary {
        task_name: task_names.get(&run.task_id).cloned(),
        id: run.id,
        task_id: run.task_id,
        started_at: run.started_at,
        finished_at: run.finished_at,
        exit_code: run.exit_code,
        status: run.status,
        attempt: run.attempt,
        duration_ms: run.duration_ms,
    }
}

fn classify(status: &JobRunStatus) -> Option<ActivityBucket> {
    match status {
        JobRunStatus::Success => Some(ActivityBucket::Success),
        JobRunStatus::Failed | JobRunStatus::Timeout | JobRunStatus::Crashed => {
            Some(ActivityBucket::Failed)
        }
        JobRunStatus::Skipped => Some(ActivityBucket::Skipped),
        JobRunStatus::Running | JobRunStatus::Retrying => Some(ActivityBucket::Running),
    }
}

fn empty_buckets(range: DashboardRange) -> Vec<DashboardBucket> {
    let mut cursor = if range.is_hourly() {
        Utc.with_ymd_and_hms(
            Utc::now().year(),
            Utc::now().month(),
            Utc::now().day(),
            Utc::now().hour(),
            0,
            0,
        )
        .single()
        .unwrap()
    } else {
        Utc.with_ymd_and_hms(
            Utc::now().year(),
            Utc::now().month(),
            Utc::now().day(),
            0,
            0,
            0,
        )
        .single()
        .unwrap()
    };

    let step = if range.is_hourly() {
        chrono::Duration::hours(1)
    } else {
        chrono::Duration::days(1)
    };
    cursor -= step * (range.slots() - 1) as i32;

    let mut buckets = Vec::new();
    for _ in 0..range.slots() {
        buckets.push(DashboardBucket {
            label: bucket_label(cursor, range),
            bucket_start: bucket_key(cursor, range),
            counts: DashboardCounts::default(),
        });
        cursor += step;
    }
    buckets
}

fn bucket_key(ts: chrono::DateTime<Utc>, range: DashboardRange) -> String {
    if range.is_hourly() {
        format!(
            "{:04}-{:02}-{:02}T{:02}",
            ts.year(),
            ts.month(),
            ts.day(),
            ts.hour()
        )
    } else {
        format!("{:04}-{:02}-{:02}", ts.year(), ts.month(), ts.day())
    }
}

fn bucket_label(ts: chrono::DateTime<Utc>, range: DashboardRange) -> String {
    if range.is_hourly() {
        format!("{:02}:00", ts.hour())
    } else {
        format!("{}/{}", ts.month(), ts.day())
    }
}
