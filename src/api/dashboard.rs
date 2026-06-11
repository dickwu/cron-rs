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
use tracing::{error, warn};

use super::AppState;
use crate::db;
use crate::db::helpers::parse_since;
use crate::models::JobRunStatus;
use crate::systemd::unit_gen;

const SUMMARY_TTL: Duration = Duration::from_secs(5);
const RECENT_RUNS_TTL: Duration = Duration::from_secs(3);
const ACTIVITY_TTL: Duration = Duration::from_secs(10);
const HEATMAP_TTL: Duration = Duration::from_secs(60);
const TASK_ACTIVITY_TTL: Duration = Duration::from_secs(30);
const RECENT_RUNS_DEFAULT_LIMIT: i64 = 20;
const RECENT_RUNS_MAX_LIMIT: i64 = 100;
const HEATMAP_DAYS: i64 = 365;
const TASK_ACTIVITY_DEFAULT_DAYS: i64 = 14;
const TASK_ACTIVITY_MAX_DAYS: i64 = 30;

#[derive(Default)]
pub struct DashboardCache {
    summary: Option<Cached<DashboardSummary>>,
    recent_runs: HashMap<i64, Cached<Vec<DashboardRunSummary>>>,
    activity: HashMap<String, Cached<DashboardActivity>>,
    heatmap: Option<Cached<DashboardHeatmap>>,
    task_activity: HashMap<i64, Cached<DashboardTaskActivity>>,
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
    fn add(&mut self, bucket: ActivityBucket, count: u64) {
        match bucket {
            ActivityBucket::Success => self.success += count,
            ActivityBucket::Failed => self.failed += count,
            ActivityBucket::Skipped => self.skipped += count,
            ActivityBucket::Running => self.running += count,
        }
    }

