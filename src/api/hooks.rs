use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info};

use super::AppState;
use crate::db;
use crate::db::helpers::DbError;
use crate::models::{Hook, HookType};

// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct CreateHookRequest {
    pub hook_type: String,
    pub command: String,
    pub timeout_secs: Option<i32>,
    pub run_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateHookRequest {
    pub hook_type: Option<String>,
    pub command: Option<String>,
    pub timeout_secs: Option<i32>,
    pub run_order: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct HookResponse {
    pub id: String,
    pub task_id: String,
    pub hook_type: HookType,
    pub command: String,
    pub timeout_secs: Option<i32>,
    pub run_order: i32,
    pub created_at: String,
}

impl From<Hook> for HookResponse {
    fn from(h: Hook) -> Self {
        HookResponse {
            id: h.id,
            task_id: h.task_id,
            hook_type: h.hook_type,
            command: h.command,
            timeout_secs: h.timeout_secs,
            run_order: h.run_order,
            created_at: h.created_at,
        }
    }
}

// --- Helpers ---

fn db_error_to_response(err: DbError) -> (StatusCode, Json<serde_json::Value>) {
    match err {
        DbError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Hook not found"})),
        ),
        DbError::Conflict(msg) => (StatusCode::CONFLICT, Json(json!({"error": msg}))),
        other => {
            error!("Database error: {}", other);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
        }
    }
}

// --- Handlers ---

/// GET /api/v1/tasks/:task_id/hooks
pub async fn list_hooks(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
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
        let (status, body) = match e {
            DbError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Task not found"})),
            ),
            other => {
                error!("Database error: {}", other);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Internal server error"})),
                )
            }
        };
        return (status, body).into_response();
    }

    match db::hooks::list_for_task(&conn, &task_id).await {
        Ok(hooks) => {
            let responses: Vec<HookResponse> =
                hooks.into_iter().map(HookResponse::from).collect();
            (StatusCode::OK, Json(json!(responses))).into_response()
        }
        Err(e) => {
            error!("Failed to list hooks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/tasks/:task_id/hooks
pub async fn create_hook(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(body): Json<CreateHookRequest>,
) -> impl IntoResponse {
    let hook_type = match body.hook_type.parse::<HookType>() {
        Ok(ht) => ht,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
        }
    };

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
        let (status, body) = match e {
            DbError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Task not found"})),
            ),
            other => {
                error!("Database error: {}", other);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Internal server error"})),
                )
            }
        };
        return (status, body).into_response();
    }

    let hook = Hook {
        id: String::new(),
        task_id,
        hook_type,
        command: body.command,
        timeout_secs: body.timeout_secs,
        run_order: body.run_order.unwrap_or(0),
        created_at: String::new(),
    };

    match db::hooks::create(&conn, &hook).await {
        Ok(created) => {
            info!("Created hook (id: {}) for task '{}'", created.id, created.task_id);
            (
                StatusCode::CREATED,
                Json(json!(HookResponse::from(created))),
            )
                .into_response()
        }
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            (status, body).into_response()
        }
    }
}

/// PUT /api/v1/hooks/:id
pub async fn update_hook(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateHookRequest>,
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

    // Fetch existing hook
    let existing = match db::hooks::get_by_id(&conn, &id).await {
        Ok(h) => h,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    let hook_type = match &body.hook_type {
        Some(ht) => match ht.parse::<HookType>() {
            Ok(parsed) => parsed,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        None => existing.hook_type,
    };

    let updated_hook = Hook {
        id: existing.id,
        task_id: existing.task_id,
        hook_type,
        command: body.command.unwrap_or(existing.command),
        timeout_secs: body.timeout_secs.or(existing.timeout_secs),
        run_order: body.run_order.unwrap_or(existing.run_order),
        created_at: existing.created_at,
    };

    match db::hooks::update(&conn, &updated_hook).await {
        Ok(saved) => {
            info!("Updated hook (id: {})", saved.id);
            (StatusCode::OK, Json(json!(HookResponse::from(saved)))).into_response()
        }
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            (status, body).into_response()
        }
    }
}

/// DELETE /api/v1/hooks/:id
pub async fn delete_hook(
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

    match db::hooks::delete(&conn, &id).await {
        Ok(()) => {
            info!("Deleted hook (id: {})", id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            (status, body).into_response()
        }
    }
}
