//! Agentshare REST surface.
//!
//! Provides a narrow, admin-only HTTP API over the on-disk agentshare
//! layout so operators (and `sandboxctl`) can list, push, and pull files
//! without SSH-ing into the host.
//!
//! Layout (from `docs/agentshare.md`):
//! - `<agentshare_root>/global-ro/`               — global RO share
//! - `<agentshare_root>/<agent>-inbox/`           — per-agent RW share
//! - `<tasks_root>/<task-id>/outbox/`             — per-task output
//!
//! All path inputs are sanitized; `..`, absolute paths, and symlinks
//! that escape their root are rejected.

use axum::{
    body::Bytes,
    extract::{Path as AxPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use super::server::AppState;

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    /// Relative path under the resolved root. Empty / missing = root itself.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EntryInfo {
    pub name: String,
    pub kind: &'static str, // "file" | "dir" | "other"
    pub size_bytes: Option<u64>,
    /// Mode in octal (e.g. "0644"). Unix-only.
    pub mode: Option<String>,
    /// RFC3339 modified time.
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub root: String,
    pub path: String,
    pub entries: Vec<EntryInfo>,
}

// ── Path safety ───────────────────────────────────────────────────────────

/// Resolve a user-provided relative path under `root`, refusing any input
/// that contains `..`, root-anchored components, or symlinks that point
/// outside the canonicalized root.
fn safe_resolve(root: &Path, rel: &str) -> Result<PathBuf, StorageHttpError> {
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    // Reject explicit absolute paths up-front. Callers must pass paths
    // relative to the share root; allowing leading `/` would be a footgun.
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(StorageHttpError::InvalidPath);
    }
    let candidate = Path::new(rel);

    // Reject before canonicalize: cheap defense-in-depth.
    for c in candidate.components() {
        match c {
            Component::ParentDir => return Err(StorageHttpError::InvalidPath),
            Component::RootDir | Component::Prefix(_) => return Err(StorageHttpError::InvalidPath),
            _ => {}
        }
    }

    let joined = root.join(candidate);

    // Canonicalize what exists. If joined doesn't yet exist (e.g. upload
    // target), canonicalize the deepest existing parent and then append the
    // remaining tail; verify the whole thing stays under root.
    let resolved = match joined.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            let mut anchor = joined.as_path();
            let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
            while !anchor.exists() {
                if let (Some(parent), Some(name)) = (anchor.parent(), anchor.file_name()) {
                    tail.push(name);
                    anchor = parent;
                } else {
                    return Err(StorageHttpError::InvalidPath);
                }
            }
            let mut canon = anchor.canonicalize().map_err(StorageHttpError::Io)?;
            for seg in tail.iter().rev() {
                canon.push(seg);
            }
            canon
        }
    };

    let canon_root = root.canonicalize().map_err(StorageHttpError::Io)?;
    if !resolved.starts_with(&canon_root) {
        return Err(StorageHttpError::InvalidPath);
    }
    Ok(resolved)
}

// ── Roots from AppState ───────────────────────────────────────────────────

fn global_root(state: &AppState) -> Option<PathBuf> {
    state
        .agentshare_root
        .as_ref()
        .map(|r| PathBuf::from(r).join("global-ro"))
}

fn inbox_root(state: &AppState, agent_id: &str) -> Option<PathBuf> {
    // Agent IDs come from the URL — refuse anything that could escape
    // the agentshare root via the inbox name itself.
    if agent_id.is_empty()
        || agent_id == "."
        || agent_id == ".."
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return None;
    }
    state
        .agentshare_root
        .as_ref()
        .map(|r| PathBuf::from(r).join(format!("{}-inbox", agent_id)))
}

fn outbox_root(state: &AppState, task_id: &str) -> Option<PathBuf> {
    if task_id.is_empty()
        || task_id == "."
        || task_id == ".."
        || task_id.contains('/')
        || task_id.contains('\\')
    {
        return None;
    }
    state
        .tasks_root
        .as_ref()
        .map(|r| PathBuf::from(r).join(task_id).join("outbox"))
}

