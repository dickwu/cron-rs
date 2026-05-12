pub mod auth;
pub mod dashboard;
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
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::error;

use crate::config::Config;
use crate::db::Database;
use crate::systemd::SystemdManager;

use self::middleware::JwtSecret;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub systemd: Arc<dyn SystemdManager>,
    pub config: Arc<Config>,
    pub event_bus: crate::event_bus::EventBus,
    pub dashboard_cache: Arc<RwLock<dashboard::DashboardCache>>,
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
        .route("/api/v1/tasks/{id}/detail", get(tasks::get_task_detail))
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
        // Dashboard
        .route("/api/v1/dashboard/summary", get(dashboard::summary))
        .route("/api/v1/dashboard/runs", get(dashboard::recent_runs))
        .route("/api/v1/dashboard/activity", get(dashboard::activity))
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
    match dashboard::summary_data(&state).await {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(e) => {
            error!("Failed to load status: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}
