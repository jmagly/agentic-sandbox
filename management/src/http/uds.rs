//! Unix-domain-socket listener for the operator HTTP API.
//!
//! Binds `<path>` (default `/run/agentic-mgmt.sock`) and serves the same
//! `axum::Router` as the TCP listener. Connections are authenticated via
//! `SO_PEERCRED`: any process that can connect to the socket (gated by
//! filesystem permissions on the socket path itself) is implicitly
//! `OperatorRole::Admin`. The bearer-token middleware no-ops when the
//! role is already in request extensions, so UDS bypasses tokens cleanly.
//!
//! Filesystem ACL: socket is created mode 0660 and `chgrp`'d to the
//! configured group (default `agentic-admin`); members of that group
//! can connect, others cannot. If the group lookup fails the socket
//! still binds with mode 0660 owned by the management server's user/
//! group — the operator must `chgrp` it manually.

use axum::Router;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::ffi::CString;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::UnixListener;
use tower::Service;
use tracing::{debug, error, info, warn};

use super::operator_auth::OperatorRole;

/// Peer credentials captured at accept time. Stashed in request
/// extensions so handlers / future audit logging can identify the
/// caller process by uid/pid without re-querying the socket.
#[derive(Debug, Clone, Copy)]
pub struct UdsPeer {
    pub uid: u32,
    pub gid: u32,
    pub pid: Option<i32>,
}

/// Configuration for the UDS listener.
pub struct UdsConfig {
    /// Filesystem path for the socket.
    pub path: PathBuf,
    /// Group whose members may connect. Lookup failure logs a warning;
    /// the socket still binds with the running user's primary group.
    pub group: String,
}

impl Default for UdsConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/run/agentic-mgmt.sock"),
            group: "agentic-admin".to_string(),
        }
    }
}

/// Bind, configure ACLs, and serve `app` over the UDS until the task
/// is cancelled or the listener errors fatally.
pub async fn serve(cfg: UdsConfig, app: Router) -> io::Result<()> {
    // Remove any stale socket file from a previous run. We deliberately
    // do this BEFORE bind so a SIGKILL'd predecessor can't keep a hot
    // socket name reserved.
    if cfg.path.exists() {
        if let Err(e) = std::fs::remove_file(&cfg.path) {
            warn!(error = %e, path = ?cfg.path, "could not remove stale UDS file; proceeding");
        }
    }
    if let Some(parent) = cfg.path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let listener = UnixListener::bind(&cfg.path)?;

    // Permissions: 0660 + chgrp <group>. If the group doesn't exist the
    // socket is owner-only effectively (group will be the user's primary
    // group); operator can chgrp manually.
    let perms = std::fs::Permissions::from_mode(0o660);
    if let Err(e) = std::fs::set_permissions(&cfg.path, perms) {
        warn!(error = %e, path = ?cfg.path, "failed to chmod 0660 on UDS");
    }
    match lookup_gid(&cfg.group) {
        Some(gid) => {
            if let Err(e) = chown_path(&cfg.path, gid) {
                warn!(
                    error = %e,
                    group = %cfg.group,
                    gid,
                    "failed to chgrp UDS; non-root users in `{}` group cannot connect",
                    cfg.group
                );
            } else {
                info!(path = ?cfg.path, group = %cfg.group, gid, "UDS group ACL applied");
            }
        }
        None => {
            warn!(
                group = %cfg.group,
                path = ?cfg.path,
                "group not found via getgrnam; UDS uses the management user's primary group. \
                 Operator must `groupadd {}` and `chgrp {} {:?}` to share access.",
                cfg.group, cfg.group, cfg.path
            );
        }
    }

    info!(path = ?cfg.path, "UDS listener accepting connections");
    accept_loop(listener, app).await
}

async fn accept_loop(listener: UnixListener, app: Router) -> io::Result<()> {
    let app = Arc::new(app);
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "UDS accept failed");
                // Briefly back off on accept errors. EMFILE etc. would
                // otherwise spin. 10ms is enough to let the OS catch up
                // without adding noticeable latency to legitimate clients.
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
        };
        let peer = match stream.peer_cred() {
            Ok(c) => UdsPeer {
                uid: c.uid(),
                gid: c.gid(),
                pid: c.pid(),
            },
            Err(e) => {
                warn!(error = %e, "could not read UDS peer creds; closing");
                continue;
            }
        };
        debug!(uid = peer.uid, gid = peer.gid, pid = ?peer.pid, "UDS connection accepted");

        let app = app.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req: hyper::Request<Incoming>| {
                // Wrap hyper's Incoming body in axum's Body so we can call
                // the Router service. axum's Body::new accepts any
                // http_body_util-compatible body.
                let (parts, body) = req.into_parts();
                let mut req = hyper::Request::from_parts(parts, axum::body::Body::new(body));
                // Pre-populate request extensions: peer-cred ⇒ admin role.
                // auth_middleware sees the existing role and skips token lookup.
                req.extensions_mut().insert(OperatorRole::Admin);
                req.extensions_mut().insert(peer);
                let mut app = (*app).clone();
                async move { app.call(req).await }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                debug!(error = %e, "UDS connection ended with error");
            }
        });
    }
}

// ── chown / group lookup helpers ──────────────────────────────────────────

fn lookup_gid(group: &str) -> Option<libc::gid_t> {
    let c = CString::new(group).ok()?;
    // SAFETY: getgrnam returns a pointer into thread-local storage that
    // remains valid until the next getgrnam call on this thread. We read
    // gr_gid immediately and don't retain the pointer.
    unsafe {
        let g = libc::getgrnam(c.as_ptr());
        if g.is_null() {
            None
        } else {
            Some((*g).gr_gid)
        }
    }
}

fn chown_path(path: &Path, gid: libc::gid_t) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // uid = -1 (u32::MAX) leaves the user unchanged; only gid is set.
    // SAFETY: c is a valid C string; chown is safe with any pointer to
    // a NUL-terminated path.
    let rc = unsafe { libc::chown(c.as_ptr(), u32::MAX, gid) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
