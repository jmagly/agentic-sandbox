//! Normalized structured agent-output stream for Chat clients (agentic-sandbox#600).
//!
//! `GET /api/v1/agent-output/chat` projects a command's `stream-json` output
//! into a message/tool/status event stream in the Fortemi
//! `POST /api/v1/chat/stream` SSE envelope (see [`crate::output::ChatStreamFrame`]).
//! It is a read-only projection: subscribing confers no controller input
//! authority, and the raw PTY/output stream (`/api/v1/agent-output/stream`)
//! stays authoritative.
//!
//! Event ids are `{session}-{seq}`; a `Last-Event-ID` header resumes after that
//! cursor by re-projecting the buffered command output (deterministic in line
//! order) and skipping already-seen frames. An unknown/expired command on the
//! resume path terminates with a Fortemi `STREAM_INTERRUPTED` error rather than
//! hanging.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use serde::Deserialize;

use super::server::AppState;
use crate::output::{
    stream_interrupted_frame, ChatStreamFrame, OutputMessage, OutputRecvError, StreamJsonProjector,
    StreamType,
};

#[derive(Debug, Deserialize)]
pub struct AgentChatQuery {
    /// Command id whose `stream-json` output to project. Required — the
    /// projection is stateful per command.
    command_id: String,
    /// Override the session id used in `{session}-{seq}` ids. Defaults to the
    /// formal session id mapped from `command_id`, else `command_id`.
    session_id: Option<String>,
    /// Replay buffered output before following live.
    replay: Option<bool>,
}

