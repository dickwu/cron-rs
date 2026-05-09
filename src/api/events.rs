use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tokio_stream::{Stream, StreamExt};

use super::AppState;

/// GET /api/v1/events — authenticated SSE stream that mirrors task and run
/// lifecycle events from the in-process event bus, plus a periodic heartbeat
/// for liveness checks.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_bus.subscribe();
    let bus_stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(msg) => Some(Ok(Event::default().event(msg.event).data(msg.data))),
        Err(_) => None,
    });

    let heartbeat = IntervalStream::new(tokio::time::interval(Duration::from_secs(15)))
        .map(|_| Ok(Event::default().event("heartbeat").data(r#"{"status":"ok"}"#)));

    let merged = bus_stream.merge(heartbeat);

    Sse::new(merged).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("heartbeat"),
    )
}
