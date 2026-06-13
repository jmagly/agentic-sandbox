//! Authentication and secret management

use anyhow::Result;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

/// Constant-time hash equality. Both inputs are hex SHA-256 outputs (64
/// bytes) so length is fixed; we still gate on length to avoid panics if a
/// malformed entry slipped in. Per #264.
#[inline]
fn ct_hash_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    a.len() == b.len() && a.ct_eq(b).into()
}

/// A secret that has been provisioned to a VM but whose new hash has not
/// yet been observed in a successful agent verify. While pending, BOTH the
/// old (primary) and new hashes are accepted; the first verify against the
/// pending hash promotes it to primary and clears the entry.
struct PendingRotation {
    new_hash: String,
    /// After this instant the pending entry is dropped on the next access,
    /// reverting to the primary hash. Prevents indefinite growth if a
    /// rotation is launched and the agent never comes back.
    deadline: Instant,
}

/// Stores agent secrets (SHA256 hashed)
pub struct SecretStore {
    secrets_dir: PathBuf,
    /// agent_id -> SHA256 hash of the active (primary) secret
    hashes: RwLock<HashMap<String, String>>,
    /// agent_id -> pending rotation. Verifying against a pending hash
    /// commits it: pending → primary, entry removed.
    pending: RwLock<HashMap<String, PendingRotation>>,
}

impl SecretStore {
    pub fn new(secrets_dir: &str) -> Result<Self> {
        let path = PathBuf::from(secrets_dir);

        // Create directory if needed
        if !path.exists() {
            fs::create_dir_all(&path)?;
        }

        let store = Self {
            secrets_dir: path,
            hashes: RwLock::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
        };

        // Load existing hashes
        store.reload()?;

        Ok(store)
    }

    /// Reload secrets from disk
    pub fn reload(&self) -> Result<()> {
        let hashes_file = self.secrets_dir.join("agent-hashes.json");

        if hashes_file.exists() {
            let content = fs::read_to_string(&hashes_file)?;
            let hashes: HashMap<String, String> = serde_json::from_str(&content)?;
            let count = hashes.len();
            *self.hashes.write() = hashes;
            info!("Loaded {} agent secrets", count);
        }

        Ok(())
    }

    /// Save secrets to disk
    fn save(&self) -> Result<()> {
        let hashes_file = self.secrets_dir.join("agent-hashes.json");
        let hashes = self.hashes.read().clone();
        let content = serde_json::to_string_pretty(&hashes)?;
        fs::write(hashes_file, content)?;
        Ok(())
    }

    /// Verify agent secret.
    ///
    /// Order of checks:
    /// 1. Primary hash matches → accept (and drop any pending rotation —
    ///    operator must have re-run with the old secret on purpose).
    /// 2. Pending rotation hash matches and deadline not expired → accept
    ///    AND commit: promote pending hash to primary, clear pending entry.
    /// 3. No primary hash stored → reject. TOFU auto-registration was retired
    ///    in #412; provisioning must pre-register legacy hashes explicitly.
    pub fn verify(&self, agent_id: &str, secret: &str) -> bool {
        let hash = Self::hash_secret(secret);

        // Clone the stored hash and drop the read guard BEFORE potentially
        // calling register() (which needs a write lock).
        let stored = self.hashes.read().get(agent_id).cloned();

        match stored {
            Some(stored_hash) if ct_hash_eq(&stored_hash, &hash) => true,
            Some(_) => {
                // Primary mismatch — see if a pending rotation matches.
                let pending = {
                    let p = self.pending.read();
                    p.get(agent_id).map(|pr| (pr.new_hash.clone(), pr.deadline))
                };
                if let Some((pending_hash, deadline)) = pending {
                    if Instant::now() > deadline {
                        // Expired — drop and report mismatch.
                        self.pending.write().remove(agent_id);
                        return false;
                    }
                    if ct_hash_eq(&pending_hash, &hash) {
                        // Commit rotation: promote pending → primary.
                        self.hashes
                            .write()
                            .insert(agent_id.to_string(), pending_hash);
                        self.pending.write().remove(agent_id);
                        if let Err(e) = self.save() {
                            warn!(error = %e, agent_id, "rotation commit save failed");
                        }
                        info!(agent_id, "agent secret rotation committed");
                        return true;
                    }
                }
                false
            }
            None => {
                warn!(
                    agent_id,
                    "no stored legacy secret hash; rejecting unknown agent"
                );
                false
            }
        }
    }