// ── Handlers: list ────────────────────────────────────────────────────────

pub async fn list_global(Query(q): Query<PathQuery>, State(state): State<AppState>) -> Response {
    let root = match global_root(&state) {
        Some(r) => r,
        None => return service_unavailable(),
    };
    list_at(&root, q.path.as_deref().unwrap_or("")).await
}

pub async fn list_inbox(
    AxPath(agent_id): AxPath<String>,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
) -> Response {
    let root = match inbox_root(&state, &agent_id) {
        Some(r) => r,
        None => return invalid_path(),
    };
    list_at(&root, q.path.as_deref().unwrap_or("")).await
}

pub async fn list_outbox(
    AxPath(task_id): AxPath<String>,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
) -> Response {
    let root = match outbox_root(&state, &task_id) {
        Some(r) => r,
        None => return invalid_path(),
    };
    list_at(&root, q.path.as_deref().unwrap_or("")).await
}

async fn list_at(root: &Path, rel: &str) -> Response {
    let target = match safe_resolve(root, rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !target.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "path not found"})),
        )
            .into_response();
    }

    let mut entries = Vec::new();
    if target.is_dir() {
        let mut rd = match fs::read_dir(&target).await {
            Ok(r) => r,
            Err(e) => return StorageHttpError::Io(e).into_response(),
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            entries.push(entry_info(&entry).await);
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
    } else {
        // Listing a single file: return one entry for it.
        if let Some(name) = target.file_name().and_then(|n| n.to_str()) {
            let meta = fs::metadata(&target).await.ok();
            entries.push(EntryInfo {
                name: name.to_string(),
                kind: "file",
                size_bytes: meta.as_ref().map(|m| m.len()),
                mode: meta.as_ref().map(format_mode),
                modified: meta.as_ref().and_then(format_mtime),
            });
        }
    }

    Json(ListResponse {
        root: root.display().to_string(),
        path: rel.to_string(),
        entries,
    })
    .into_response()
}

async fn entry_info(entry: &tokio::fs::DirEntry) -> EntryInfo {
    let name = entry.file_name().to_string_lossy().into_owned();
    let meta = entry.metadata().await.ok();
    let kind = match meta.as_ref().map(|m| m.file_type()) {
        Some(ft) if ft.is_dir() => "dir",
        Some(ft) if ft.is_file() => "file",
        _ => "other",
    };
    EntryInfo {
        name,
        kind,
        size_bytes: meta.as_ref().map(|m| m.len()),
        mode: meta.as_ref().map(format_mode),
        modified: meta.as_ref().and_then(format_mtime),
    }
}

#[cfg(unix)]
fn format_mode(m: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    format!("{:04o}", m.permissions().mode() & 0o7777)
}
#[cfg(not(unix))]
fn format_mode(_: &std::fs::Metadata) -> String {
    String::from("0000")
}

fn format_mtime(m: &std::fs::Metadata) -> Option<String> {
    let modified = m.modified().ok()?;
    let datetime: chrono::DateTime<chrono::Utc> = modified.into();
    Some(datetime.to_rfc3339())
}

// ── Handlers: download ────────────────────────────────────────────────────

pub async fn download_global(
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
) -> Response {
    let root = match global_root(&state) {
        Some(r) => r,
        None => return service_unavailable(),
    };
    download_at(&root, q.path.as_deref().unwrap_or("")).await
}

pub async fn download_inbox(
    AxPath(agent_id): AxPath<String>,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
) -> Response {
    let root = match inbox_root(&state, &agent_id) {
        Some(r) => r,
        None => return invalid_path(),
    };
    download_at(&root, q.path.as_deref().unwrap_or("")).await
}

pub async fn download_outbox(
    AxPath(task_id): AxPath<String>,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
) -> Response {
    let root = match outbox_root(&state, &task_id) {
        Some(r) => r,
        None => return invalid_path(),
    };
    download_at(&root, q.path.as_deref().unwrap_or("")).await
}

