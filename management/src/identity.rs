//! Persistent sandbox identity.
//!
//! On first start a UUID v4 is generated and written to `identity_path`.
//! Subsequent starts reload the same UUID so the sandbox always presents a
//! stable identity to `aiwg serve` regardless of restarts or re-registrations.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;
use uuid::Uuid;

pub struct SandboxIdentity {
    pub id: String,
    path: PathBuf,
}

impl SandboxIdentity {
    /// Load identity from `path`, or generate and persist a new one.
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();

        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading identity file {}", path.display()))?;
            let id = raw.trim().to_string();
            // Validate it's actually a UUID so we don't silently use garbage.
            Uuid::parse_str(&id)
                .with_context(|| format!("identity file {} contains invalid UUID", path.display()))?;
            Ok(Self { id, path })
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating identity dir {}", parent.display()))?;
            }
            let id = Uuid::now_v7().to_string();
            std::fs::write(&path, &id)
                .with_context(|| format!("writing identity file {}", path.display()))?;
            info!(id = %id, path = %path.display(), "Generated new sandbox identity");
            Ok(Self { id, path })
        }
    }

    /// Default identity file path derived from the secrets dir.
    pub fn default_path(secrets_dir: &str) -> PathBuf {
        Path::new(secrets_dir)
            .parent()
            .unwrap_or(Path::new("/var/lib/agentic-sandbox"))
            .join("identity")
    }
}
