//! Broadcast event bus — Rust port of `app/events.py`.
//!
//! The Python version had per-subscriber `asyncio.Queue` with drop-oldest
//! semantics. `tokio::sync::broadcast` provides equivalent behavior: every
//! subscriber gets its own ring buffer (capacity = `BUS_CAPACITY`), and a
//! slow receiver returns `RecvError::Lagged(n)` instead of blocking the
//! sender.
//!
//! A relay task spawned at app setup time receives `Event`s from this bus
//! and converts each to a Tauri event (`app_handle.emit(name, payload)`)
//! visible to the webview. The bus survives profile hot reload.

use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast::{self, Receiver, Sender};

/// Drop-oldest backlog per subscriber. Matches the `maxsize=1000` deque in
/// the Python EventBus.
pub const BUS_CAPACITY: usize = 1024;

/// Tauri event names emitted by the relay. Frontend listens to these.
pub mod tauri_event_name {
    pub const EC2_LINE: &str = "ec2-line";
    pub const CONSOLE_MSG: &str = "console-msg";
    pub const CLAUDE_CHUNK: &str = "claude-chunk";
    pub const METRICS_UPDATE: &str = "metrics-update";
    pub const MONITOR_STATUS: &str = "monitor-status";
    pub const MATCH_DETECTED: &str = "match-detected";
}

/// Strongly-typed payload variants. `serde(tag="channel", content="payload")`
/// keeps the JSON shape identical to the old `{channel, payload, ts}` events
/// for easy frontend migration / debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "channel", content = "payload", rename_all = "snake_case")]
pub enum EventPayload {
    /// One line of `docker logs -f` output for a node.
    Ec2 { node: String, line: String },

    /// Tool console line — system status, matches, errors, warnings.
    Console {
        level: String,
        msg: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
    },

    /// Raw chunk from the agent CLI's PTY (xterm.js writes this directly).
    Claude { chunk: String },

    /// Periodic metrics for one node.
    Metrics {
        node: String,
        docker: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        health: Option<Value>,
    },

    /// Whether monitoring is currently active.
    Status { running: bool },

    /// New match was just persisted to history (and possibly injected). The
    /// frontend uses this to fire OS notifications and refresh history.
    Match {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<i64>,
        node: String,
        container: String,
        pattern: String,
        matched_line: String,
    },
}

/// Wire-level event with timestamp. The Tauri relay strips `channel` when
/// dispatching, since the event NAME already carries that information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: String,
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    pub fn new(payload: EventPayload) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            payload,
        }
    }

    pub fn ec2(node: impl Into<String>, line: impl Into<String>) -> Self {
        Self::new(EventPayload::Ec2 {
            node: node.into(),
            line: line.into(),
        })
    }

    pub fn console(level: &str, msg: impl Into<String>, node: Option<String>) -> Self {
        Self::new(EventPayload::Console {
            level: level.into(),
            msg: msg.into(),
            node,
        })
    }

    pub fn claude(chunk: impl Into<String>) -> Self {
        Self::new(EventPayload::Claude {
            chunk: chunk.into(),
        })
    }

    pub fn metrics(node: impl Into<String>, docker: Value, health: Option<Value>) -> Self {
        Self::new(EventPayload::Metrics {
            node: node.into(),
            docker,
            health,
        })
    }

    pub fn status(running: bool) -> Self {
        Self::new(EventPayload::Status { running })
    }

    pub fn match_detected(
        id: Option<i64>,
        node: impl Into<String>,
        container: impl Into<String>,
        pattern: impl Into<String>,
        matched_line: impl Into<String>,
    ) -> Self {
        Self::new(EventPayload::Match {
            id,
            node: node.into(),
            container: container.into(),
            pattern: pattern.into(),
            matched_line: matched_line.into(),
        })
    }

    /// Tauri event name appropriate for this payload variant.
    pub fn tauri_event_name(&self) -> &'static str {
        match self.payload {
            EventPayload::Ec2 { .. } => tauri_event_name::EC2_LINE,
            EventPayload::Console { .. } => tauri_event_name::CONSOLE_MSG,
            EventPayload::Claude { .. } => tauri_event_name::CLAUDE_CHUNK,
            EventPayload::Metrics { .. } => tauri_event_name::METRICS_UPDATE,
            EventPayload::Status { .. } => tauri_event_name::MONITOR_STATUS,
            EventPayload::Match { .. } => tauri_event_name::MATCH_DETECTED,
        }
    }
}

/// Broadcast bus shared across all monitors / pollers / PTY readers.
#[derive(Clone)]
pub struct EventBus {
    tx: Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    pub fn sender(&self) -> Sender<Event> {
        self.tx.clone()
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        self.tx.subscribe()
    }

    /// Best-effort publish. If there are no subscribers (no Tauri relay yet,
    /// no active history listeners), `send` returns Err — that's fine, we
    /// silently drop. Lagged subscribers are handled on the receive side.
    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishing_with_no_subscribers_does_not_panic() {
        let bus = EventBus::new();
        bus.publish(Event::status(true));
    }

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(Event::ec2("web-1", "hello"));
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.tauri_event_name(), tauri_event_name::EC2_LINE);
        match ev.payload {
            EventPayload::Ec2 { node, line } => {
                assert_eq!(node, "web-1");
                assert_eq!(line, "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn match_event_serializes_with_channel_tag() {
        let ev = Event::match_detected(
            Some(7),
            "web-1",
            "user-api",
            r#""statusCode":500"#,
            r#"{"statusCode":500,"url":"/x"}"#,
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["channel"], "match");
        assert_eq!(json["payload"]["id"], 7);
        assert_eq!(json["payload"]["node"], "web-1");
    }
}
