//! Structured agent-output chat projection (agentic-sandbox#600).
//!
//! Projects a runtime's `stream-json` output (Claude Code NDJSON) into a
//! normalized, message-oriented event stream for Chat clients such as AIWG
//! Cockpit. Raw PTY output stays authoritative; these events are a projection
//! with provenance back to the originating command stream.
//!
//! ## Wire compatibility with Fortemi
//!
//! The emitted frames follow the Fortemi `POST /api/v1/chat/stream` envelope
//! (`Fortemi/fortemi` `ChatStreamFrame`): named SSE events carrying a JSON
//! `data` object, with monotonic `{session}-{seq}` event ids attached at emit
//! time. The `delta` / `done` / `error` events carry the exact Fortemi fields
//! as a subset, so a Fortemi-only client consumes the assistant-text
//! projection unchanged. Richer agent events (`tool_call`, `tool_result`,
//! `status`, `raw`) are additive named events that a superset client reads and
//! a Fortemi-only client ignores. Convergence toward a shared, widely-usable
//! agent-chat schema is tracked in `Fortemi/fortemi`.

use serde::Serialize;
use serde_json::{json, Value};

/// Which structured-output source a session/runtime exposes for Chat.
///
/// Advertised on session capability so a client can choose Chat vs Terminal
/// without probing. `PtyParse` is reserved for the ADR's per-platform PTY
/// fallback; agentic-sandbox does not parse PTY server-side today, so
/// [`ChatSource::detect`] only returns `StreamJson` or `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatSource {
    /// Runtime emits machine-readable `stream-json` (Claude Code).
    StreamJson,
    /// Client must parse the PTY byte stream per platform (reserved).
    PtyParse,
    /// No structured chat projection available; use Terminal only.
    None,
}

impl std::fmt::Display for ChatSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatSource::StreamJson => write!(f, "stream-json"),
            ChatSource::PtyParse => write!(f, "pty-parse"),
            ChatSource::None => write!(f, "none"),
        }
    }
}

impl ChatSource {
    /// Detect the chat source from a session's command invocation.
    ///
    /// A Claude Code invocation running with `--output-format stream-json`
    /// emits NDJSON we can project. Everything else (interactive shells, Codex
    /// TUI, unknown runtimes) advertises `none`; Codex structured-output
    /// support is tracked as follow-up (agentic-sandbox#600 acceptance).
    pub fn detect(command: &str, args: &[String]) -> ChatSource {
        let mut invocation = command.to_ascii_lowercase();
        for arg in args {
            invocation.push(' ');
            invocation.push_str(&arg.to_ascii_lowercase());
        }
        let mentions_claude = invocation.contains("claude");
        let mentions_stream_json = invocation.contains("stream-json");
        if mentions_claude && mentions_stream_json {
            ChatSource::StreamJson
        } else {
            ChatSource::None
        }
    }

    pub fn is_available(self) -> bool {
        !matches!(self, ChatSource::None)
    }
}

/// A normalized chat frame in the Fortemi-compatible envelope.
///
/// `event` is the SSE event name; `data` is the JSON payload. `seq` is a
/// projector-local monotonic counter used to build the `{session}-{seq}` SSE
/// id at emit time and to drive `Last-Event-ID` cursor resume.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatStreamFrame {
    pub event: &'static str,
    pub data: Value,
    pub seq: u64,
}

impl ChatStreamFrame {
    /// Serialize the `data` payload to the compact JSON string an SSE `data:`
    /// line carries.
    pub fn data_string(&self) -> String {
        self.data.to_string()
    }

    /// Whether this frame terminates the stream (`done` or `error`).
    pub fn is_terminal(&self) -> bool {
        matches!(self.event, "done" | "error")
    }
}

const EVENT_DELTA: &str = "delta";
const EVENT_TOOL_CALL: &str = "tool_call";
const EVENT_TOOL_RESULT: &str = "tool_result";
const EVENT_STATUS: &str = "status";
const EVENT_DONE: &str = "done";
const EVENT_ERROR: &str = "error";
const EVENT_RAW: &str = "raw";

