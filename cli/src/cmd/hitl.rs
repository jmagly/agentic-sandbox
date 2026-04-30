//! `hitl` — human-in-the-loop responses.
//!
//! Backing route: `POST /api/v1/hitl/{id}/respond` with `{ text }` body.
//! Returns 204 No Content on success.

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;

pub async fn respond(c: &HttpClient, id: &str, text: &str, as_json: bool) -> Result<()> {
    let body = serde_json::json!({ "text": text });
    // Server returns 204; HttpClient's empty-body branch yields a
    // null `Value` we don't render (just confirm).
    let _v: Value = c
        .post_json(&format!("/api/v1/hitl/{}/respond", id), Some(&body))
        .await?;
    if as_json {
        let payload = serde_json::json!({
            "hitl_id": id,
            "delivered": true,
            "bytes": text.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("hitl/{}: response delivered ({} bytes)", id, text.len());
    }
    Ok(())
}
