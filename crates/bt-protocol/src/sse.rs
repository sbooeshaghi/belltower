use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SseEnvelope<T> {
    pub id: i64,
    pub event: String,
    pub protocol_version: String,
    pub data: T,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventStreamCursor {
    pub last_event_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawSseEnvelope {
    pub id: i64,
    pub event: String,
    pub protocol_version: String,
    pub data: Value,
}
