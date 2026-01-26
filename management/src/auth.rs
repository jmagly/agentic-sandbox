//! Authentication and secret management

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::{info, warn};

/// Stores agent secrets (SHA256 hashed)
pub struct SecretStore {
    secrets_dir: PathBuf,
    /// agent_id -> SHA256 hash of secret
    hashes: RwLock<HashMap<String, String>>,
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
            *self.hashes.write().unwrap() = hashes;
            info!("Loaded {} agent secrets", self.hashes.read().unwrap().len());
        }

        Ok(())
    }

    /// Save secrets to disk
    fn save(&self) -> Result<()> {
        let hashes_file = self.secrets_dir.join("agent-hashes.json");
        let content = serde_json::to_string_pretty(&*self.hashes.read().unwrap())?;
        fs::write(hashes_file, content)?;
        Ok(())
    }

    /// Verify agent secret
    pub fn verify(&self, agent_id: &str, secret: &str) -> bool {
        let hash = Self::hash_secret(secret);

        if let Some(stored_hash) = self.hashes.read().unwrap().get(agent_id) {
            stored_hash == &hash
        } else {
            // If no hash stored, auto-register on first connect
            warn!("No secret found for {}, auto-registering", agent_id);
            self.register(agent_id, secret).is_ok()
        }
    }

    /// Register a new agent secret
    pub fn register(&self, agent_id: &str, secret: &str) -> Result<()> {
        let hash = Self::hash_secret(secret);
        self.hashes.write().unwrap().insert(agent_id.to_string(), hash);
        self.save()?;
        info!("Registered secret for agent: {}", agent_id);
        Ok(())
    }

    /// Remove an agent secret
    #[allow(dead_code)]
    pub fn remove(&self, agent_id: &str) -> Result<()> {
        self.hashes.write().unwrap().remove(agent_id);
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
