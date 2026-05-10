//! `agentic-sandbox/pty` extension. Filled in by #213 + W4.1.
//!
//! Advertises the PTY-over-WebSocket binding and surfaces TTY metadata
//! (rows, cols, encoding) on the AgentCard.
//!
//! Pre-#213-completion stub: declares the URI and a no-op handler so
//! `build_default_registry` compiles.

use super::ExtensionHandler;

/// Extension URI per spec.
pub const URI: &str = "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1";

/// PTY-extension marker.
pub struct PtyExtension;

impl PtyExtension {
    /// No-op constructor for registry wiring.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PtyExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHandler for PtyExtension {
    fn uri(&self) -> &'static str {
        URI
    }
    fn required(&self) -> bool {
        false
    }
}
