//! `container` verbs (#173 Section B / D).
//!
//! Backing routes:
//! - `GET    /api/v1/containers`             ← `container list` (`--state`)
//! - `GET    /api/v1/containers/{name}`      ← `container get <name>`
//! - `POST   /api/v1/containers`             ← `container create <name> --image ...`
//! - `POST   /api/v1/containers/{name}/start` ← `container start <name>`
//! - `POST   /api/v1/containers/{name}/stop`  ← `container stop <name> [--timeout]`
//! - `DELETE /api/v1/containers/{name}`      ← `container delete <name>` (admin)
//!
//! PTY attach inside a container goes through the formal session
//! protocol once #174 (in-container agent + image build pipeline)
//! lands. For now this group is for create/inspect/lifecycle only.

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, status: Option<&str>, as_json: bool) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = status {
        q.push(("status".into(), s.into()));
    }
    let path = super::with_query("/api/v1/containers", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("containers")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|cn| {
                vec![
                    jstr(cn, "name", "").to_string(),
                    short_id(jstr(cn, "id", "")),
                    jstr(cn, "status", "-").to_string(),
                    jstr(cn, "finished_at", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "ID", "STATUS", "FINISHED"], &rows)
    })
}

pub async fn get(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/containers/{}", name)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", "-").to_string()),
            ("id", jstr(&v, "id", "-").to_string()),
            ("status", jstr(&v, "status", "-").to_string()),
            ("finished_at", jstr(&v, "finished_at", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    c: &HttpClient,
    name: &str,
    image: &str,
    env: &[String],
    mounts: &[String],
    network: Option<&str>,
    cmd: &[String],
    as_json: bool,
) -> Result<()> {
    let body = json!({
        "name": name,
        "image": image,
        "env": env,
        "mounts": mounts,
        "network": network,
        "cmd": cmd,
    });
    let v: Value = c.post_json("/api/v1/containers", Some(&body)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", name).to_string()),
            ("id", jstr(&v, "id", "-").to_string()),
            ("image", jstr(&v, "image", image).to_string()),
            ("status", jstr(&v, "status", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

pub async fn start(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let v: Value = c
        .post_json::<Value, ()>(&format!("/api/v1/containers/{}/start", name), None)
        .await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", name).to_string()),
            ("action", jstr(&v, "action", "start").to_string()),
            ("status", jstr(&v, "status", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

pub async fn stop(c: &HttpClient, name: &str, timeout: u64, as_json: bool) -> Result<()> {
    let path = format!("/api/v1/containers/{}/stop?timeout={}", name, timeout);
    let v: Value = c.post_json::<Value, ()>(&path, None).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", name).to_string()),
            ("action", jstr(&v, "action", "stop").to_string()),
            ("status", jstr(&v, "status", "-").to_string()),
            ("timeout", crate::output::jnum(&v, "timeout")),
        ];
        kv::render(&pairs)
    })
}

pub async fn delete(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let v: Value = c.delete_json(&format!("/api/v1/containers/{}", name)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", name).to_string()),
            ("deleted", crate::output::jnum(&v, "deleted")),
        ];
        kv::render(&pairs)
    })
}

/// Truncate a Docker ID to its 12-char short form for table display.
fn short_id(full: &str) -> String {
    if full.len() > 12 {
        full[..12].to_string()
    } else {
        full.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_truncates_long_ids() {
        let id = "abcdef0123456789feedcafe";
        assert_eq!(short_id(id), "abcdef012345");
    }

    #[test]
    fn short_id_passes_through_short() {
        assert_eq!(short_id("short"), "short");
    }
}