/// Stateful projector turning a `stream-json` byte stream into chat frames.
///
/// Reassembles NDJSON lines across chunk boundaries, so partial lines split by
/// transport framing are handled correctly. The projection is deterministic in
/// line order, which is what makes cursor-based replay (re-project the buffer,
/// skip `seq <= cursor`) safe.
pub struct StreamJsonProjector {
    session_id: String,
    command_id: String,
    /// Carries an incomplete trailing line between `push` calls.
    pending: String,
    /// Monotonic frame sequence, mirrored into the SSE id.
    next_seq: u64,
    /// Ordinal of the raw NDJSON line each frame derives from (provenance).
    next_line: u64,
    /// Model slug captured from the `system`/init line, echoed on `done`.
    model: Option<String>,
}

impl StreamJsonProjector {
    pub fn new(session_id: impl Into<String>, command_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            command_id: command_id.into(),
            pending: String::new(),
            next_seq: 0,
            next_line: 0,
            model: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Feed a raw byte chunk (UTF-8, lossily decoded) and return any completed
    /// frames. Bytes that do not complete a line are retained for the next call.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<ChatStreamFrame> {
        let text = String::from_utf8_lossy(bytes);
        self.push_str(&text)
    }

    /// Feed a text chunk and return any completed frames.
    pub fn push_str(&mut self, chunk: &str) -> Vec<ChatStreamFrame> {
        self.pending.push_str(chunk);
        let mut frames = Vec::new();
        // Drain every complete line; keep the trailing partial in `pending`.
        while let Some(newline) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            frames.extend(self.project_line(line));
        }
        frames
    }

    /// Flush a trailing line that arrived without a final newline (e.g. process
    /// exit). Call once when the source stream closes.
    pub fn finish(&mut self) -> Vec<ChatStreamFrame> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            return Vec::new();
        }
        let line = std::mem::take(&mut self.pending);
        self.project_line(line.trim_end_matches(['\n', '\r']))
    }

    fn raw_ref(&self, line: u64) -> Value {
        json!({ "command_id": self.command_id, "line": line })
    }

    fn frame(&mut self, event: &'static str, mut data: Value, line: u64) -> ChatStreamFrame {
        // Every frame carries session identity + provenance back to the raw
        // command stream (agentic-sandbox#600 acceptance: stable identity and
        // enough provenance to correlate with raw frames / command ids).
        if let Value::Object(map) = &mut data {
            map.entry("session_id")
                .or_insert_with(|| Value::String(self.session_id.clone()));
            map.entry("raw_ref").or_insert_with(|| self.raw_ref(line));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        ChatStreamFrame { event, data, seq }
    }

    fn project_line(&mut self, line: &str) -> Vec<ChatStreamFrame> {
        let line_no = self.next_line;
        self.next_line += 1;

        if line.trim().is_empty() {
            return Vec::new();
        }

        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Non-JSON output (e.g. a stray log line) is surfaced as `raw`
                // rather than dropped, preserving the ADR's "raw stays
                // available" guarantee even in the projection.
                return vec![self.frame(
                    EVENT_RAW,
                    json!({
                        "role": "system",
                        "kind": "raw",
                        "content": line,
                    }),
                    line_no,
                )];
            }
        };

        // Capture the model slug from the init line for the terminal `done`.
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            self.model = Some(model.to_string());
        }

        match value.get("type").and_then(Value::as_str) {
            Some("system") => self.project_system(&value, line_no),
            Some("assistant") => self.project_assistant(&value, line_no),
            Some("user") => self.project_user(&value, line_no),
            Some("result") => self.project_result(&value, line_no),
            _ => vec![self.frame(
                EVENT_RAW,
                json!({ "role": "system", "kind": "raw", "content": line }),
                line_no,
            )],
        }
    }

    fn project_system(&mut self, value: &Value, line: u64) -> Vec<ChatStreamFrame> {
        let subtype = value
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("system");
        vec![self.frame(
            EVENT_STATUS,
            json!({
                "role": "system",
                "kind": "message",
                "status": subtype,
                "content": format!("session {subtype}"),
            }),
            line,
        )]
    }

    fn project_assistant(&mut self, value: &Value, line: u64) -> Vec<ChatStreamFrame> {
        let mut frames = Vec::new();
        for block in content_blocks(value) {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }
                    // Fortemi-compatible `delta`: `content` is the exact field a
                    // Fortemi client reads; role/kind are superset extensions.
                    frames.push(self.frame(
                        EVENT_DELTA,
                        json!({
                            "role": "assistant",
                            "kind": "message",
                            "content": text,
                        }),
                        line,
                    ));
                }
                Some("tool_use") => {
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    frames.push(self.frame(
                        EVENT_TOOL_CALL,
                        json!({
                            "role": "assistant",
                            "kind": "tool_call",
                            "name": name,
                            "tool_id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                            "input": input,
                        }),
                        line,
                    ));
                }
                _ => {}
            }
        }
        frames
    }

    fn project_user(&mut self, value: &Value, line: u64) -> Vec<ChatStreamFrame> {
        let mut frames = Vec::new();
        for block in content_blocks(value) {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let tool_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            frames.push(self.frame(
                EVENT_TOOL_RESULT,
                json!({
                    "role": "tool",
                    "kind": "tool_result",
                    "tool_id": tool_id,
                    "status": if is_error { "error" } else { "ok" },
                    "content": tool_result_text(&block),
                }),
                line,
            ));
        }
        frames
    }

    fn project_result(&mut self, value: &Value, line: u64) -> Vec<ChatStreamFrame> {
        let subtype = value
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("success");
        let is_error = value
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(subtype != "success");
        let finish_reason = if is_error { "error" } else { "stop" };
        // Fortemi-compatible `done`: `finish_reason` + `model` are the exact
        // fields a Fortemi client reads; usage/kind are superset extensions.
        let mut data = json!({
            "role": "status",
            "kind": "usage",
            "finish_reason": finish_reason,
            "model": self.model.clone().unwrap_or_default(),
        });
        if let Value::Object(map) = &mut data {
            if let Some(usage) = value.get("usage") {
                map.insert("usage".into(), usage.clone());
            }
            if let Some(cost) = value.get("total_cost_usd") {
                map.insert("total_cost_usd".into(), cost.clone());
            }
            if let Some(result) = value.get("result").and_then(Value::as_str) {
                map.insert("content".into(), Value::String(result.to_string()));
            }
        }
        vec![self.frame(EVENT_DONE, data, line)]
    }
}

