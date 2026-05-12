//! `tasks` — A2A core operations against a specific executor instance.
//!
//! Endpoints (all under `/agents/{instance_id}/v1/...` on the executor):
//! - `POST /messages:send`                  ← `tasks send <inst> <file|->`
//! - `GET  /tasks?state=&cursor=&limit=`    ← `tasks list <inst> ...`
//! - `GET  /tasks/{tid}`                    ← `tasks get <inst> <tid>`
//! - `GET  /tasks/{tid}/subscribe` (SSE)    ← `tasks subscribe <inst> <tid>`
//! - `POST /tasks/{tid}/cancel`             ← `tasks cancel <inst> <tid>`
//!
//! All mutating ops set the required A2A extension headers
//! (runtime/v1 + idempotency/v1) per #236.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;

use crate::client::http::HttpClient;

/// A2A extensions required on every mutating request to the executor.
/// The runtime/v1 extension carries the runtime kind; idempotency/v1 carries
/// the operator's `Idempotency-Key` if any. Both are mandatory per #236
/// even when payloads omit them — the gate middleware checks the header.
pub const REQUIRED_EXTENSIONS: &str = concat!(
    "https://agentic-sandbox.aiwg.io/extensions/runtime/v1, ",
    "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1"
);

/// `tasks send <instance_id> <message-file>` — POST a Message envelope to
/// `messages:send` and print the resulting `task_id`.
pub async fn send(
    c: &HttpClient,
    instance_id: &str,
    message_source: &str,
    as_json: bool,
) -> Result<()> {
    let body = read_json_input(message_source)?;
    let path = format!("/agents/{}/v1/messages:send", instance_id);
    let v = post_with_extensions(c, &path, &body).await?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        // Server may return a Task envelope directly or a Message wrapping
        // a task pointer. Surface both shapes.
        let tid = task_id_from_response(&v).unwrap_or_else(|| "-".to_string());
        let state = v
            .get("status")
            .and_then(|s| s.get("state"))
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        println!("task_id: {}", tid);
        println!("state:   {}", state);
    }
    Ok(())
}

/// `tasks list <instance_id> [--state] [--cursor] [--limit]`.
pub async fn list(
    c: &HttpClient,
    instance_id: &str,
    state: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    if let Some(c_) = cursor {
        q.push(("cursor".into(), c_.into()));
    }
    if let Some(l) = limit {
        q.push(("limit".into(), l.to_string()));
    }
    let path = crate::cmd::with_query(&format!("/agents/{}/v1/tasks", instance_id), &q);
    let v = c.get_value(&path).await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    // Render a compact table.
    let arr = v
        .get("tasks")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    println!("{:<40} {:<12} {:<24}", "TASK_ID", "STATE", "CREATED_AT");
    for t in &arr {
        let id = t.get("id").and_then(|x| x.as_str()).unwrap_or("-");
        let state = t
            .get("status")
            .and_then(|s| s.get("state"))
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        let created = t
            .get("status")
            .and_then(|s| s.get("timestamp"))
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        println!("{:<40} {:<12} {:<24}", id, state, created);
    }
    if let Some(next) = v.get("next_cursor").and_then(|x| x.as_str()) {
        println!("\nnext_cursor: {}", next);
    }
    Ok(())
}

/// `tasks get <instance_id> <task_id>`.
pub async fn get(c: &HttpClient, instance_id: &str, task_id: &str, as_json: bool) -> Result<()> {
    let path = format!("/agents/{}/v1/tasks/{}", instance_id, task_id);
    let v = c.get_value(&path).await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}

/// `tasks cancel <instance_id> <task_id>` — POST with no body.
pub async fn cancel(c: &HttpClient, instance_id: &str, task_id: &str, as_json: bool) -> Result<()> {
    let path = format!("/agents/{}/v1/tasks/{}/cancel", instance_id, task_id);
    let body = Value::Object(Default::default());
    let v = post_with_extensions(c, &path, &body).await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        let state = v
            .get("status")
            .and_then(|s| s.get("state"))
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        println!("task_id: {}", task_id);
        println!("state:   {}", state);
    }
    Ok(())
}