    /// Stage a new secret for `agent_id`. The primary hash continues to
    /// authenticate until either:
    ///   (a) the agent re-registers with the new secret (commits rotation), or
    ///   (b) the deadline passes (rotation expires; primary unchanged).
    ///
    /// Returns the deadline so callers can surface it in an operation record.
    pub fn prepare_rotation(&self, agent_id: &str, new_secret: &str, grace: Duration) -> Instant {
        let new_hash = Self::hash_secret(new_secret);
        let deadline = Instant::now() + grace;
        self.pending
            .write()
            .insert(agent_id.to_string(), PendingRotation { new_hash, deadline });
        info!(
            agent_id,
            grace_secs = grace.as_secs(),
            "agent secret rotation prepared"
        );
        deadline
    }

    /// Drop a pending rotation without committing. Used when the SSH push
    /// fails before the agent reconnects, or the operator cancels.
    pub fn rollback_rotation(&self, agent_id: &str) -> bool {
        let removed = self.pending.write().remove(agent_id).is_some();
        if removed {
            info!(agent_id, "agent secret rotation rolled back");
        }
        removed
    }

    /// Whether a rotation is currently staged for `agent_id`.
    pub fn rotation_pending(&self, agent_id: &str) -> bool {
        self.pending.read().contains_key(agent_id)
    }

    /// Register a new agent secret
    pub fn register(&self, agent_id: &str, secret: &str) -> Result<()> {
        let hash = Self::hash_secret(secret);
        self.hashes.write().insert(agent_id.to_string(), hash);
        self.save()?;
        info!("Registered secret for agent: {}", agent_id);
        Ok(())
    }

    /// Remove an agent secret
    #[allow(dead_code)]
    pub fn remove(&self, agent_id: &str) -> Result<()> {
        self.hashes.write().remove(agent_id);
        self.save()?;
        Ok(())
    }

    /// Hash a secret using SHA256
    fn hash_secret(secret: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rotation_pending_accepts_both_old_and_new_until_committed() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path().to_str().unwrap()).unwrap();
        store.register("agent-01", "old").unwrap();

        let _ = store.prepare_rotation("agent-01", "new", Duration::from_secs(60));
        assert!(
            store.verify("agent-01", "old"),
            "old secret valid pre-commit"
        );
        assert!(store.rotation_pending("agent-01"));

        // First verify against new commits and clears pending.
        assert!(
            store.verify("agent-01", "new"),
            "new secret commits rotation"
        );
        assert!(!store.rotation_pending("agent-01"));
        assert!(
            !store.verify("agent-01", "old"),
            "old secret rejected after commit"
        );
        assert!(store.verify("agent-01", "new"), "new secret is the primary");
    }

    #[test]
    fn rotation_expires_after_deadline() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path().to_str().unwrap()).unwrap();
        store.register("agent-02", "old").unwrap();
        // Zero grace ⇒ already expired.
        store.prepare_rotation("agent-02", "new", Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(5));
        assert!(!store.verify("agent-02", "new"), "expired rotation refused");
        assert!(
            !store.rotation_pending("agent-02"),
            "expired entry dropped on access"
        );
        assert!(store.verify("agent-02", "old"), "old still primary");
    }

    #[test]
    fn rollback_drops_pending_without_touching_primary() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path().to_str().unwrap()).unwrap();
        store.register("agent-03", "old").unwrap();
        store.prepare_rotation("agent-03", "new", Duration::from_secs(60));
        assert!(store.rollback_rotation("agent-03"));
        assert!(!store.rotation_pending("agent-03"));
        assert!(store.verify("agent-03", "old"));
        assert!(!store.verify("agent-03", "new"));
    }

    #[test]
    fn verify_rejects_unknown_agent_without_tofu_registration() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path().to_str().unwrap()).unwrap();

        assert!(
            !store.verify("agent-04", "first-secret"),
            "unknown agents must not auto-register during verify"
        );
        assert!(
            !store.verify("agent-04", "first-secret"),
            "failed first verify must not create a stored hash"
        );
    }
}