/// Extract the `message.content` array from an assistant/user line.
fn content_blocks(value: &Value) -> Vec<Value> {
    value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A `tool_result` block's content may be a string or an array of text blocks.
fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Build a terminal `error` frame in the Fortemi `STREAM_INTERRUPTED` shape.
///
/// Used when a resume cursor references an unknown/expired command, matching
/// Fortemi's contract so a shared client handles interruption identically.
pub fn stream_interrupted_frame(seq: u64) -> ChatStreamFrame {
    ChatStreamFrame {
        event: EVENT_ERROR,
        data: json!({
            "error": "stream interrupted before completion; resend the request to regenerate",
            "code": "STREAM_INTERRUPTED",
        }),
        seq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_all(lines: &[&str]) -> Vec<ChatStreamFrame> {
        let mut p = StreamJsonProjector::new("sess-1", "cmd-1");
        let mut out = Vec::new();
        for line in lines {
            out.extend(p.push_str(&format!("{line}\n")));
        }
        out.extend(p.finish());
        out
    }

    #[test]
    fn detect_claude_stream_json() {
        assert_eq!(
            ChatSource::detect("claude", &["--output-format".into(), "stream-json".into()]),
            ChatSource::StreamJson
        );
        assert_eq!(ChatSource::detect("bash", &["-l".into()]), ChatSource::None);
        assert_eq!(
            ChatSource::detect("codex", &["--tui".into()]),
            ChatSource::None
        );
    }

    #[test]
    fn chat_source_display_is_kebab() {
        assert_eq!(ChatSource::StreamJson.to_string(), "stream-json");
        assert_eq!(ChatSource::PtyParse.to_string(), "pty-parse");
        assert_eq!(ChatSource::None.to_string(), "none");
        assert!(ChatSource::StreamJson.is_available());
        assert!(!ChatSource::None.is_available());
    }

    #[test]
    fn assistant_text_projects_to_fortemi_delta() {
        let frames = project_all(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]}}"#,
        ]);
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(f.event, "delta");
        // Fortemi contract: delta data carries `content`.
        assert_eq!(f.data["content"], "Hello");
        // Superset extensions.
        assert_eq!(f.data["role"], "assistant");
        assert_eq!(f.data["kind"], "message");
        assert_eq!(f.data["session_id"], "sess-1");
        assert_eq!(f.data["raw_ref"]["command_id"], "cmd-1");
        assert_eq!(f.seq, 0);
    }

    #[test]
    fn tool_use_and_result_project_to_named_events() {
        let frames = project_all(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":false,"content":"file1\nfile2"}]}}"#,
        ]);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].event, "tool_call");
        assert_eq!(frames[0].data["name"], "Bash");
        assert_eq!(frames[0].data["tool_id"], "toolu_1");
        assert_eq!(frames[0].data["input"]["command"], "ls");
        assert_eq!(frames[1].event, "tool_result");
        assert_eq!(frames[1].data["tool_id"], "toolu_1");
        assert_eq!(frames[1].data["status"], "ok");
        assert_eq!(frames[1].data["content"], "file1\nfile2");
    }

    #[test]
    fn tool_result_array_content_flattens_to_text() {
        let frames = project_all(&[
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":true,"content":[{"type":"text","text":"boom"}]}]}}"#,
        ]);
        assert_eq!(frames[0].data["status"], "error");
        assert_eq!(frames[0].data["content"], "boom");
    }

    #[test]
    fn result_line_projects_to_fortemi_done() {
        let frames = project_all(&[
            r#"{"type":"system","subtype":"init","model":"claude-fable-5"}"#,
            r#"{"type":"result","subtype":"success","total_cost_usd":0.01,"usage":{"output_tokens":50},"result":"final"}"#,
        ]);
        // system -> status, result -> done
        assert_eq!(frames[0].event, "status");
        assert_eq!(frames[0].data["status"], "init");
        let done = &frames[1];
        assert_eq!(done.event, "done");
        // Fortemi contract: done carries finish_reason + model.
        assert_eq!(done.data["finish_reason"], "stop");
        assert_eq!(done.data["model"], "claude-fable-5");
        // Superset extensions.
        assert_eq!(done.data["usage"]["output_tokens"], 50);
        assert_eq!(done.data["total_cost_usd"], 0.01);
        assert_eq!(done.data["content"], "final");
        assert!(done.is_terminal());
    }

    #[test]
    fn error_result_maps_finish_reason_error() {
        let frames =
            project_all(&[r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#]);
        assert_eq!(frames[0].data["finish_reason"], "error");
    }

    #[test]
    fn non_json_line_becomes_raw_not_dropped() {
        let frames = project_all(&["not json at all"]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, "raw");
        assert_eq!(frames[0].data["content"], "not json at all");
        assert_eq!(frames[0].data["kind"], "raw");
    }

    #[test]
    fn partial_lines_reassemble_across_chunks() {
        let mut p = StreamJsonProjector::new("s", "c");
        // Split a single JSON line across three transport chunks.
        assert!(p
            .push_str(r#"{"type":"assistant","message":{"content":"#)
            .is_empty());
        assert!(p.push_str(r#"[{"type":"text","text":"hi"#).is_empty());
        let frames = p.push_str("\"}]}}\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data["content"], "hi");
    }

    #[test]
    fn seq_is_monotonic_across_frames() {
        let frames = project_all(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
        ]);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].seq, 0);
        assert_eq!(frames[1].seq, 1);
    }

    #[test]
    fn stream_interrupted_matches_fortemi_shape() {
        let f = stream_interrupted_frame(7);
        assert_eq!(f.event, "error");
        assert_eq!(f.data["code"], "STREAM_INTERRUPTED");
        assert_eq!(f.seq, 7);
        assert!(f.is_terminal());
    }
}
