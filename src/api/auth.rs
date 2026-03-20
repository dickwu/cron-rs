use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::AppState;
use super::middleware::Claims;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
}

/// POST /api/v1/auth/login
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    // Check username
    if body.username != state.config.username {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid credentials"})),
        );
    }

    // Verify password against stored argon2 hash
    let password_hash = &state.config.password_hash;
    if password_hash.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid credentials"})),
        );
    }

    let hash_parsed = match argon2::PasswordHash::new(password_hash) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Server configuration error"})),
            );
        }
    };

    use argon2::PasswordVerifier;
    let argon2 = argon2::Argon2::default();
    if argon2
        .verify_password(body.password.as_bytes(), &hash_parsed)
        .is_err()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid credentials"})),
        );
    }

    // Parse token expiry from config (e.g. "24h", "1h", "30m")
    let expiry_secs = parse_expiry(&state.config.token_expiry).unwrap_or(24 * 3600);

    let exp = (chrono::Utc::now().timestamp() as usize) + expiry_secs;
    let claims = Claims {
        sub: body.username.clone(),
        exp,
    };

    let jwt_secret = &state.config.jwt_secret;
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_bytes()),
    ) {
        Ok(token) => (StatusCode::OK, Json(json!({"token": token}))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to generate token"})),
        ),
    }
}

/// Parse an expiry string like "24h", "1h", "30m", "3600s" into seconds.
fn parse_expiry(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.ends_with('h') {
        s[..s.len() - 1].parse::<usize>().ok().map(|h| h * 3600)
    } else if s.ends_with('m') {
        s[..s.len() - 1].parse::<usize>().ok().map(|m| m * 60)
    } else if s.ends_with('s') {
        s[..s.len() - 1].parse::<usize>().ok()
    } else {
        s.parse::<usize>().ok()
    }
}
