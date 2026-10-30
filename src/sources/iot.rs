//! AWS IoT Core subscriber. **A stub**: the MQTT client is not written yet, so
//! `run` only reports that in the status line. The mapping is ready: once
//! messages arrive, [`CallTracker`] turns Apollo's live call-event stream
//! (`apollo/<env>/t/<tenant>/call/<callId>/events`, payload `CallEventPayload`
//! in sip's `services/shared/src/iot-publish.ts`) into one timer island per
//! call that goes away on hangup.
//!
//! To finish it: connect to `config.endpoint` with MQTT over WSS, SigV4-signed
//! with the profile's credentials (`aws configure export-credentials`), needing
//! `iot:Connect`, `iot:Subscribe` and `iot:Receive` on the topic filters.
//! Subscribe to `config.topics` and feed each message to `CallTracker::handle`.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use chrono::DateTime;
use serde::Deserialize;

use crate::config::IotConfig;
use crate::island::{Island, Kind, Severity};
use crate::source::{Inbound, Sink, Source};

pub struct Iot {
    config: IotConfig,
}

impl Iot {
    pub fn new(config: IotConfig) -> Self {
        Self { config }
    }
}

impl Source for Iot {
    fn id(&self) -> String {
        "iot".into()
    }

    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>) {
        sink.status(format!(
            "iot: not implemented yet ({} topic filter(s) on {})",
            self.config.topics.len(),
            if self.config.endpoint.is_empty() {
                "no endpoint"
            } else {
                &self.config.endpoint
            }
        ));
        // Stay registered so actions on its islands have somewhere to go.
        for _ in inbox {}
    }
}

/// One message on a call's `events` topic.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallEvent {
    #[serde(rename = "type")]
    kind: String,
    tenant_id: String,
    call_id: String,
    event_type: String,
    occurred_at: String,
}

/// What one call event does to the island for that call.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub enum CallUpdate {
    Upsert(Box<Island>),
    Remove(String),
}

/// Remembers when each call started, since only `call.started` carries it.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default)]
pub struct CallTracker {
    started: HashMap<String, i64>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CallTracker {
    /// `None` for payloads that are not call events.
    pub fn handle(&mut self, payload: &str) -> Option<CallUpdate> {
        let ev: CallEvent = serde_json::from_str(payload).ok()?;
        if ev.kind != "call_event" {
            return None;
        }
        let key = format!("call:{}:{}", ev.tenant_id, ev.call_id);
        if ev.event_type == "call.ended" {
            self.started.remove(&key);
            return Some(CallUpdate::Remove(key));
        }
        let at = DateTime::parse_from_rfc3339(&ev.occurred_at)
            .map(|t| t.timestamp_millis())
            .unwrap_or_default();
        let started = *self.started.entry(key.clone()).or_insert(at);
        let severity = if ev.event_type.ends_with("_failed") || ev.event_type == "routing.no_agent"
        {
            Severity::Warning
        } else {
            Severity::Info
        };
        let island = Island::new(
            &key,
            Kind::Timer {
                started_at: started,
            },
            "Live call",
        )
        .subtitle(ev.event_type.clone())
        .field("Tenant", ev.tenant_id)
        .field("Call", ev.call_id)
        .icon("📞")
        .severity(severity)
        .sticky()
        .silent(severity == Severity::Info)
        .revision(ev.event_type);
        Some(CallUpdate::Upsert(Box::new(island)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event_type: &str, at: &str) -> String {
        serde_json::json!({
            "type": "call_event",
            "tenantId": "t1",
            "callId": "call-_-447700900000",
            "eventType": event_type,
            "occurredAt": at,
            "id": "x",
            "detail": {}
        })
        .to_string()
    }

    #[test]
    fn one_island_per_call() {
        let mut t = CallTracker::default();
        let Some(CallUpdate::Upsert(start)) =
            t.handle(&event("call.started", "2026-10-30T10:00:00Z"))
        else {
            panic!("expected an island");
        };
        assert!(start.silent);
        let Some(CallUpdate::Upsert(next)) =
            t.handle(&event("routing.no_agent", "2026-10-30T10:01:00Z"))
        else {
            panic!("expected an island");
        };
        assert_eq!(next.key, start.key);
        assert_eq!(next.kind, start.kind, "keeps the start time");
        assert_eq!(next.severity, Severity::Warning);
        assert!(!next.silent);
        assert_eq!(
            t.handle(&event("call.ended", "2026-10-30T10:02:00Z")),
            Some(CallUpdate::Remove(start.key.clone()))
        );
        assert!(t.handle(r#"{"type":"reservation_offer"}"#).is_none());
    }
}