async fn download_at(root: &Path, rel: &str) -> Response {
    if rel.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path required for download"})),
        )
            .into_response();
    }
    let target = match safe_resolve(root, rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !target.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "file not found"})),
        )
            .into_response();
    }
    if !target.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path is not a regular file"})),
        )
            .into_response();
    }
    match fs::read(&target).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&target)
                .first_or_octet_stream()
                .to_string();
            let filename = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download");
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{}\"", filename),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => StorageHttpError::Io(e).into_response(),
    }
}

// ── Handlers: upload ──────────────────────────────────────────────────────

pub async fn upload_global(
    _: super::operator_auth::RequireAdmin,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    let root = match global_root(&state) {
        Some(r) => r,
        None => return service_unavailable(),
    };
    upload_at(&root, q.path.as_deref().unwrap_or(""), body).await
}

pub async fn upload_inbox(
    _: super::operator_auth::RequireAdmin,
    AxPath(agent_id): AxPath<String>,
    Query(q): Query<PathQuery>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    let root = match inbox_root(&state, &agent_id) {
        Some(r) => r,
        None => return invalid_path(),
    };
    upload_at(&root, q.path.as_deref().unwrap_or(""), body).await
}

async fn upload_at(root: &Path, rel: &str, body: Bytes) -> Response {
    if rel.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path required for upload"})),
        )
            .into_response();
    }
    let target = match safe_resolve(root, rel) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    // Ensure parent dir exists.
    if let Some(parent) = target.parent() {
        if let Err(e) = fs::create_dir_all(parent).await {
            return StorageHttpError::Io(e).into_response();
        }
    }

    // Atomic write: temp file in the same dir, then rename.
    let parent = target.parent().unwrap_or(Path::new("."));
    let tmp_name = format!(".sandboxctl-upload-{}.tmp", uuid::Uuid::new_v4().simple());
    let tmp = parent.join(tmp_name);

    let write_res: Result<(), std::io::Error> = async {
        let mut f = fs::File::create(&tmp).await?;
        f.write_all(&body).await?;
        f.flush().await?;
        f.sync_all().await?;
        Ok(())
    }
    .await;
    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp).await;
        return StorageHttpError::Io(e).into_response();
    }
    if let Err(e) = fs::rename(&tmp, &target).await {
        let _ = fs::remove_file(&tmp).await;
        return StorageHttpError::Io(e).into_response();
    }
    // Best-effort: 0644 default. Caller-specified modes intentionally not
    // supported in v1.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).await;
    }

    Json(serde_json::json!({
        "path": rel,
        "bytes_written": body.len(),
    }))
    .into_response()
}

// ── Errors / shared helpers ───────────────────────────────────────────────

#[derive(Debug)]
enum StorageHttpError {
    InvalidPath,
    Io(std::io::Error),
}

impl IntoResponse for StorageHttpError {
    fn into_response(self) -> Response {
        match self {
            StorageHttpError::InvalidPath => invalid_path(),
            StorageHttpError::Io(e) => {
                warn!(error = %e, "storage IO error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}

fn invalid_path() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "invalid path: absolute paths, '..', and unsafe components are rejected"
        })),
    )
        .into_response()
}

fn service_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "agentshare not configured on this server"
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn safe_resolve_rejects_parent_dir() {
        let root = tempdir().unwrap();
        assert!(safe_resolve(root.path(), "../etc/passwd").is_err());
        assert!(safe_resolve(root.path(), "a/../../b").is_err());
    }

    #[test]
    fn safe_resolve_rejects_absolute() {
        let root = tempdir().unwrap();
        assert!(safe_resolve(root.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn safe_resolve_accepts_nested_relative() {
        let root = tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let resolved = safe_resolve(root.path(), "a/b").unwrap();
        assert_eq!(resolved, nested.canonicalize().unwrap());
    }

    #[test]
    fn safe_resolve_allows_nonexistent_under_root() {
        let root = tempdir().unwrap();
        // Used by upload_at — file doesn't exist yet, but path is safe.
        let resolved = safe_resolve(root.path(), "new/file.txt").unwrap();
        assert!(resolved.starts_with(root.path().canonicalize().unwrap()));
    }
}