/// Stream normalized chat events as Server-Sent Events.
pub async fn stream_agent_chat(
    State(state): State<AppState>,
    Query(query): Query<AgentChatQuery>,
    headers: HeaderMap,
) -> Response {
    let command_id = query.command_id.clone();
    if command_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "command_id is required" })),
        )
            .into_response();
    }

    // Resolve a stable session identity for the `{session}-{seq}` id space.
    let session_id = query
        .session_id
        .clone()
        .or_else(|| state.dispatcher.session_id_for_command(&command_id))
        .unwrap_or_else(|| command_id.clone());

    let resume_cursor = parse_last_event_id(&headers, &session_id);
    let replay = query.replay.unwrap_or(false) || resume_cursor.is_some();

    // Subscribe before snapshotting the buffer so live frames arriving during
    // replay serialization are not missed. Live chunks already present in the
    // snapshot are de-duplicated below so the projector consumes each raw line
    // exactly once and `seq` stays stable for cursor resume.
    let mut subscription = state.output_agg.subscribe(None, Some(StreamType::Stdout));
    let buffered = state.output_agg.get_buffered(&command_id);

    // Unknown/expired command on the resume path: terminate like Fortemi.
    if resume_cursor.is_some() && buffered.is_empty() {
        let frame = stream_interrupted_frame(0);
        let stream = async_stream::stream! {
            yield Ok::<_, std::convert::Infallible>(sse_event(&frame, &session_id));
        };
        return Sse::new(stream).into_response();
    }

    let mut projector = StreamJsonProjector::new(session_id.clone(), command_id.clone());
    let cursor = resume_cursor.map(|(_, seq)| seq);

    // Project the buffered prefix (if replaying) and record which raw chunks it
    // covered, so overlapping live chunks are skipped exactly.
    let (replay_frames, mut seen) = if replay {
        project_buffered(&mut projector, &buffered, &command_id, cursor)
    } else {
        (Vec::new(), HashSet::new())
    };

    let session_for_stream = session_id.clone();
    let stream = async_stream::stream! {
        for frame in replay_frames {
            yield Ok::<_, std::convert::Infallible>(sse_event(&frame, &session_for_stream));
        }

        loop {
            match subscription.recv_with_policy().await {
                Ok(Some(msg)) => {
                    if msg.command_id != command_id {
                        continue;
                    }
                    // Skip chunks already covered by the replay snapshot so the
                    // projector never double-consumes a raw line.
                    if !seen.is_empty() && seen.remove(&fingerprint(&msg)) {
                        continue;
                    }
                    for frame in projector.push_bytes(&msg.data) {
                        yield Ok(sse_event(&frame, &session_for_stream));
                    }
                }
                Ok(None) => {
                    // Source closed: flush any trailing partial line.
                    for frame in projector.finish() {
                        yield Ok(sse_event(&frame, &session_for_stream));
                    }
                    break;
                }
                Err(OutputRecvError::SlowSubscriber { .. }) => {
                    let frame = stream_interrupted_frame(u64::MAX);
                    yield Ok(sse_event(&frame, &session_for_stream));
                    break;
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Build the SSE event for a frame, attaching the `{session}-{seq}` id.
fn sse_event(frame: &ChatStreamFrame, session_id: &str) -> Event {
    Event::default()
        .event(frame.event)
        .id(format!("{session_id}-{}", frame.seq))
        .data(frame.data_string())
}

/// Parse a `Last-Event-ID` of the form `{session}-{seq}` into its cursor.
///
/// Mirrors Fortemi's `rfind('-')` split so a malformed cursor (non-numeric
/// sequence) is rejected rather than silently treated as zero.
fn parse_last_event_id(headers: &HeaderMap, session_id: &str) -> Option<(String, u64)> {
    let raw = headers.get("last-event-id")?.to_str().ok()?;
    let (sess, seq) = raw.rsplit_once('-')?;
    let seq: u64 = seq.parse().ok()?;
    // A cursor for a different session cannot resume this projection.
    if sess != session_id {
        return None;
    }
    Some((sess.to_string(), seq))
}

/// Feed the buffered command prefix through the projector, returning the frames
/// after the resume `cursor` and the fingerprint set of consumed raw chunks
/// (used to de-duplicate the replay/live overlap window). Advances `projector`
/// so the live loop continues with a stable, monotonic `seq`.
fn project_buffered(
    projector: &mut StreamJsonProjector,
    buffered: &[OutputMessage],
    command_id: &str,
    cursor: Option<u64>,
) -> (Vec<ChatStreamFrame>, HashSet<u64>) {
    let mut seen = HashSet::with_capacity(buffered.len());
    let mut frames = Vec::new();
    for msg in buffered {
        if msg.command_id != command_id {
            continue;
        }
        seen.insert(fingerprint(msg));
        for frame in projector.push_bytes(&msg.data) {
            if cursor.map_or(true, |c| frame.seq > c) {
                frames.push(frame);
            }
        }
    }
    (frames, seen)
}

/// Cheap identity for an output chunk, used to de-duplicate the replay/live
/// overlap window (bounded buffer, so collisions are irrelevant in practice).
fn fingerprint(msg: &OutputMessage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    msg.timestamp.hash(&mut hasher);
    msg.data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn last_event_id_parses_matching_session_cursor() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("sess-1-5"));
        assert_eq!(
            parse_last_event_id(&headers, "sess-1"),
            Some(("sess-1".to_string(), 5))
        );
    }

    #[test]
    fn last_event_id_rejects_malformed_sequence() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("sess-1-notnum"));
        assert_eq!(parse_last_event_id(&headers, "sess-1"), None);
    }

    #[test]
    fn last_event_id_rejects_foreign_session() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("other-9"));
        assert_eq!(parse_last_event_id(&headers, "sess-1"), None);
    }

    #[test]
    fn last_event_id_handles_hyphenated_session() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("sess-a-b-3"));
        assert_eq!(
            parse_last_event_id(&headers, "sess-a-b"),
            Some(("sess-a-b".to_string(), 3))
        );
    }

    #[test]
    fn replay_projects_buffered_aggregator_output_end_to_end() {
        use crate::output::OutputAggregator;
        // A stream-json command's stdout, split across two transport chunks.
        let agg = OutputAggregator::new(64);
        agg.push(
            "agent".into(),
            "cmd-1".into(),
            StreamType::Stdout,
            br#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"#.to_vec(),
        );
        agg.push(
            "agent".into(),
            "cmd-1".into(),
            StreamType::Stdout,
            b"\"}]}}\n".to_vec(),
        );
        // Output for a different command must not leak into this projection.
        agg.push(
            "agent".into(),
            "other".into(),
            StreamType::Stdout,
            b"{\"type\":\"system\",\"subtype\":\"init\"}\n".to_vec(),
        );

        let buffered = agg.get_buffered("cmd-1");
        let mut projector = StreamJsonProjector::new("sess-1", "cmd-1");
        let (frames, seen) = project_buffered(&mut projector, &buffered, "cmd-1", None);

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, "delta");
        assert_eq!(frames[0].data["content"], "hi");
        assert_eq!(frames[0].data["session_id"], "sess-1");
        // Both chunks of cmd-1 were consumed (dedup set), the `other` chunk was not.
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn replay_honors_resume_cursor() {
        use crate::output::OutputAggregator;
        let agg = OutputAggregator::new(64);
        agg.push(
            "agent".into(),
            "cmd-1".into(),
            StreamType::Stdout,
            br#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"},{"type":"text","text":"c"}]}}"#
                .to_vec(),
        );
        agg.push(
            "agent".into(),
            "cmd-1".into(),
            StreamType::Stdout,
            b"\n".to_vec(),
        );

        let buffered = agg.get_buffered("cmd-1");
        let mut projector = StreamJsonProjector::new("sess-1", "cmd-1");
        // Resume after seq 0 → only the b/c deltas (seq 1, 2) replay.
        let (frames, _) = project_buffered(&mut projector, &buffered, "cmd-1", Some(0));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data["content"], "b");
        assert_eq!(frames[0].seq, 1);
        assert_eq!(frames[1].data["content"], "c");
        assert_eq!(frames[1].seq, 2);
    }

    #[test]
    fn fingerprint_is_stable_and_distinguishes_chunks() {
        let a = OutputMessage {
            agent_id: "a".into(),
            command_id: "c".into(),
            stream_type: StreamType::Stdout,
            data: b"one".to_vec(),
            timestamp: 1,
        };
        let a2 = a.clone();
        let b = OutputMessage {
            data: b"two".to_vec(),
            ..a.clone()
        };
        assert_eq!(fingerprint(&a), fingerprint(&a2));
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }
}
