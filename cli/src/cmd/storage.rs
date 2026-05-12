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
    let v2 = super::with_query("/api/v2/admin/storage/global", &q);
    let v1 = super::with_query("/api/v1/storage/global", &q);
    render_listing_dual(c, &v2, &v1, as_json).await
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
    let v2 = super::with_query(&format!("/api/v2/admin/storage/inbox/{}", agent), &q);
    let v1 = super::with_query(&format!("/api/v1/storage/inbox/{}", agent), &q);
    render_listing_dual(c, &v2, &v1, as_json).await
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
    let v2 = super::with_query(&format!("/api/v2/admin/storage/outbox/{}", task), &q);
    let v1 = super::with_query(&format!("/api/v1/storage/outbox/{}", task), &q);
    render_listing_dual(c, &v2, &v1, as_json).await
}

async fn render_listing_dual(c: &HttpClient, v2: &str, v1: &str, as_json: bool) -> Result<()> {
    let (v, _via_v1) = c.try_v2_then_v1(v2, v1, "GET", None).await?;
    render_listing_value(&v, as_json)
}

fn render_listing_value(v: &Value, as_json: bool) -> Result<()> {
    super::emit(v, as_json, || {
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
///
/// `push_bytes` is mutating, so v2-first/v1-fallback is implemented inline
/// by retrying on `NotFound` rather than going through `try_v2_then_v1`
/// (which expects JSON bodies).
pub async fn global_push(
    c: &HttpClient,
    remote_path: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    let v2 = format!(
        "/api/v2/admin/storage/global?path={}",
        super::urlencode(remote_path)
    );
    let v1 = format!(
        "/api/v1/storage/global?path={}",
        super::urlencode(remote_path)
    );
    push_inner_with_fallback(c, &v2, &v1, local, as_json).await
}

/// `storage inbox push <agent> --path <remote> --file <local>`.
pub async fn inbox_push(
    c: &HttpClient,
    agent: &str,
    remote_path: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    let v2 = format!(
        "/api/v2/admin/storage/inbox/{}?path={}",
        agent,
        super::urlencode(remote_path)
    );
    let v1 = format!(
        "/api/v1/storage/inbox/{}?path={}",
        agent,
        super::urlencode(remote_path)
    );
    push_inner_with_fallback(c, &v2, &v1, local, as_json).await
}

async fn push_inner_with_fallback(
    c: &HttpClient,
    v2_url: &str,
    v1_url: &str,
    local: &std::path::Path,
    as_json: bool,
) -> Result<()> {
    let bytes =
        std::fs::read(local).map_err(|e| anyhow::anyhow!("reading {}: {}", local.display(), e))?;
    let total = bytes.len();
    if total > 10 * 1024 * 1024 && !as_json {
        eprintln!(
            "uploading {} bytes ({:.1} MiB)…",
            total,
            total as f64 / (1024.0 * 1024.0)
        );
    }
    // Try v2 first; on 404 fall back to v1 with a Sunset warning.
    let v: Value = match c.post_bytes::<Value>(v2_url, bytes.clone()).await {
        Ok(v) => v,
        Err(crate::client::http::ClientError::NotFound(_)) => {
            eprintln!(
                "warning: v2 admin path `{}` returned 404; falling back to v1 `{}`. \
                 v1 admin paths are scheduled for removal.",
                v2_url, v1_url
            );
            c.post_bytes(v1_url, bytes).await?
        }
        Err(e) => return Err(e.into()),
    };
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("path", jstr(&v, "path", "-").to_string()),
            ("bytes_written", crate::output::jnum(&v, "bytes_written")),
            ("uploaded_from", local.display().to_string()),
        ];
        crate::output::kv::render(&pairs)
    })
}

// `push_inner` (single-URL) removed during the v2 migration. The new
// `push_inner_with_fallback` handles both v2 and v1 paths.
