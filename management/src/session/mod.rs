//! Formal PTY session multiplexing layer.
//!
//! A [`Session`] is the durable unit of PTY multiplexing.  It outlives
//! individual command invocations and client connections.  Multiple clients
//! attach as [`Role::Observer`]s or request the [`Role::Controller`] slot,
//! which grants exclusive stdin/resize access.
//!
//! **The server owns all state.**  Clients are dumb connectors — they join,
//! receive a stream of sequenced [`SessionFrame`]s, optionally take control,
//! and detach without killing the session.  This is the tmux/screen model.

pub mod registry;
pub mod replay;

pub use registry::{Session, SessionAttachment, SessionRegistry};
pub use replay::ReplayBuffer;

use serde::{Deserialize, Serialize};

/// Stable session identifier (UUIDv7).  Survives command_id changes on
/// agent reconnect.
pub type SessionId = String;

/// Per-connection client identifier (UUIDv4, assigned on WS attach).
pub type ClientId = String;

// ── Role ─────────────────────────────────────────────────────────────────────

/// Role of a client within a session.
///
/// At most one `Controller` exists at a time.  All others are `Observer`s.
/// Observers may request promotion; the current controller may voluntarily
/// yield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Exclusive stdin/resize access.
    Controller,
    /// Read-only; receives all output frames.
    Observer,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Controller => write!(f, "controller"),
            Role::Observer => write!(f, "observer"),
        }
    }
}

impl Role {
    pub fn from_str(s: &str) -> Self {
        match s {
            "controller" => Role::Controller,
            _ => Role::Observer,
        }
    }
}

// ── Wire frame ────────────────────────────────────────────────────────────────

/// The fundamental unit emitted to every client attached to a session.
///
/// `seq` is monotonically increasing per session.  Clients use it to detect
/// gaps and to request replay from a specific point after reconnect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFrame {
    pub session_id: SessionId,
    /// Monotonically increasing per-session sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub ts: i64,
    #[serde(flatten)]
    pub payload: SessionPayload,
}

/// Content of a [`SessionFrame`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionPayload {
    /// Raw PTY bytes from the running process.
    Output {
        stream: StreamKind,
        /// Base64-encoded binary PTY data (binary-safe).
        data: String,
    },
    /// Terminal dimensions changed; broadcast to all clients.
    Resize { cols: u16, rows: u16 },
    /// The caller's role in this session was (re)assigned.
    RoleAssigned { role: Role },
    /// The controller slot changed.  `None` means no controller.
    ControllerChanged { controller: Option<ClientId> },
    /// Session ended (process exited or was killed).
    Closed { exit_code: Option<i32> },
    /// Session-level error.
    Error { message: String },
}

/// PTY output stream discriminant (stdout / stderr / log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Stdout,
    Stderr,
    Log,
}

impl std::fmt::Display for StreamKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamKind::Stdout => write!(f, "stdout"),
            StreamKind::Stderr => write!(f, "stderr"),
            StreamKind::Log => write!(f, "log"),
        }
    }
}
