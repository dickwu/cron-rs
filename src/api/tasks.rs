use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use tracing::{error, info};

use super::AppState;
use crate::db;
use crate::db::helpers::DbError;
use crate::models::task::ConcurrencyPolicy;
use crate::models::Task;
use crate::systemd::unit_gen;

// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub command: String,
    pub schedule: String,
    pub tags: Option<Vec<String>>,
    pub description: Option<String>,
    pub max_retries: Option<i32>,
    pub retry_delay_secs: Option<i32>,
    pub timeout_secs: Option<i32>,
    pub concurrency_policy: Option<String>,
    pub lock_key: Option<String>,
    pub sandbox_profile: Option<String>,
}

#[derive(Debug, Clone, Default)]
enum OptionalStringUpdate {
    #[default]
    Missing,
    Clear,
    Set(String),
}

impl<'de> Deserialize<'de> for OptionalStringUpdate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    pub name: Option<String>,
    pub command: Option<String>,
    pub schedule: Option<String>,
    pub tags: Option<Vec<String>>,
    pub description: Option<String>,
    pub max_retries: Option<i32>,
    pub retry_delay_secs: Option<i32>,
    pub timeout_secs: Option<i32>,
    pub concurrency_policy: Option<String>,
    #[serde(default)]
    lock_key: OptionalStringUpdate,
    #[serde(default)]
    sandbox_profile: OptionalStringUpdate,
}

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub id: String,
    pub name: String,
    pub command: String,
    pub schedule: String,
    pub tags: Vec<String>,
    pub description: String,
    pub enabled: bool,
    pub max_retries: i32,
    pub retry_delay_secs: i32,
    pub timeout_secs: Option<i32>,
    pub concurrency_policy: ConcurrencyPolicy,
    pub lock_key: Option<String>,
    pub sandbox_profile: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Task> for TaskResponse {
    fn from(t: Task) -> Self {
        TaskResponse {
            id: t.id,
            name: t.name,
            command: t.command,
            schedule: t.schedule,
            tags: t.tags,
            description: t.description,
            enabled: t.enabled,
            max_retries: t.max_retries,
            retry_delay_secs: t.retry_delay_secs,
            timeout_secs: t.timeout_secs,
            concurrency_policy: t.concurrency_policy,
            lock_key: t.lock_key,
            sandbox_profile: t.sandbox_profile,
            created_at: t.created_at,
            updated_at: t.updated_at,
        }
    }
}

// --- Helpers ---

fn db_error_to_response(err: DbError) -> (StatusCode, Json<serde_json::Value>) {
    match err {
        DbError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Task not found"})),
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

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for tag in tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }

        if seen.insert(trimmed.to_ascii_lowercase()) {
            normalized.push(trimmed.to_string());
        }
    }

    normalized
}

