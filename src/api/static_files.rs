use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// Fallback handler for static file serving.
/// When the exported cron-rs-web dashboard is embedded via the `embed-web`
/// feature, serves the SPA. Otherwise returns a simple message pointing to the
/// separate web server.
pub async fn static_handler(request: Request) -> impl IntoResponse {
    let path = request.uri().path();

    // Don't interfere with API routes
    if path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    if path == "/runtime-config.js" {
        return runtime_config_response(request.headers());
    }

    #[cfg(feature = "embed-web")]
    {
        use rust_embed::Embed;

        #[derive(Embed)]
        #[folder = "../cron-rs-web/out/"]
        struct WebAssets;

        if let Some(resp) = serve_spa_path::<WebAssets>(path.trim_start_matches('/')) {
            return resp;
        }
    }

    // No embedded web assets — tell the user where the dashboard is
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(
            "<html><body style='font-family:system-ui;padding:40px;text-align:center'>\
             <h2>cron-rs API</h2>\
             <p>API is running. The web dashboard is served separately.</p>\
             <p>Run <code>cron-rs-web-server</code> or visit <code>http://localhost:3000</code></p>\
             </body></html>",
        ))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response())
}

fn runtime_config_response(headers: &HeaderMap) -> Response {
    let api_url = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.trim().is_empty())
        .map(|host| format!("http://{}", host.trim()))
        .unwrap_or_else(|| "http://localhost:9746".to_string());

    let body = format!(
        "window.__CRON_RS_CONFIG__ = {{ apiUrl: {} }};",
        serde_json::to_string(&api_url).unwrap_or_else(|_| "null".to_string())
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response())
}

#[cfg(feature = "embed-web")]
fn serve_spa_path<E: rust_embed::Embed>(path: &str) -> Option<Response> {
    if let Some(resp) = serve_file::<E>(path) {
        return Some(resp);
    }

    let html_path = if path.is_empty() {
        "index.html".to_string()
    } else {
        format!("{path}.html")
    };
    if let Some(resp) = serve_file::<E>(&html_path) {
        return Some(resp);
    }

    let index_path = format!("{path}/index.html");
    if let Some(resp) = serve_file::<E>(&index_path) {
        return Some(resp);
    }

    if !path.contains('.') {
        return serve_file::<E>("index.html");
    }

    None
}

#[cfg(feature = "embed-web")]
fn serve_file<E: rust_embed::Embed>(path: &str) -> Option<Response> {
    let file = E::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if mime.as_ref() == "text/html" {
        "no-cache"
    } else {
        "public, max-age=3600"
    };
    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache_control)
            .body(Body::from(file.data.to_vec()))
            .unwrap_or_else(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }),
    )
}
