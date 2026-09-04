//! In-memory ring buffer of recent tracing events.
//!
//! Backs the `GET /api/v1/logs` endpoint exposed to the dashboard's "System"
//! tab. Captures structured tracing events as they happen and keeps the most
//! recent N entries; older entries are evicted on overflow.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{OnceLock, RwLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const DEFAULT_CAPACITY: usize = 2000;

/// One captured tracing event, in a form suitable for JSON serialization.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    /// Monotonic hot-buffer identifier used for snapshot and SSE resume.
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub level: &'static str,
    pub target: String,
    pub message: String,
}

struct Buffer {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    last_id: u64,
}

static BUFFER: OnceLock<RwLock<Buffer>> = OnceLock::new();

fn buffer() -> &'static RwLock<Buffer> {
    BUFFER.get_or_init(|| {
        RwLock::new(Buffer {
            entries: VecDeque::with_capacity(DEFAULT_CAPACITY),
            capacity: DEFAULT_CAPACITY,
            last_id: 0,
        })
    })
}

/// Snapshot the most recent `limit` entries, newest first.
pub fn snapshot(limit: usize) -> Vec<LogEntry> {
    let buf = buffer().read().expect("log buffer poisoned");
    buf.entries.iter().rev().take(limit).cloned().collect()
}

/// Snapshot all entries newer than `since`, newest first.
pub fn snapshot_since(since: DateTime<Utc>, limit: usize) -> Vec<LogEntry> {
    let buf = buffer().read().expect("log buffer poisoned");
    buf.entries
        .iter()
        .rev()
        .filter(|e| e.timestamp > since)
        .take(limit)
        .cloned()
        .collect()
}

/// Snapshot retained entries after a cursor, oldest first. The boolean reports
/// that the requested cursor predates the bounded ring and some entries are no
/// longer available. The final value is the latest assigned cursor.
pub fn snapshot_after(cursor: u64, limit: usize) -> (Vec<LogEntry>, bool, u64) {
    let buf = buffer().read().expect("log buffer poisoned");
    let oldest = buf.entries.front().map(|entry| entry.id);
    let gap = oldest
        .map(|oldest_id| cursor.saturating_add(1) < oldest_id)
        .unwrap_or(false);
    let entries = buf
        .entries
        .iter()
        .filter(|entry| entry.id > cursor)
        .take(limit)
        .cloned()
        .collect();
    (entries, gap, buf.last_id)
}

pub fn last_id() -> u64 {
    buffer().read().expect("log buffer poisoned").last_id
}

fn push(mut entry: LogEntry) {
    if let Ok(mut buf) = buffer().write() {
        buf.last_id += 1;
        entry.id = buf.last_id;
        if buf.entries.len() == buf.capacity {
            buf.entries.pop_front();
        }
        buf.entries.push_back(entry);
    }
}

/// `tracing-subscriber` layer that captures every event into the ring buffer.
pub struct MemoryLayer;

impl<S: Subscriber> Layer<S> for MemoryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        push(LogEntry {
            id: 0,
            timestamp: Utc::now(),
            level: metadata.level().as_str(),
            target: metadata.target().to_string(),
            message: visitor.into_message(),
        });
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn into_message(self) -> String {
        let mut out = self.message.unwrap_or_default();
        for (k, v) in self.fields {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&k);
            out.push('=');
            out.push_str(&v);
        }
        out
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{:?}", value));
        } else {
            self.fields
                .push((field.name().to_string(), format!("{:?}", value)));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .push((field.name().to_string(), value.to_string()));
        }
    }
}