/// `tasks subscribe <instance_id> <task_id>` — open the SSE stream and
/// print each `data:` payload as one line. Exits when the connection
/// closes or a terminal task state is observed.
pub async fn subscribe(c: &HttpClient, instance_id: &str, task_id: &str) -> Result<()> {
    let url = format!(
        "{}/agents/{}/v1/tasks/{}/subscribe",
        c.base(),
        instance_id,
        task_id
    );
    let mut rb = c
        .inner_for_sse()
        .get(&url)
        .header("Accept", "text/event-stream");
    if let Some(tok) = c.bearer_token() {
        rb = rb.bearer_auth(tok);
    }
    let resp = rb.send().await.context("connect to SSE stream")?;
    if !resp.status().is_success() {
        let st = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("subscribe failed: HTTP {}: {}", st, body));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("SSE read")?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        // Process complete events separated by a blank line.
        while let Some(end) = buf.find("\n\n") {
            let frame = buf[..end].to_string();
            buf.drain(..end + 2);
            if let Some(data) = parse_sse_data(&frame) {
                // Try to parse the data as JSON and print compactly.
                match serde_json::from_str::<Value>(&data) {
                    Ok(v) => {
                        let st = v
                            .get("status")
                            .and_then(|s| s.get("state"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        println!("{}", serde_json::to_string(&v).unwrap_or(data.clone()));
                        if is_terminal_state(st) {
                            return Ok(());
                        }
                    }
                    Err(_) => {
                        println!("{}", data);
                    }
                }
            }
        }
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Read JSON from a file or stdin (when path is `-`).
fn read_json_input(source: &str) -> Result<Value> {
    let s = if source == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("read stdin")?;
        s
    } else {
        let p = PathBuf::from(source);
        std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?
    };
    serde_json::from_str(&s).context("parse message JSON")
}

/// POST `body` to `path` with the required A2A extension header set.
async fn post_with_extensions(c: &HttpClient, path: &str, body: &Value) -> Result<Value> {
    let url = format!("{}{}", c.base(), path);
    let mut rb = c
        .inner_for_sse()
        .post(&url)
        .header("A2A-Extensions", REQUIRED_EXTENSIONS)
        .header("Content-Type", "application/json")
        .json(body);
    if let Some(tok) = c.bearer_token() {
        rb = rb.bearer_auth(tok);
    }
    let resp = rb.send().await.context("send request")?;
    let status = resp.status();
    let text = resp.text().await.context("read response body")?;
    if !status.is_success() {
        return Err(anyhow!("HTTP {}: {}", status, text));
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).context("parse response JSON")
}

/// Extract a Task ID from a `messages:send` response. The server may return
/// a Task envelope directly (`{ "id": ..., "status": ... }`) or a Message
/// referencing a `task_id` field. We tolerate both shapes.
fn task_id_from_response(v: &Value) -> Option<String> {
    if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
        return Some(id.to_string());
    }
    if let Some(id) = v.get("task_id").and_then(|x| x.as_str()) {
        return Some(id.to_string());
    }
    None
}

/// Parse an SSE frame for its `data:` line(s). Per RFC: multiple `data:`
/// lines are concatenated with `\n`. We strip a single leading space if
/// present (also per spec).
fn parse_sse_data(frame: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for line in frame.split('\n') {
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            out.push(rest.to_string());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

fn is_terminal_state(s: &str) -> bool {
    matches!(
        s,
        "completed" | "failed" | "canceled" | "cancelled" | "rejected"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_client(url: &str) -> HttpClient {
        use crate::config::ContextEntry;
        HttpClient::new(&ContextEntry {
            server: url.to_string(),
            token: "test-token".into(),
            role: "operator".into(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn tasks_send_sets_required_extensions_header() {
        let server = MockServer::start().await;
        // The colon in `messages:send` survives reqwest's URL pipeline
        // un-encoded, but wiremock's `path` matcher percent-decodes the
        // incoming path before comparison. We assert on the full set of
        // headers separately via `received_requests()` below to keep this
        // mock authoritative without over-constraining the matcher.
        Mock::given(method("POST"))
            .and(path("/agents/inst-1/v1/messages:send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-abc",
                "status": { "state": "submitted", "timestamp": "2026-05-11T00:00:00Z" }
            })))
            .mount(&server)
            .await;

        // Stage a message file.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let msg = serde_json::json!({"message": {"role": "user", "parts": []}});
        std::fs::write(tmp.path(), msg.to_string()).unwrap();

        let c = test_client(&server.uri());
        let res = send(&c, "inst-1", tmp.path().to_str().unwrap(), true).await;
        assert!(res.is_ok(), "send: {:?}", res.err());

        // Verify the recorded request carried our required headers.
        let reqs = server.received_requests().await.expect("requests recorded");
        let req = reqs
            .iter()
            .find(|r| r.method == wiremock::http::Method::POST)
            .unwrap();
        let ext = req.headers.get("a2a-extensions").expect("header present");
        assert_eq!(ext.to_str().unwrap(), REQUIRED_EXTENSIONS);
        let auth = req.headers.get("authorization").expect("auth header");
        assert_eq!(auth.to_str().unwrap(), "Bearer test-token");
    }

    #[tokio::test]
    async fn tasks_get_returns_task_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/agents/inst-1/v1/tasks/task-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-abc",
                "status": { "state": "completed" }
            })))
            .mount(&server)
            .await;
        let c = test_client(&server.uri());
        assert!(get(&c, "inst-1", "task-abc", true).await.is_ok());
    }

    #[tokio::test]
    async fn tasks_cancel_409_when_terminal() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/agents/inst-1/v1/tasks/task-abc/cancel"))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_string(r#"{"error":"task already in terminal state"}"#),
            )
            .mount(&server)
            .await;
        let c = test_client(&server.uri());
        let r = cancel(&c, "inst-1", "task-abc", true).await;
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(
            msg.contains("409") || msg.contains("terminal"),
            "msg: {}",
            msg
        );
    }

    #[test]
    fn parse_sse_data_strips_prefix_and_joins() {
        let frame = "event: task\ndata: {\"id\":\"x\"}";
        assert_eq!(parse_sse_data(frame).unwrap(), r#"{"id":"x"}"#);

        let multi = "data: line1\ndata: line2";
        assert_eq!(parse_sse_data(multi).unwrap(), "line1\nline2");

        assert!(parse_sse_data("event: ping").is_none());
    }

    #[test]
    fn tasks_subscribe_parses_sse_chunks() {
        // Simulate the chunk-buffering protocol that `subscribe()` uses.
        // We don't fire up a network stream here; we just verify that
        // SSE frame parsing splits on `\n\n` correctly and decodes the
        // payload into a Task state.
        let mut buf = String::new();
        buf.push_str("data: {\"id\":\"t1\",\"status\":{\"state\":\"working\"}}\n\n");
        buf.push_str("data: {\"id\":\"t1\",\"status\":{\"state\":\"completed\"}}\n\n");

        let mut frames: Vec<String> = Vec::new();
        while let Some(end) = buf.find("\n\n") {
            let frame = buf[..end].to_string();
            buf.drain(..end + 2);
            if let Some(d) = parse_sse_data(&frame) {
                frames.push(d);
            }
        }
        assert_eq!(frames.len(), 2);
        let v: Value = serde_json::from_str(&frames[1]).unwrap();
        assert_eq!(v["status"]["state"].as_str().unwrap(), "completed");
        assert!(is_terminal_state("completed"));
        assert!(!is_terminal_state("working"));
    }

    #[test]
    fn task_id_from_response_tolerates_both_shapes() {
        assert_eq!(
            task_id_from_response(&serde_json::json!({"id": "abc"})),
            Some("abc".to_string())
        );
        assert_eq!(
            task_id_from_response(&serde_json::json!({"task_id": "def"})),
            Some("def".to_string())
        );
        assert_eq!(task_id_from_response(&serde_json::json!({})), None);
    }
}
