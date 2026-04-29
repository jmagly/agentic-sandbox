//! `task` read-only verbs.
//!
//! Backing routes:
//! - `GET /api/v1/tasks`                                  ← `task list` (`--state`, `--limit`, `--offset`)
//! - `GET /api/v1/tasks/{id}`                             ← `task get <id>`
//! - `GET /api/v1/tasks/{id}/artifacts`                   ← `task artifacts list <id>`

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(
    c: &HttpClient,
    state: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    if let Some(l) = limit {
        q.push(("limit".into(), l.to_string()));
    }
    if let Some(o) = offset {
        q.push(("offset".into(), o.to_string()));
    }
    let path = super::with_query("/api/v1/tasks", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("tasks")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|t| {
                vec![
                    jstr(t, "id", "").to_string(),
                    jstr(t, "name", "-").to_string(),
                    jstr(t, "state", "-").to_string(),
                    jstr(t, "created_at", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["ID", "NAME", "STATE", "CREATED"], &rows)
    })
}

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/tasks/{}", id)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("id", jstr(&v, "id", "").to_string()),
            ("name", jstr(&v, "name", "-").to_string()),
            ("state", jstr(&v, "state", "-").to_string()),
            ("created_at", jstr(&v, "created_at", "-").to_string()),
            ("started_at", jstr(&v, "started_at", "-").to_string()),
            ("vm_name", jstr(&v, "vm_name", "-").to_string()),
            ("exit_code", crate::output::jnum(&v, "exit_code")),
        ];
        kv::render(&pairs)
    })
}

pub async fn artifacts_list(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/tasks/{}/artifacts", id)).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("artifacts")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|a| {
                vec![
                    jstr(a, "name", "").to_string(),
                    crate::output::jnum(a, "size_bytes"),
                    jstr(a, "content_type", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "SIZE", "CONTENT_TYPE"], &rows)
    })
}
