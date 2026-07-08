//! Structured agent-output SSE stream.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::server::AppState;
use crate::output::{OutputMessage, OutputRecvError, StreamType};

const AGENT_OUTPUT_SCHEMA: &str = "agentic.agent_output.v1";
const DEFAULT_REPLAY_LIMIT: usize = 200;
const MAX_REPLAY_LIMIT: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct AgentOutputQuery {
    /// Optional exact agent id filter.
    agent_id: Option<String>,
    /// Optional exact command id filter. Required for replay because the
    /// aggregator retains bounded buffers per command id.
    command_id: Option<String>,
    /// Optional stream filter: stdout, stderr, or log.
    stream: Option<String>,
    /// Replay buffered output for `command_id` before following live output.
    replay: Option<bool>,
    /// Maximum replayed chunks. Live output is unaffected.
    limit: Option<usize>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AgentOutputEvent {
    pub schema: &'static str,
    pub event_type: &'static str,
    pub agent_id: String,
    pub command_id: String,
    pub stream: String,
    pub timestamp_ms: i64,
    pub data_base64: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
struct AgentOutputErrorEvent {
    schema: &'static str,
    event_type: &'static str,
    error: &'static str,
    subscriber_id: String,
    dropped: u64,
}

/// Stream structured agent output as Server-Sent Events.
pub async fn stream_agent_output(
    State(state): State<AppState>,
    Query(query): Query<AgentOutputQuery>,
) -> Response {
    let stream_filter = match parse_stream_filter(query.stream.as_deref()) {
        Ok(filter) => filter,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": message })),
            )
                .into_response();
        }
    };

    if query.replay.unwrap_or(false) && query.command_id.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "command_id is required when replay=true"
            })),
        )
            .into_response();
    }

    let command_filter = query.command_id.clone();
    let agent_filter = query.agent_id.clone();
    let replay = query.replay.unwrap_or(false);
    let replay_limit = query
        .limit
        .unwrap_or(DEFAULT_REPLAY_LIMIT)
        .min(MAX_REPLAY_LIMIT);

    // Subscribe before replay so a reconnecting client cannot miss live chunks
    // that arrive while the bounded replay window is being serialized.
    let mut subscription = state.output_agg.subscribe(agent_filter, stream_filter);
    let output_agg = state.output_agg.clone();

    let stream = async_stream::stream! {
        if replay {
            if let Some(command_id) = command_filter.as_deref() {
                let buffered = output_agg.get_buffered(command_id);
                let matching: Vec<OutputMessage> = buffered
                    .into_iter()
                    .filter(|msg| message_matches(msg, command_filter.as_deref()))
                    .filter(|msg| stream_filter.map_or(true, |stream| msg.stream_type == stream))
                    .collect();
                let start = matching.len().saturating_sub(replay_limit);
                for msg in matching.into_iter().skip(start) {
                    if let Some(event) = output_sse_event(&msg, "agent_output.replay") {
                        yield Ok::<_, std::convert::Infallible>(event);
                    }
                }
            }
        }

        loop {
            match subscription.recv_with_policy().await {
                Ok(Some(msg)) => {
                    if !message_matches(&msg, command_filter.as_deref()) {
                        continue;
                    }
                    if let Some(event) = output_sse_event(&msg, "agent_output") {
                        yield Ok(event);
                    }
                }
                Ok(None) => break,
                Err(OutputRecvError::SlowSubscriber { subscriber_id, dropped }) => {
                    let error = AgentOutputErrorEvent {
                        schema: AGENT_OUTPUT_SCHEMA,
                        event_type: "slow_subscriber",
                        error: "subscriber_lagged",
                        subscriber_id,
                        dropped,
                    };
                    if let Ok(data) = serde_json::to_string(&error) {
                        yield Ok(Event::default().event("agent_output.error").data(data));
                    }
                    break;
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn parse_stream_filter(value: Option<&str>) -> Result<Option<StreamType>, String> {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") => Ok(None),
        Some("stdout") => Ok(Some(StreamType::Stdout)),
        Some("stderr") => Ok(Some(StreamType::Stderr)),
        Some("log") => Ok(Some(StreamType::Log)),
        Some(other) => Err(format!(
            "invalid stream '{}'; expected stdout, stderr, or log",
            other
        )),
    }
}

fn message_matches(msg: &OutputMessage, command_filter: Option<&str>) -> bool {
    command_filter.map_or(true, |command_id| msg.command_id == command_id)
}

fn output_sse_event(msg: &OutputMessage, event_name: &'static str) -> Option<Event> {
    serde_json::to_string(&agent_output_event(msg))
        .ok()
        .map(|data| Event::default().event(event_name).data(data))
}

fn agent_output_event(msg: &OutputMessage) -> AgentOutputEvent {
    AgentOutputEvent {
        schema: AGENT_OUTPUT_SCHEMA,
        event_type: "chunk",
        agent_id: msg.agent_id.clone(),
        command_id: msg.command_id.clone(),
        stream: msg.stream_type.to_string(),
        timestamp_ms: msg.timestamp,
        data_base64: base64::engine::general_purpose::STANDARD.encode(&msg.data),
        text: String::from_utf8_lossy(&msg.data).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_filter_accepts_supported_streams() {
        assert_eq!(parse_stream_filter(None).unwrap(), None);
        assert_eq!(
            parse_stream_filter(Some("stdout")).unwrap(),
            Some(StreamType::Stdout)
        );
        assert_eq!(
            parse_stream_filter(Some(" STDERR ")).unwrap(),
            Some(StreamType::Stderr)
        );
        assert_eq!(
            parse_stream_filter(Some("log")).unwrap(),
            Some(StreamType::Log)
        );
    }

    #[test]
    fn stream_filter_rejects_unknown_streams() {
        let error = parse_stream_filter(Some("metrics")).unwrap_err();
        assert!(error.contains("expected stdout, stderr, or log"));
    }

    #[test]
    fn event_preserves_raw_bytes_and_readable_text() {
        let msg = OutputMessage {
            agent_id: "agent-1".to_string(),
            command_id: "cmd-1".to_string(),
            stream_type: StreamType::Stdout,
            data: b"hello\xff".to_vec(),
            timestamp: 42,
        };

        let event = agent_output_event(&msg);

        assert_eq!(event.schema, AGENT_OUTPUT_SCHEMA);
        assert_eq!(event.event_type, "chunk");
        assert_eq!(event.agent_id, "agent-1");
        assert_eq!(event.command_id, "cmd-1");
        assert_eq!(event.stream, "stdout");
        assert_eq!(event.timestamp_ms, 42);
        assert_eq!(event.data_base64, "aGVsbG//");
        assert!(event.text.starts_with("hello"));
    }
}
