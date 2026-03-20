use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// Fallback handler for static file serving.
/// When a `web/out/` directory is embedded (via the `embed-web` feature),
/// serves the SPA. Otherwise returns a simple message pointing to the
/// separate cron-rs-web server.
pub async fn static_handler(request: Request) -> impl IntoResponse {
    let path = request.uri().path();

    // Don't interfere with API routes
    if path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    #[cfg(feature = "embed-web")]
    {
        use rust_embed::Embed;

        #[derive(Embed)]
        #[folder = "web/out/"]
        struct WebAssets;

        let file_path = path.trim_start_matches('/');

        // Try exact file
        if let Some(resp) = serve_file::<WebAssets>(file_path) {
            return resp;
        }

        // SPA fallback
        if let Some(resp) = serve_file::<WebAssets>("index.html") {
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

#[cfg(feature = "embed-web")]
fn serve_file<E: rust_embed::Embed>(path: &str) -> Option<Response> {
    let file = E::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(Body::from(file.data.to_vec()))
            .unwrap_or_else(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }),
    )
}
