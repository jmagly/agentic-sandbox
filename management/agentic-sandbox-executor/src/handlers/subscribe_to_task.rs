//! A2A `tasks/subscribe` handler (#210).
//!
//! `GET /agents/{instance_id}/v1/tasks/{tid}:subscribe`
//!
//! Server-Sent Events stream. First event is `event: task` carrying the
//! current Task JSON. Implementation note: this is a **polling stub** —
//! we poll [`TaskStore`] every second and emit a `task` event whenever
//! the row changes (state transitions). On terminal state we send one
//! final event and close. Real event-source streaming wires in #213.
//!
//! [`TaskStore`]: crate::store::task_store::TaskStore

use std::convert::Infallible;
use std::time::Duration;

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;

use super::task_row_to_a2a;

/// Axum handler for `GET /agents/{instance_id}/v1/tasks/{tid}:subscribe`.
pub async fn handler(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    // Verify the task exists before opening the stream so we can return 404.
    let initial = match state.store.get_task(&tid) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "https://agentic-sandbox.aiwg.io/errors/task-not-found",
                "Task not found",
                format!("Task '{}' not found", tid),
                "task.not_found",
                None,
                Some(&instance_id),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                format!("Failed to read task: {e}"),
                "internal.error",
                None,
                Some(&instance_id),
            );
        }
    };

    let store = state.store.clone();
    let tid_owned = tid.clone();

    let stream = sse_stream(store, tid_owned, initial);

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn sse_stream(
    store: std::sync::Arc<crate::store::task_store::TaskStore>,
    tid: String,
    initial: crate::store::task_store::TaskRow,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream! {
        let task_json = task_row_to_a2a(&initial);
        let event = Event::default().event("task").data(task_json.to_string());
        yield Ok(event);

        if initial.state.is_terminal() {
            return;
        }

        let mut last_state = initial.state;
        let mut last_updated = initial.updated_at;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match store.get_task(&tid) {
                Ok(Some(row)) => {
                    if row.state != last_state || row.updated_at != last_updated {
                        let task_json = task_row_to_a2a(&row);
                        let event = Event::default().event("task").data(task_json.to_string());
                        yield Ok(event);
                        last_state = row.state;
                        last_updated = row.updated_at;
                        if row.state.is_terminal() {
                            return;
                        }
                    }
                }
                Ok(None) => return,
                Err(_) => return,
            }
        }
    }
}
