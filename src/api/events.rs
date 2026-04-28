use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::{Stream, StreamExt};

/// GET /api/v1/events — authenticated SSE heartbeat stream.
pub async fn events() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let interval = tokio::time::interval(Duration::from_secs(15));
    let stream = IntervalStream::new(interval).map(|_| {
        Ok(Event::default()
            .event("heartbeat")
            .data(r#"{"status":"ok"}"#))
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("heartbeat"),
    )
}