fn normalize_lock_key(lock_key: Option<String>) -> Option<String> {
    lock_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn normalize_sandbox_profile(profile: Option<String>) -> Result<Option<String>, String> {
    match profile
        .map(|profile| profile.trim().to_string())
        .filter(|profile| !profile.is_empty())
    {
        Some(profile) if unit_gen::is_supported_sandbox_profile(&profile) => Ok(Some(profile)),
        Some(profile) => Err(format!("unsupported sandbox_profile: {profile}")),
        None => Ok(None),
    }
}

fn parse_optional_string_update(
    value: OptionalStringUpdate,
    field: &str,
) -> Result<Option<Option<String>>, String> {
    match value {
        OptionalStringUpdate::Missing => Ok(None),
        OptionalStringUpdate::Clear => Ok(Some(None)),
        OptionalStringUpdate::Set(value) => {
            let normalized = normalize_lock_key(Some(value))
                .ok_or_else(|| format!("{field} cannot be empty"))?;
            Ok(Some(Some(normalized)))
        }
    }
}

// --- Handlers ---

/// GET /api/v1/tasks
pub async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
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

    match db::tasks::list(&conn).await {
        Ok(tasks) => {
            let responses: Vec<TaskResponse> = tasks.into_iter().map(TaskResponse::from).collect();
            (StatusCode::OK, Json(json!(responses))).into_response()
        }
        Err(e) => {
            error!("Failed to list tasks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/tasks
pub async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let concurrency_policy = match &body.concurrency_policy {
        Some(cp) => match cp.parse::<ConcurrencyPolicy>() {
            Ok(p) => p,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        None => ConcurrencyPolicy::Skip,
    };

    let task = Task {
        id: String::new(),
        name: body.name,
        command: body.command,
        schedule: body.schedule,
        tags: normalize_tags(body.tags.unwrap_or_default()),
        description: body.description.unwrap_or_default(),
        enabled: true,
        max_retries: body.max_retries.unwrap_or(0),
        retry_delay_secs: body.retry_delay_secs.unwrap_or(5),
        timeout_secs: body.timeout_secs,
        concurrency_policy,
        lock_key: normalize_lock_key(body.lock_key),
        sandbox_profile: match normalize_sandbox_profile(body.sandbox_profile) {
            Ok(profile) => profile,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        created_at: String::new(),
        updated_at: String::new(),
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

    let created = match db::tasks::create(&conn, &task).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Install systemd units if the task is enabled
    if created.enabled {
        if let Err(e) = state.systemd.install_task(&created).await {
            error!(
                "Failed to install systemd units for task '{}': {}",
                created.name, e
            );
            // Task was created in DB but systemd install failed. Return the task
            // but log the error. Don't fail the request.
        }
    }

    info!("Created task '{}' (id: {})", created.name, created.id);
    (
        StatusCode::CREATED,
        Json(json!(TaskResponse::from(created))),
    )
        .into_response()
}

/// GET /api/v1/tasks/:id
pub async fn get_task(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
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

    match db::tasks::get_by_id(&conn, &id).await {
        Ok(task) => (StatusCode::OK, Json(json!(TaskResponse::from(task)))).into_response(),
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            (status, body).into_response()
        }
    }
}

/// PUT /api/v1/tasks/:id
pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskRequest>,
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

    // Fetch existing task
    let existing = match db::tasks::get_by_id(&conn, &id).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Parse concurrency policy if provided
    let concurrency_policy = match &body.concurrency_policy {
        Some(cp) => match cp.parse::<ConcurrencyPolicy>() {
            Ok(p) => p,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        None => existing.concurrency_policy.clone(),
    };

    let updated_task = Task {
        id: existing.id.clone(),
        name: body.name.unwrap_or(existing.name.clone()),
        command: body.command.unwrap_or(existing.command),
        schedule: body.schedule.unwrap_or(existing.schedule),
        tags: body.tags.map(normalize_tags).unwrap_or(existing.tags),
        description: body.description.unwrap_or(existing.description),
        enabled: existing.enabled,
        max_retries: body.max_retries.unwrap_or(existing.max_retries),
        retry_delay_secs: body.retry_delay_secs.unwrap_or(existing.retry_delay_secs),
        timeout_secs: body.timeout_secs.or(existing.timeout_secs),
        concurrency_policy,
        lock_key: match parse_optional_string_update(body.lock_key, "lock_key") {
            Ok(Some(lock_key)) => lock_key,
            Ok(None) => existing.lock_key,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        sandbox_profile: match parse_optional_string_update(body.sandbox_profile, "sandbox_profile")
        {
            Ok(Some(profile)) => match normalize_sandbox_profile(profile) {
                Ok(profile) => profile,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
                }
            },
            Ok(None) => existing.sandbox_profile,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
            }
        },
        created_at: existing.created_at,
        updated_at: String::new(), // will be set by db::tasks::update
    };

    let saved = match db::tasks::update(&conn, &updated_task).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Reinstall systemd units if the task is enabled
    if saved.enabled {
        // Remove old units (using old name if name changed)
        let _ = state.systemd.remove_task(&existing.name).await;
        if let Err(e) = state.systemd.install_task(&saved).await {
            error!(
                "Failed to reinstall systemd units for task '{}': {}",
                saved.name, e
            );
        }
    }

    info!("Updated task '{}' (id: {})", saved.name, saved.id);
    (StatusCode::OK, Json(json!(TaskResponse::from(saved)))).into_response()
}

/// DELETE /api/v1/tasks/:id
pub async fn delete_task(
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

    // Get task to know its name for systemd cleanup
    let task = match db::tasks::get_by_id(&conn, &id).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Remove systemd units
    if let Err(e) = state.systemd.remove_task(&task.name).await {
        error!(
            "Failed to remove systemd units for task '{}': {}",
            task.name, e
        );
    }

    // Delete from DB
    match db::tasks::delete(&conn, &id).await {
        Ok(()) => {
            info!("Deleted task '{}' (id: {})", task.name, id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            (status, body).into_response()
        }
    }
}

/// POST /api/v1/tasks/:id/enable
pub async fn enable_task(
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

    let mut task = match db::tasks::get_by_id(&conn, &id).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    task.enabled = true;
    let saved = match db::tasks::update(&conn, &task).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Install and enable systemd timer
    if let Err(e) = state.systemd.install_task(&saved).await {
        error!(
            "Failed to enable systemd timer for task '{}': {}",
            saved.name, e
        );
    }

    info!("Enabled task '{}' (id: {})", saved.name, saved.id);
    (StatusCode::OK, Json(json!(TaskResponse::from(saved)))).into_response()
}

/// POST /api/v1/tasks/:id/disable
pub async fn disable_task(
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

    let mut task = match db::tasks::get_by_id(&conn, &id).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    task.enabled = false;
    let saved = match db::tasks::update(&conn, &task).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Disable systemd timer
    if let Err(e) = state.systemd.disable_timer(&saved.name).await {
        error!(
            "Failed to disable systemd timer for task '{}': {}",
            saved.name, e
        );
    }
    // Also stop the timer
    if let Err(e) = state.systemd.stop_timer(&saved.name).await {
        error!(
            "Failed to stop systemd timer for task '{}': {}",
            saved.name, e
        );
    }

    info!("Disabled task '{}' (id: {})", saved.name, saved.id);
    (StatusCode::OK, Json(json!(TaskResponse::from(saved)))).into_response()
}

/// POST /api/v1/tasks/:id/trigger
pub async fn trigger_task(
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

    let task = match db::tasks::get_by_id(&conn, &id).await {
        Ok(t) => t,
        Err(e) => {
            let (status, body) = db_error_to_response(e);
            return (status, body).into_response();
        }
    };

    // Spawn `cron-rs run --task-id ... --task-name ... --db-path ...` as a detached background process
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to resolve current executable: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response();
        }
    };

    let db_path = state.config.db_path.to_string_lossy().to_string();

    // Clone values needed after the spawn
    let task_name = task.name.clone();
    let task_id = task.id.clone();
    let spawn_id = id.clone();

    tokio::spawn(async move {
        let result = tokio::process::Command::new(&exe)
            .arg("run")
            .arg("--task-id")
            .arg(&spawn_id)
            .arg("--task-name")
            .arg(&task.name)
            .arg("--db-path")
            .arg(&db_path)
            .spawn();

        match result {
            Ok(mut child) => {
                if let Err(e) = child.wait().await {
                    error!("Triggered task run process failed: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to spawn triggered task run: {}", e);
            }
        }
    });

    info!("Triggered task '{}' (id: {})", task_name, task_id);
    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Task triggered", "task_id": task_id})),
    )
        .into_response()
}