    fn total(&self) -> u64 {
        self.success + self.failed + self.skipped + self.running
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

#[derive(Debug, Clone, Serialize)]
pub struct HeatmapDayCount {
    /// UTC calendar day, `YYYY-MM-DD`.
    pub date: String,
    pub total: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardHeatmap {
    pub days: i64,
    /// Sparse ascending day buckets; days without runs are omitted.
    pub buckets: Vec<HeatmapDayCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskActivityRow {
    pub task_id: String,
    /// UTC calendar day, `YYYY-MM-DD`.
    pub date: String,
    pub counts: DashboardCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardTaskActivity {
    pub days: i64,
    pub rows: Vec<TaskActivityRow>,
}

#[derive(Debug, Deserialize)]
pub struct RecentRunsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub range: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TaskActivityQuery {
    pub days: Option<i64>,
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

/// GET /api/v1/dashboard/heatmap — daily run totals for the trailing year,
/// aggregated in SQL so the payload stays a few KB at any run volume.
pub async fn heatmap(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(value) = state
        .dashboard_cache
        .read()
        .await
        .heatmap
        .as_ref()
        .and_then(Cached::get)
    {
        return (StatusCode::OK, Json(value)).into_response();
    }

    match load_heatmap(&state).await {
        Ok(heatmap) => {
            state.dashboard_cache.write().await.heatmap =
                Some(Cached::new(heatmap.clone(), HEATMAP_TTL));
            (StatusCode::OK, Json(heatmap)).into_response()
        }
        Err(e) => internal_error("Failed to load dashboard heatmap", e),
    }
}

/// GET /api/v1/dashboard/task-activity — per-task daily counts for sparklines.
pub async fn task_activity(
    State(state): State<AppState>,
    Query(query): Query<TaskActivityQuery>,
) -> impl IntoResponse {
    let days = query
        .days
        .unwrap_or(TASK_ACTIVITY_DEFAULT_DAYS)
        .clamp(1, TASK_ACTIVITY_MAX_DAYS);

    if let Some(value) = state
        .dashboard_cache
        .read()
        .await
        .task_activity
        .get(&days)
        .and_then(Cached::get)
    {
        return (StatusCode::OK, Json(value)).into_response();
    }

    match load_task_activity(&state, days).await {
        Ok(activity) => {
            state
                .dashboard_cache
                .write()
                .await
                .task_activity
                .insert(days, Cached::new(activity.clone(), TASK_ACTIVITY_TTL));
            (StatusCode::OK, Json(activity)).into_response()
        }
        Err(e) => internal_error("Failed to load task activity", e),
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

fn cutoff_rfc3339(duration: chrono::Duration) -> String {
    (Utc::now() - duration).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn load_summary(state: &AppState) -> anyhow::Result<DashboardSummary> {
    let conn = state.db.connect().await.context("connect database")?;
    let tasks = db::tasks::list(&conn).await.context("list tasks")?;

    // One systemctl call for all timers instead of one per task.
    let active_timers = match state.systemd.active_timer_names().await {
        Ok(active) => tasks
            .iter()
            .filter(|task| task.enabled && active.contains(&unit_gen::timer_filename(&task.name)))
            .count() as u64,
        Err(e) => {
            warn!("Failed to list active timers: {}", e);
            0
        }
    };

    // Exact 24h counts straight from SQL; immune to row-fetch caps.
    let cutoff = cutoff_rfc3339(chrono::Duration::hours(24));
    let counts = db::runs::status_counts_since(&conn, Some(&cutoff))
        .await
        .context("count run statuses")?;

    let mut totals = DashboardCounts::default();
    for (status, count) in counts {
        if let Some(bucket) = classify_str(&status) {
            totals.add(bucket, count as u64);
        }
    }

    let finished_24h = totals.success + totals.failed;
    let success_rate = if finished_24h > 0 {
        (totals.success as f64 / finished_24h as f64) * 100.0
    } else {
        100.0
    };

    Ok(DashboardSummary {
        task_count: tasks.len(),
        active_timers,
        runs_24h: totals.total(),
        success_rate,
        recent_failures_24h: totals.failed,
    })
}

async fn load_recent_runs(
    state: &AppState,
    limit: i64,
) -> anyhow::Result<Vec<DashboardRunSummary>> {
    let conn = state.db.connect().await.context("connect database")?;
    let task_names = task_names(&conn).await?;
    let runs = db::runs::list_job_run_summaries(&conn, None, None, None, Some(limit), Some(0))
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
    let cutoff = cutoff_rfc3339(duration);
    let key_len: u8 = if range.is_hourly() { 13 } else { 10 };

    // Aggregate buckets, totals, and per-task breakdowns in SQL so results
    // are exact regardless of run volume.
    let bucket_counts = db::runs::bucket_status_counts(&conn, &cutoff, key_len)
        .await
        .context("bucket run statuses")?;
    let per_task = db::runs::per_task_status_counts(&conn, &cutoff)
        .await
        .context("count per-task statuses")?;
    let failed_summaries = db::runs::list_failed_run_summaries(&conn, &cutoff, 5)
        .await
        .context("list failed runs")?;

    let mut buckets = empty_buckets(range);
    let bucket_index: HashMap<String, usize> = buckets
        .iter()
        .enumerate()
        .map(|(idx, bucket)| (bucket.bucket_start.clone(), idx))
        .collect();

    let mut totals = DashboardCounts::default();
    for (bucket_key, status, count) in bucket_counts {
        let Some(bucket) = classify_str(&status) else {
            continue;
        };
        totals.add(bucket, count as u64);
        if let Some(idx) = bucket_index.get(&bucket_key) {
            buckets[*idx].counts.add(bucket, count as u64);
        }
    }

    let mut breakdown: HashMap<String, DashboardTaskBreakdown> = HashMap::new();
    for (task_id, status, count) in per_task {
        let Some(bucket) = classify_str(&status) else {
            continue;
        };
        let entry = breakdown
            .entry(task_id.clone())
            .or_insert_with(|| DashboardTaskBreakdown {
                task_name: task_names.get(&task_id).cloned(),
                task_id,
                total: 0,
                counts: DashboardCounts::default(),
            });
        entry.total += count as u64;
        entry.counts.add(bucket, count as u64);
    }

    let mut top_tasks: Vec<_> = breakdown.into_values().collect();
    top_tasks.sort_by(|a, b| {
        b.counts
            .failed
            .cmp(&a.counts.failed)
            .then_with(|| b.total.cmp(&a.total))
    });
    top_tasks.truncate(5);

    let failed_runs: Vec<_> = failed_summaries
        .into_iter()
        .map(|run| with_task_name(run, &task_names))
        .collect();

    let finished = totals.success + totals.failed;
    let success_rate = (finished > 0).then(|| (totals.success as f64 / finished as f64) * 100.0);

    Ok(DashboardActivity {
        range: range.key().to_string(),
        buckets,
        total: totals.total(),
        success: totals.success,
        failed: totals.failed,
        skipped: totals.skipped,
        running: totals.running,
        success_rate,
        top_tasks,
        failed_runs,
    })
}

async fn load_heatmap(state: &AppState) -> anyhow::Result<DashboardHeatmap> {
    let conn = state.db.connect().await.context("connect database")?;
    let cutoff = cutoff_rfc3339(chrono::Duration::days(HEATMAP_DAYS));
    let counts = db::runs::bucket_status_counts(&conn, &cutoff, 10)
        .await
        .context("bucket daily statuses")?;

    let mut buckets: Vec<HeatmapDayCount> = Vec::new();
    for (date, status, count) in counts {
        if buckets.last().map(|b| b.date.as_str()) != Some(date.as_str()) {
            buckets.push(HeatmapDayCount {
                date,
                total: 0,
                failed: 0,
            });
        }
        let day = buckets.last_mut().expect("bucket just pushed");
        day.total += count as u64;
        if matches!(classify_str(&status), Some(ActivityBucket::Failed)) {
            day.failed += count as u64;
        }
    }

    Ok(DashboardHeatmap {
        days: HEATMAP_DAYS,
        buckets,
    })
}

async fn load_task_activity(state: &AppState, days: i64) -> anyhow::Result<DashboardTaskActivity> {
    let conn = state.db.connect().await.context("connect database")?;
    let cutoff = cutoff_rfc3339(chrono::Duration::days(days));
    let counts = db::runs::per_task_daily_status_counts(&conn, &cutoff)
        .await
        .context("count per-task daily statuses")?;

    let mut rows: Vec<TaskActivityRow> = Vec::new();
    for (task_id, date, status, count) in counts {
        let Some(bucket) = classify_str(&status) else {
            continue;
        };
        let needs_new_row = rows
            .last()
            .map(|row| row.task_id != task_id || row.date != date)
            .unwrap_or(true);
        if needs_new_row {
            rows.push(TaskActivityRow {
                task_id,
                date,
                counts: DashboardCounts::default(),
            });
        }
        rows.last_mut()
            .expect("row just pushed")
            .counts
            .add(bucket, count as u64);
    }

    Ok(DashboardTaskActivity { days, rows })
}

async fn task_names(conn: &libsql::Connection) -> anyhow::Result<HashMap<String, String>> {
    let tasks = db::tasks::list(conn).await.context("list tasks")?;
    Ok(tasks.into_iter().map(|task| (task.id, task.name)).collect())
}

fn with_task_name(
    run: crate::models::JobRunSummary,
    task_names: &HashMap<String, String>,
) -> DashboardRunSummary {
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

fn classify_str(status: &str) -> Option<ActivityBucket> {
    match status.parse::<JobRunStatus>().ok()? {
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
