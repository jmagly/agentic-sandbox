//! `storage` — agentshare REST surface.
//!
//! Backing routes (admin-gated server-side):
//! - `GET    /api/v1/storage/global?path=<p>`              ← `storage global ls`
//! - `POST   /api/v1/storage/global?path=<p>`              ← `storage global push`
//! - `GET    /api/v1/storage/inbox/{agent}?path=<p>`       ← `storage inbox ls`
//! - `POST   /api/v1/storage/inbox/{agent}?path=<p>`       ← `storage inbox push`
//! - `GET    /api/v1/storage/outbox/{task}?path=<p>`       ← `storage outbox ls`

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, table};

pub async fn global_ls(c: &HttpClient, path: Option<&str>, as_json: bool) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(p) = path {
        q.push(("path".into(), p.into()));
    }
    let url = super::with_query("/api/v1/storage/global", &q);
    render_listing(c, &url, as_json).await
}

pub async fn inbox_ls(
    c: &HttpClient,
    agent: &str,
    path: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(p) = path {
        q.push(("path".into(), p.into()));
    }
    let url = super::with_query(&format!("/api/v1/storage/inbox/{}", agent), &q);
    render_listing(c, &url, as_json).await
}

pub async fn outbox_ls(
    c: &HttpClient,
    task: &str,
    path: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(p) = path {
        q.push(("path".into(), p.into()));
    }
    let url = super::with_query(&format!("/api/v1/storage/outbox/{}", task), &q);
    render_listing(c, &url, as_json).await
}

async fn render_listing(c: &HttpClient, url: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(url).await?;
    super::emit(&v, as_json, || {
        let entries = v
            .get("entries")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                vec![
                    jstr(e, "kind", "-").to_string(),
                    jstr(e, "name", "-").to_string(),
                    crate::output::jnum(e, "size_bytes"),
                    jstr(e, "mode", "-").to_string(),
                    jstr(e, "modified", "-").to_string(),
                ]
            })
            .collect();
        let mut out = format!("root: {}\n", jstr(&v, "root", "-"));
        out.push_str(&format!("path: {}\n\n", jstr(&v, "path", "")));
        out.push_str(&table::render(
            &["KIND", "NAME", "SIZE", "MODE", "MODIFIED"],
            &rows,
        ));
        out
    })
}

/// `storage global push --path <remote> --file <local>` — POST raw body.
pub async fn global_push(
    c: &HttpClient,
    remote_path: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    push_inner(
        c,
        &format!(
            "/api/v1/storage/global?path={}",
            super::urlencode(remote_path)
        ),
        local,
        as_json,
    )
    .await
}

/// `storage inbox push <agent> --path <remote> --file <local>`.
pub async fn inbox_push(
    c: &HttpClient,
    agent: &str,
    remote_path: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    push_inner(
        c,
        &format!(
            "/api/v1/storage/inbox/{}?path={}",
            agent,
            super::urlencode(remote_path)
        ),
        local,
        as_json,
    )
    .await
}

async fn push_inner(
    c: &HttpClient,
    url_path: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    let bytes =
        std::fs::read(local).map_err(|e| anyhow::anyhow!("reading {}: {}", local.display(), e))?;
    let total = bytes.len();
    if total > 10 * 1024 * 1024 && !as_json {
        // Issue scope: "storage push shows progress for files > 10 MiB".
        // We don't have a streaming upload yet; surface the size up-front
        // so the operator knows the wait is intentional.
        eprintln!(
            "uploading {} bytes ({:.1} MiB)…",
            total,
            total as f64 / (1024.0 * 1024.0)
        );
    }
    let v: Value = c.post_bytes(url_path, bytes).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("path", jstr(&v, "path", "-").to_string()),
            ("bytes_written", crate::output::jnum(&v, "bytes_written")),
            ("uploaded_from", local.display().to_string()),
        ];
        crate::output::kv::render(&pairs)
    })
}
