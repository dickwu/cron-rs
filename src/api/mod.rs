pub mod auth;
pub mod events;
pub mod hooks;
pub mod middleware;
pub mod runs;
pub mod settings;
pub mod static_files;
pub mod tasks;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::error;

use crate::config::Config;
use crate::db;
use crate::db::helpers::parse_run_ts;
use crate::db::Database;
use crate::models::JobRunStatus;
use crate::systemd::SystemdManager;

use self::middleware::JwtSecret;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub systemd: Arc<dyn SystemdManager>,
    pub config: Arc<Config>,
    pub event_bus: crate::event_bus::EventBus,
}

/// Build the Axum router with all routes.
pub fn router(state: AppState) -> Router {
    let jwt_secret = state.config.jwt_secret.clone();

    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/health", get(health));

    // Protected routes (auth required)
    let protected_routes = Router::new()
        // Tasks
        .route("/api/v1/tasks", get(tasks::list_tasks))
        .route("/api/v1/tasks", post(tasks::create_task))
        .route("/api/v1/tasks/{id}", get(tasks::get_task))
        .route("/api/v1/tasks/{id}", put(tasks::update_task))
        .route("/api/v1/tasks/{id}", delete(tasks::delete_task))
        .route("/api/v1/tasks/{id}/enable", post(tasks::enable_task))
        .route("/api/v1/tasks/{id}/disable", post(tasks::disable_task))
        .route("/api/v1/tasks/{id}/trigger", post(tasks::trigger_task))
        // Hooks
        .route("/api/v1/hooks", get(hooks::list_all_hooks))
        .route("/api/v1/hooks/global", get(hooks::list_global_hooks))
        .route("/api/v1/hooks/global", post(hooks::create_global_hook))
        .route("/api/v1/tasks/{id}/hooks", get(hooks::list_hooks))
        .route("/api/v1/tasks/{id}/hooks", post(hooks::create_hook))
        .route("/api/v1/hooks/{id}", put(hooks::update_hook))
        .route("/api/v1/hooks/{id}", delete(hooks::delete_hook))
        // Runs
        .route("/api/v1/runs", get(runs::list_runs))
        .route("/api/v1/runs/{id}", get(runs::get_run))
        .route("/api/v1/runs/{id}/hooks", get(runs::list_hook_runs))
        .route("/api/v1/tasks/{id}/runs", get(runs::list_task_runs))
        // Settings
        .route("/api/v1/settings", get(settings::get_settings))
        .route("/api/v1/settings", put(settings::update_settings))
        // Status
        .route("/api/v1/status", get(status))
        .route("/api/v1/events", get(events::events))
        // Apply auth middleware to all protected routes
        .layer(axum::middleware::from_fn(middleware::require_auth));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .fallback(static_files::static_handler)
        .layer(axum::Extension(JwtSecret(jwt_secret)))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// GET /api/v1/health — unauthenticated health check
async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

/// GET /api/v1/status — authenticated status overview
async fn status(State(state): State<AppState>) -> impl IntoResponse {
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

    // Count tasks
    let task_count = match db::tasks::list(&conn).await {
        Ok(tasks) => tasks.len(),
        Err(_) => 0,
    };

    // Count active timers
    let active_timers = match db::tasks::list(&conn).await {
        Ok(tasks) => {
            let mut count = 0u64;
            for task in &tasks {
                if task.enabled {
                    if let Ok(true) = state.systemd.is_timer_active(&task.name).await {
                        count += 1;
                    }
                }
            }
            count
        }
        Err(_) => 0,
    };

    // Aggregate the last 24h in Rust so the cutoff comparison is correct across
    // both RFC 3339 and legacy naïve timestamp formats (lex comparison breaks
    // when one side has a `T` separator and the other has a space).
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let mut runs_24h: u64 = 0;
    let mut success_24h: u64 = 0;
    let mut failed_24h: u64 = 0;
    if let Ok(runs) = db::runs::list_job_runs(&conn, None, None, Some(5000), Some(0)).await {
        for r in &runs {
            let Some(ts) = parse_run_ts(&r.started_at) else { continue };
            if ts < cutoff {
                continue;
            }
            runs_24h += 1;
            match r.status {
                JobRunStatus::Success => success_24h += 1,
                JobRunStatus::Failed | JobRunStatus::Timeout | JobRunStatus::Crashed => {
                    failed_24h += 1
                }
                _ => {}
            }
        }
    }

    let finished_24h = success_24h + failed_24h;
    let success_rate = if finished_24h > 0 {
        (success_24h as f64 / finished_24h as f64) * 100.0
    } else {
        // No finished runs in the window — nothing to grade. Show 100% rather
        // than 0% so a quiet installation doesn't look like it's on fire.
        100.0
    };

    (
        StatusCode::OK,
        Json(json!({
            "task_count": task_count,
            "active_timers": active_timers,
            "runs_24h": runs_24h,
            "success_rate": success_rate,
            "recent_failures_24h": failed_24h,
        })),
    )
        .into_response()
}
