use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// JWT claims embedded in every issued token.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

/// JWT authentication middleware.
/// Validates the Authorization: Bearer <token> header and injects claims
/// into request extensions.
pub async fn require_auth(
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let jwt_secret = request
        .extensions()
        .get::<JwtSecret>()
        .map(|s| s.0.clone())
        .unwrap_or_default();

    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            header.strip_prefix("Bearer ").unwrap().to_string()
        }
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"error": "Missing or invalid Authorization header"})),
            )
                .into_response());
        }
    };

    let validation = Validation::new(Algorithm::HS256);
    match decode::<Claims>(
        &token,
        &DecodingKey::from_secret(jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(_token_data) => Ok(next.run(request).await),
        Err(_) => Err((
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": "Invalid or expired token"})),
        )
            .into_response()),
    }
}

/// Wrapper type to pass the JWT secret through request extensions.
#[derive(Clone)]
pub struct JwtSecret(pub String);
