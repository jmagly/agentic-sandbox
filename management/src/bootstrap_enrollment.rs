//! Single-use bootstrap tokens for fleet agent enrollment.
//!
//! ADR-026 chooses in-agent key generation plus a short-lived one-time token
//! for fleet mTLS enrollment. This module provides the management-side token
//! primitive without wiring a CSR API yet.

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tracing::info;

use crate::transport_identity::SpiffeId;

const TOKENS_FILE: &str = "bootstrap-tokens.json";
const TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedBootstrapToken {
    /// Plaintext token. Only returned to the caller for one-time delivery to
    /// the target agent; it is never persisted by this store.
    pub token: String,
    pub instance_id: String,
    pub spiffe_id: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedBootstrapToken {
    pub instance_id: String,
    pub spiffe_id: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BootstrapTokenError {
    #[error("bootstrap token is unknown")]
    Unknown,
    #[error("bootstrap token is expired")]
    Expired,
    #[error("bootstrap token was already consumed")]
    AlreadyConsumed,
    #[error("bootstrap token is not valid for requested SPIFFE id")]
    SpiffeMismatch,
    #[error("bootstrap token state could not be persisted")]
    Persistence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    token_hash: String,
    instance_id: String,
    spiffe_id: String,
    expires_at_unix_ms: u64,
    consumed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TokenFile {
    records: Vec<TokenRecord>,
}

pub struct BootstrapTokenStore {
    path: PathBuf,
    records: parking_lot::RwLock<BTreeMap<String, TokenRecord>>,
}

impl BootstrapTokenStore {
    pub fn load_or_create(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        set_mode(dir, 0o700).with_context(|| format!("chmod 0700 {}", dir.display()))?;

        let path = dir.join(TOKENS_FILE);
        let records = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading bootstrap token store {}", path.display()))?;
            let file: TokenFile =
                serde_json::from_str(&raw).context("parsing bootstrap token store")?;
            file.records
                .into_iter()
                .map(|record| (record.token_hash.clone(), record))
                .collect()
        } else {
            BTreeMap::new()
        };

        if path.exists() {
            set_mode(&path, 0o600).with_context(|| format!("chmod 0600 {}", path.display()))?;
        }

        Ok(Self {
            path,
            records: parking_lot::RwLock::new(records),
        })
    }

    pub fn issue(
        &self,
        instance_id: impl Into<String>,
        spiffe_id: impl Into<String>,
        ttl: Duration,
    ) -> Result<IssuedBootstrapToken> {
        let instance_id = instance_id.into();
        let spiffe_id = spiffe_id.into();
        let parsed_spiffe = SpiffeId::parse(&spiffe_id)
            .with_context(|| format!("invalid bootstrap token SPIFFE id `{spiffe_id}`"))?;
        if parsed_spiffe.instance_id() != instance_id {
            anyhow::bail!(
                "bootstrap token SPIFFE id instance {} does not match requested instance {instance_id}",
                parsed_spiffe.instance_id()
            );
        }

        let token = generate_token();
        let token_hash = hash_token(&token);
        let expires_at_unix_ms = now_unix_ms().saturating_add(ttl.as_millis() as u64);

        let record = TokenRecord {
            token_hash: token_hash.clone(),
            instance_id: instance_id.clone(),
            spiffe_id: spiffe_id.clone(),
            expires_at_unix_ms,
            consumed_at_unix_ms: None,
        };

        self.records.write().insert(token_hash, record);
        self.save()?;
        info!(
            instance_id,
            spiffe_id,
            ttl_secs = ttl.as_secs(),
            "issued fleet bootstrap enrollment token"
        );

        Ok(IssuedBootstrapToken {
            token,
            instance_id,
            spiffe_id,
            expires_at_unix_ms,
        })
    }

    pub fn consume(
        &self,
        token: &str,
        requested_spiffe_id: &str,
    ) -> std::result::Result<ConsumedBootstrapToken, BootstrapTokenError> {
        let token_hash = hash_token(token);
        let now = now_unix_ms();
        let mut records = self.records.write();

        let Some(record) = records
            .values_mut()
            .find(|record| ct_hash_eq(&record.token_hash, &token_hash))
        else {
            return Err(BootstrapTokenError::Unknown);
        };

        if record.consumed_at_unix_ms.is_some() {
            return Err(BootstrapTokenError::AlreadyConsumed);
        }

        if now > record.expires_at_unix_ms {
            return Err(BootstrapTokenError::Expired);
        }

        if record.spiffe_id != requested_spiffe_id {
            return Err(BootstrapTokenError::SpiffeMismatch);
        }

        record.consumed_at_unix_ms = Some(now);
        let consumed = ConsumedBootstrapToken {
            instance_id: record.instance_id.clone(),
            spiffe_id: record.spiffe_id.clone(),
        };
        drop(records);

        if let Err(e) = self.save() {
            tracing::error!(error = %e, "failed to persist consumed bootstrap token state");
            return Err(BootstrapTokenError::Persistence);
        }

        Ok(consumed)
    }

    pub fn prune_expired_unconsumed(&self) -> Result<usize> {
        let now = now_unix_ms();
        let mut records = self.records.write();
        let before = records.len();
        records.retain(|_, record| {
            record.consumed_at_unix_ms.is_some() || record.expires_at_unix_ms >= now
        });
        let removed = before.saturating_sub(records.len());
        drop(records);

        if removed > 0 {
            self.save()?;
        }

        Ok(removed)
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            set_mode(parent, 0o700).with_context(|| format!("chmod 0700 {}", parent.display()))?;
        }

        let file = {
            let records = self.records.read();
            TokenFile {
                records: records.values().cloned().collect(),
            }
        };
        let raw = serde_json::to_string_pretty(&file).context("serializing bootstrap tokens")?;
        fs::write(&self.path, raw)
            .with_context(|| format!("writing bootstrap token store {}", self.path.display()))?;
        set_mode(&self.path, 0o600).with_context(|| format!("chmod 0600 {}", self.path.display()))
    }
}

fn generate_token() -> String {
    let mut bytes = [0_u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn ct_hash_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    a.len() == b.len() && a.ct_eq(b).into()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCE_ID: &str = "018fb9f1-3291-7a73-b261-c7de8a2af4d1";
    const SPIFFE_ID: &str = "spiffe://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";

    #[test]
    fn issued_token_is_not_persisted_in_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();

        let issued = store
            .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_secs(60))
            .unwrap();

        assert_eq!(issued.token.len(), TOKEN_BYTES * 2);
        let path = dir.path().join(TOKENS_FILE);
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains(&issued.token),
            "bootstrap token plaintext must not be persisted"
        );
        assert!(raw.contains("token_hash"));
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn issue_rejects_invalid_or_mismatched_spiffe_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();

        let err = store
            .issue(INSTANCE_ID, "https://not-spiffe", Duration::from_secs(60))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("invalid bootstrap token SPIFFE id"));

        let err = store
            .issue(
                INSTANCE_ID,
                "spiffe://sandbox.example/agent/018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1",
                Duration::from_secs(60),
            )
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("does not match requested instance"));
    }

    #[test]
    fn consume_succeeds_once_for_bound_spiffe_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
        let issued = store
            .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_secs(60))
            .unwrap();

        let consumed = store.consume(&issued.token, SPIFFE_ID).unwrap();
        assert_eq!(consumed.instance_id, INSTANCE_ID);
        assert_eq!(consumed.spiffe_id, SPIFFE_ID);

        let err = store.consume(&issued.token, SPIFFE_ID).unwrap_err();
        assert_eq!(err, BootstrapTokenError::AlreadyConsumed);
    }

    #[test]
    fn consumed_state_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let issued = {
            let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
            let issued = store
                .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_secs(60))
                .unwrap();
            store.consume(&issued.token, SPIFFE_ID).unwrap();
            issued
        };

        let reloaded = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
        let err = reloaded.consume(&issued.token, SPIFFE_ID).unwrap_err();
        assert_eq!(err, BootstrapTokenError::AlreadyConsumed);
    }

    #[test]
    fn consume_rejects_spiffe_mismatch_without_burning_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
        let issued = store
            .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_secs(60))
            .unwrap();

        let err = store
            .consume(
                &issued.token,
                "spiffe://sandbox.example/agent/018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1",
            )
            .unwrap_err();
        assert_eq!(err, BootstrapTokenError::SpiffeMismatch);

        assert!(store.consume(&issued.token, SPIFFE_ID).is_ok());
    }

    #[test]
    fn consume_rejects_expired_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
        let issued = store
            .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_millis(0))
            .unwrap();

        std::thread::sleep(Duration::from_millis(2));
        let err = store.consume(&issued.token, SPIFFE_ID).unwrap_err();
        assert_eq!(err, BootstrapTokenError::Expired);
    }

    #[test]
    fn prune_removes_expired_unconsumed_but_keeps_consumed_audit_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = BootstrapTokenStore::load_or_create(dir.path()).unwrap();
        let expired = store
            .issue(INSTANCE_ID, SPIFFE_ID, Duration::from_millis(0))
            .unwrap();
        let consumed = store
            .issue(
                "018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1",
                "spiffe://sandbox.example/agent/018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1",
                Duration::from_secs(60),
            )
            .unwrap();
        store.consume(&consumed.token, &consumed.spiffe_id).unwrap();

        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(store.prune_expired_unconsumed().unwrap(), 1);

        let raw = fs::read_to_string(dir.path().join(TOKENS_FILE)).unwrap();
        assert!(!raw.contains(&expired.spiffe_id));
        assert!(raw.contains(&consumed.spiffe_id));
        assert!(raw.contains("consumed_at_unix_ms"));
    }
}
