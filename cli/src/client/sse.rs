//! Server-Sent Events client.
//!
//! Reads `text/event-stream` from `reqwest::Response::bytes_stream()` and
//! yields `SseEvent { event, data }` records. Used by `task logs --follow`
//! and `event tail`.
//!
//! SSE wire format we parse:
//! - `event: <name>\n` — the event type. Optional; defaults to "message".
//! - `data: <line>\n`  — one line of payload. May appear multiple times;
//!   lines are joined with "\n" per the SSE spec.
//! - empty line ⇒ event boundary; we yield the accumulated record.
//!
//! Comments (`: ...`) and `id:` / `retry:` fields are accepted-and-ignored.

use anyhow::{anyhow, Result};
use bytes::{Buf, Bytes, BytesMut};
use futures_util::stream::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::http::HttpClient;

#[derive(Debug, Clone, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buf: BytesMut,
    pending: SseEvent,
    pending_data: Vec<String>,
}

impl SseStream {
    /// Open an SSE stream against `path`. Adds bearer auth + reuses the
    /// HttpClient's reqwest::Client so connection pooling and TLS config
    /// match the rest of the CLI.
    pub async fn open(c: &HttpClient, path: &str) -> Result<Self> {
        let url = format!("{}{}", c.base(), path);
        let mut rb = c
            .inner_for_sse()
            .get(&url)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(tok) = c.bearer_token() {
            if !tok.is_empty() {
                rb = rb.bearer_auth(tok);
            }
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| anyhow!("SSE connect to {url} failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("SSE handshake failed ({status}): {body}"));
        }
        let stream = resp.bytes_stream();
        Ok(Self {
            inner: Box::pin(stream),
            buf: BytesMut::with_capacity(4096),
            pending: SseEvent::default(),
            pending_data: Vec::new(),
        })
    }

    fn try_emit(&mut self) -> Option<SseEvent> {
        // Find a complete record (terminated by an empty line).
        // Multi-line `data:` is concatenated with "\n".
        loop {
            let nl = self.buf.iter().position(|b| *b == b'\n')?;
            let line = self.buf.split_to(nl + 1);
            // Strip the trailing newline (and a preceding \r if CRLF).
            let mut s = &line[..line.len() - 1];
            if s.last() == Some(&b'\r') {
                s = &s[..s.len() - 1];
            }
            if s.is_empty() {
                // Boundary — emit if we have accumulated data.
                if self.pending_data.is_empty() && self.pending.event.is_none() {
                    continue;
                }
                let mut out = std::mem::take(&mut self.pending);
                out.data = std::mem::take(&mut self.pending_data).join("\n");
                return Some(out);
            }
            if s.starts_with(b":") {
                continue; // comment
            }
            // field:[ ]value  (the optional space after the colon is stripped).
            if let Some(colon) = s.iter().position(|b| *b == b':') {
                let field = &s[..colon];
                let mut value = &s[colon + 1..];
                if value.first() == Some(&b' ') {
                    value = &value[1..];
                }
                let value_str = String::from_utf8_lossy(value).to_string();
                match field {
                    b"event" => self.pending.event = Some(value_str),
                    b"data" => self.pending_data.push(value_str),
                    // id/retry: accept silently
                    _ => {}
                }
            }
        }
    }
}

impl Stream for SseStream {
    type Item = Result<SseEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(ev) = this.try_emit() {
                return Poll::Ready(Some(Ok(ev)));
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow!("SSE transport error: {e}"))));
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    this.buf.extend_from_slice(chunk.chunk());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    fn build(stream: Vec<Bytes>) -> SseStream {
        let inner = stream::iter(stream.into_iter().map(reqwest::Result::Ok));
        SseStream {
            inner: Box::pin(inner),
            buf: BytesMut::new(),
            pending: SseEvent::default(),
            pending_data: Vec::new(),
        }
    }

    #[tokio::test]
    async fn parses_simple_data_only_event() {
        use futures_util::StreamExt;
        let mut s = build(vec![Bytes::from_static(b"data: hello\n\n")]);
        let ev = s.next().await.unwrap().unwrap();
        assert!(ev.event.is_none());
        assert_eq!(ev.data, "hello");
    }

    #[tokio::test]
    async fn parses_multi_line_data() {
        use futures_util::StreamExt;
        let mut s = build(vec![Bytes::from_static(
            b"event: lagged\ndata: line1\ndata: line2\n\n",
        )]);
        let ev = s.next().await.unwrap().unwrap();
        assert_eq!(ev.event.as_deref(), Some("lagged"));
        assert_eq!(ev.data, "line1\nline2");
    }

    #[tokio::test]
    async fn ignores_comments_and_unknown_fields() {
        use futures_util::StreamExt;
        let mut s = build(vec![Bytes::from_static(
            b": keepalive\nid: 42\nretry: 1000\ndata: ok\n\n",
        )]);
        let ev = s.next().await.unwrap().unwrap();
        assert_eq!(ev.data, "ok");
    }
}
