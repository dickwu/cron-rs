use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct SseMessage {
    pub event: String,
    pub data: String,
}

pub type EventBus = broadcast::Sender<SseMessage>;

pub fn new(capacity: usize) -> EventBus {
    let (tx, _rx) = broadcast::channel(capacity);
    tx
}

pub fn publish(bus: &EventBus, event: &str, data: serde_json::Value) {
    let payload = match serde_json::to_string(&data) {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = bus.send(SseMessage {
        event: event.to_string(),
        data: payload,
    });
}
