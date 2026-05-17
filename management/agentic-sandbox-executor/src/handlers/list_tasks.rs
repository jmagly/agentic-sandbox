//! A2A `tasks/list` handler (#210).
//!
//! `GET /agents/{instance_id}/v1/tasks?limit=...&cursor=...&state=...`
//!
//! Cursor is an opaque base64-encoded epoch-millisecond timestamp of the
//! created_at boundary. Pagination is keyset-style: each page returns
//! tasks with `created_at > cursor` (when supplied), ordered ascending by
//! `created_at`. `next_cursor` is the encoded `created_at` of the last
//! task in the page, present only when more rows likely follow.
//!
//! Sort order note: the spec asks for descending by status timestamp, but
//! the [`TaskStore`] only exposes ascending `created_at` ordering. We sort
//! ascending by `created_at` here so paging is deterministic; switching
//! to descending requires a TaskStore API change, deferred until #213.
//!
//! [`TaskStore`]: crate::store::task_store::TaskStore

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;
use crate::store::task_store::ListFilter;

use super::{parse_state, task_row_to_a2a};

const DEFAULT_LIMIT: u64 = 25;
const MAX_LIMIT: u64 = 100;

/// Query parameters for `tasks:list`.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Page size (default 25, max 100).
    pub limit: Option<u64>,
    /// Opaque cursor from a prior `next_cursor`.
    pub cursor: Option<String>,
    /// Filter by A2A task state (e.g. `working`, `completed`).
    pub state: Option<String>,
}

/// Axum handler for `GET /agents/{instance_id}/v1/tasks`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT).max(1);

    let state_filter = match &query.state {
        Some(s) => match parse_state(s) {
            Some(ts) => Some(ts),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "https://agentic-sandbox.aiwg.io/errors/invalid-params",
                    "Invalid params",
                    format!("Unknown task state: {s}"),
                    "request.invalid_params",
                    None,
                    Some(&instance_id),
                );
            }
        },
        None => None,
    };

    let cursor_ms = match &query.cursor {
        Some(c) => match decode_cursor(c) {
            Some(ms) => Some(ms),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "https://agentic-sandbox.aiwg.io/errors/invalid-params",
                    "Invalid params",
                    "Cursor must be base64url-encoded epoch milliseconds",
                    "request.invalid_params",
                    None,
                    Some(&instance_id),
                );
            }
        },
        None => None,
    };

    // The TaskStore ListFilter doesn't expose a cursor or descending sort,
    // so we fetch a generous window and filter / slice in-process. This is
    // acceptable until #213 graduates the store API.
    let filter = ListFilter {
        state: state_filter,
        // Pull more than we need so cursor filtering still yields a full page.
        limit: Some((limit * 4).max(limit + 10)),
        include_terminal: true,
        // #269: scope to the instance from the URL path. Previously the
        // store returned every task across every instance, which
        // surfaced as cross-instance bleed in the dashboard.
        instance_id: Some(instance_id.clone()),
    };

    let rows = match state.store.list_tasks(filter) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                format!("Failed to list tasks: {e}"),
                "internal.error",
                None,
                Some(&instance_id),
            );
        }
    };

    // Apply cursor: keep rows with created_at > cursor.
    let filtered: Vec<_> = match cursor_ms {
        Some(ms) => rows
            .into_iter()
            .filter(|r| r.created_at.timestamp_millis() > ms)
            .collect(),
        None => rows,
    };

    let page: Vec<_> = filtered.into_iter().take(limit as usize).collect();
    let next_cursor = if page.len() as u64 == limit {
        page.last()
            .map(|r| encode_cursor(r.created_at.timestamp_millis()))
    } else {
        None
    };

    let tasks: Vec<_> = page.iter().map(task_row_to_a2a).collect();
    let mut body = json!({ "tasks": tasks });
    if let Some(c) = next_cursor {
        body["next_cursor"] = serde_json::Value::String(c);
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(body.to_string()))
        .unwrap()
        .into_response()
}

fn encode_cursor(ms: i64) -> String {
    URL_SAFE_NO_PAD.encode(ms.to_string().as_bytes())
}

fn decode_cursor(s: &str) -> Option<i64> {
    let bytes = URL_SAFE_NO_PAD.decode(s.as_bytes()).ok()?;
    let s = std::str::from_utf8(&bytes).ok()?;
    s.parse::<i64>().ok()
}
