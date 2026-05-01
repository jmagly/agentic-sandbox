//! Client-side configuration for `sandboxctl`.
//!
//! kubeconfig-style contexts file at `$XDG_CONFIG_HOME/agentic-sandbox/contexts.toml`
//! (defaults to `~/.config/agentic-sandbox/contexts.toml`):
//!
//! ```toml
//! current_context = "lab"
//!
//! [contexts.lab]
//! server = "http://localhost:8122"
//! token  = "..."          # optional; required when remote auth is enabled
//! role   = "admin"        # informational; server enforces actual role
//! ```
//!
//! File mode: 0600 (tokens live here; keep readable only by the owner).

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextsFile {
    /// Name of the active context. None ⇒ no context selected.
    pub current_context: Option<String>,
    /// Named contexts (server + token + role).
    #[serde(default)]
    pub contexts: BTreeMap<String, ContextEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Management-server base URL (no trailing slash).
    pub server: String,
    /// Bearer token for remote auth. Empty ⇒ rely on Unix socket / no auth.
    #[serde(default)]
    pub token: String,
    /// Operator-declared role. Server enforces the real role; this is just
    /// for `sandboxctl config whoami` and operator awareness.
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "operator".to_string()
}

impl ContextsFile {
    /// Default path: `$XDG_CONFIG_HOME/agentic-sandbox/contexts.toml`,
    /// falling back to `~/.config/agentic-sandbox/contexts.toml`.
    pub fn default_path() -> Result<PathBuf> {
        let base =
            dirs::config_dir().ok_or_else(|| anyhow!("could not resolve config directory"))?;
        Ok(base.join("agentic-sandbox").join("contexts.toml"))
    }

    /// Load from the default path. Missing file ⇒ empty `ContextsFile`.
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let parsed: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(parsed)
    }

    /// Save to the default path with mode 0600. Creates the parent directory.
    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing contexts")?;
        // Write to a temp file then rename so partial writes can't leave a
        // half-written file on disk.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
        Ok(path)
    }

    /// Resolve the active context. None if `current_context` is unset or
    /// names a missing context.
    pub fn active(&self) -> Option<(&str, &ContextEntry)> {
        let name = self.current_context.as_deref()?;
        self.contexts.get(name).map(|c| (name, c))
    }

    pub fn set_context(&mut self, name: &str, server: String, token: String, role: String) {
        self.contexts.insert(
            name.to_string(),
            ContextEntry {
                server,
                token,
                role,
            },
        );
    }

    pub fn use_context(&mut self, name: &str) -> Result<()> {
        if !self.contexts.contains_key(name) {
            return Err(anyhow!("context not found: {}", name));
        }
        self.current_context = Some(name.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_serializes_and_parses() {
        let mut cfg = ContextsFile::default();
        cfg.set_context(
            "lab",
            "http://localhost:8122".into(),
            "tkn".into(),
            "admin".into(),
        );
        cfg.use_context("lab").unwrap();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: ContextsFile = toml::from_str(&text).unwrap();
        assert_eq!(back.current_context.as_deref(), Some("lab"));
        let active = back.active().unwrap();
        assert_eq!(active.0, "lab");
        assert_eq!(active.1.server, "http://localhost:8122");
        assert_eq!(active.1.token, "tkn");
        assert_eq!(active.1.role, "admin");
    }

    #[test]
    fn use_unknown_context_errors() {
        let mut cfg = ContextsFile::default();
        assert!(cfg.use_context("nope").is_err());
    }

    #[test]
    fn missing_role_defaults_to_operator() {
        let text = r#"
[contexts.lab]
server = "http://localhost:8122"
token = ""
"#;
        let cfg: ContextsFile = toml::from_str(text).unwrap();
        assert_eq!(cfg.contexts.get("lab").unwrap().role, "operator");
    }
}
