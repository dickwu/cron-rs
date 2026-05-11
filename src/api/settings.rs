use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;

use super::AppState;
use crate::db;

const RETENTION_DAYS_MIN: u32 = 1;
const RETENTION_DAYS_MAX: u32 = 3650;

#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    pub retention_days: u32,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsPayload {
    pub retention_days: u32,
}

/// GET /api/v1/settings
pub async fn get_settings(State(state): State<AppState>) -> impl IntoResponse {
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

    match db::settings::get_retention_days(&conn).await {
        Ok(retention_days) => (
            StatusCode::OK,
            Json(json!(SettingsResponse { retention_days })),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to read settings: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Internal server error"})),
            )
                .into_response()
        }
    }
}

/// PUT /api/v1/settings
pub async fn update_settings(
    State(state): State<AppState>,
    Json(payload): Json<UpdateSettingsPayload>,
) -> impl IntoResponse {
    if payload.retention_days < RETENTION_DAYS_MIN || payload.retention_days > RETENTION_DAYS_MAX {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "retention_days must be between {RETENTION_DAYS_MIN} and {RETENTION_DAYS_MAX}"
                )
            })),
        )
            .into_response();
    }

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

    if let Err(e) = db::settings::set_retention_days(&conn, payload.retention_days).await {
        error!("Failed to write settings: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Internal server error"})),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!(SettingsResponse {
            retention_days: payload.retention_days,
        })),
    )
        .into_response()
}
