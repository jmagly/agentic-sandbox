//! Workload credential metadata broker.
//!
//! This is the first broker slice for ADR-028: it stores provider credential
//! metadata and backend references while keeping submitted secret values out of
//! durable records and API responses.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential id is required")]
    MissingId,
    #[error("credential provider is required")]
    MissingProvider,
    #[error("credential type is required")]
    MissingType,
    #[error("credential already exists: {0}")]
    AlreadyExists(String),
    #[error("credential not found: {0}")]
    NotFound(String),
    #[error("credential is not configured: {0}")]
    NotConfigured(String),
    #[error("credential lease denied: {0}")]
    LeaseDenied(String),
    #[error("credential lease not found: {0}")]
    LeaseNotFound(String),
    #[error("credential backend is not supported for local materialization: {0}")]
    UnsupportedBackend(String),
    #[error("credential backend read failed for {kind} reference {reference}: {source}")]
    BackendRead {
        kind: String,
        reference: String,
        #[source]
        source: std::io::Error,
    },
    #[error("credential persistence failed: {0}")]
    Persistence(#[from] std::io::Error),
    #[error("credential serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialBackendRef {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialSet {
    pub id: String,
    pub provider: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub allowed_uses: Vec<String>,
    #[serde(default)]
    pub backend: Option<CredentialBackendRef>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_rotated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialMetadataResponse {
    pub id: String,
    pub provider: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub allowed_uses: Vec<String>,
    #[serde(default)]
    pub backend: Option<CredentialBackendRef>,
    pub configured: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_rotated_at: Option<DateTime<Utc>>,
}

impl CredentialSet {
    fn response(&self, configured: bool) -> CredentialMetadataResponse {
        CredentialMetadataResponse {
            id: self.id.clone(),
            provider: self.provider.clone(),
            credential_type: self.credential_type.clone(),
            owner: self.owner.clone(),
            scopes: self.scopes.clone(),
            allowed_uses: self.allowed_uses.clone(),
            backend: self.backend.clone(),
            configured,
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_rotated_at: self.last_rotated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CredentialValueInput {
    pub kind: String,
    #[serde(default)]
    pub plaintext: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertCredentialRequest {
    pub id: String,
    pub provider: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub allowed_uses: Vec<String>,
    #[serde(default)]
    pub backend: Option<CredentialBackendRef>,
    #[serde(default)]
    pub value: Option<CredentialValueInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialLeaseState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialLease {
    pub id: String,
    pub credential_id: String,
    pub agent_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub provider: String,
    pub allowed_use: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: CredentialLeaseState,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialLeaseResponse {
    pub id: String,
    pub credential_id: String,
    pub agent_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub provider: String,
    pub allowed_use: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: CredentialLeaseState,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

impl CredentialLease {
    fn response(&self) -> CredentialLeaseResponse {
        CredentialLeaseResponse {
            id: self.id.clone(),
            credential_id: self.credential_id.clone(),
            agent_id: self.agent_id.clone(),
            instance_id: self.instance_id.clone(),
            session_id: self.session_id.clone(),
            provider: self.provider.clone(),
            allowed_use: self.allowed_use.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            state: self.state.clone(),
            revoked_at: self.revoked_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueCredentialLeaseRequest {
    pub agent_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub provider: String,
    pub allowed_use: String,
    #[serde(default = "default_lease_ttl_seconds")]
    pub ttl_seconds: i64,
}

fn default_lease_ttl_seconds() -> i64 {
    900
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialStoreFile {
    version: u32,
    credentials: Vec<CredentialSet>,
    #[serde(default)]
    leases: Vec<CredentialLease>,
}

#[derive(Default)]
struct CredentialBrokerInner {
    credentials: BTreeMap<String, CredentialSet>,
    plaintext_values: HashMap<String, String>,
    leases: BTreeMap<String, CredentialLease>,
}

#[derive(Clone)]
pub struct CredentialBroker {
    inner: Arc<RwLock<CredentialBrokerInner>>,
    store_path: Option<PathBuf>,
}

impl Default for CredentialBroker {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

impl CredentialBroker {
    pub fn new_in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(CredentialBrokerInner::default())),
            store_path: None,
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, CredentialError> {
        let path = path.as_ref().to_path_buf();
        let broker = Self {
            inner: Arc::new(RwLock::new(CredentialBrokerInner::default())),
            store_path: Some(path.clone()),
        };
        if path.exists() {
            let bytes = fs::read(&path)?;
            let parsed: CredentialStoreFile = serde_json::from_slice(&bytes)?;
            let mut inner = broker.inner.write();
            inner.credentials = parsed
                .credentials
                .into_iter()
                .map(|credential| (credential.id.clone(), credential))
                .collect();
            inner.leases = parsed
                .leases
                .into_iter()
                .map(|lease| (lease.id.clone(), lease))
                .collect();
        }
        Ok(broker)
    }

    pub fn create(
        &self,
        request: UpsertCredentialRequest,
    ) -> Result<CredentialMetadataResponse, CredentialError> {
        self.validate_request(&request)?;
        let now = Utc::now();
        let plaintext = secret_plaintext(&request);
        let credential = CredentialSet {
            id: request.id.clone(),
            provider: request.provider,
            credential_type: request.credential_type,
            owner: request.owner,
            scopes: request.scopes,
            allowed_uses: request.allowed_uses,
            backend: request.backend,
            created_at: now,
            updated_at: now,
            last_rotated_at: plaintext.as_ref().map(|_| now),
        };

        let mut inner = self.inner.write();
        if inner.credentials.contains_key(&request.id) {
            return Err(CredentialError::AlreadyExists(request.id));
        }
        let configured = credential.backend.is_some() || plaintext.is_some();
        if let Some(value) = plaintext {
            inner.plaintext_values.insert(credential.id.clone(), value);
        }
        inner
            .credentials
            .insert(credential.id.clone(), credential.clone());
        drop(inner);
        self.persist()?;
        Ok(credential.response(configured))
    }

    pub fn update(
        &self,
        id: &str,
        request: UpsertCredentialRequest,
    ) -> Result<CredentialMetadataResponse, CredentialError> {
        if id != request.id {
            return Err(CredentialError::NotFound(id.to_string()));
        }
        self.validate_request(&request)?;
        let now = Utc::now();
        let plaintext = secret_plaintext(&request);

        let mut inner = self.inner.write();
        let Some(existing) = inner.credentials.get(id).cloned() else {
            return Err(CredentialError::NotFound(id.to_string()));
        };
        let mut updated = CredentialSet {
            id: request.id.clone(),
            provider: request.provider,
            credential_type: request.credential_type,
            owner: request.owner,
            scopes: request.scopes,
            allowed_uses: request.allowed_uses,
            backend: request.backend,
            created_at: existing.created_at,
            updated_at: now,
            last_rotated_at: existing.last_rotated_at,
        };
        if let Some(value) = plaintext {
            inner.plaintext_values.insert(updated.id.clone(), value);
            updated.last_rotated_at = Some(now);
        }
        let configured = updated.backend.is_some() || inner.plaintext_values.contains_key(id);
        inner.credentials.insert(id.to_string(), updated.clone());
        drop(inner);
        self.persist()?;
        Ok(updated.response(configured))
    }

    pub fn list(&self) -> Vec<CredentialMetadataResponse> {
        let inner = self.inner.read();
        inner
            .credentials
            .values()
            .map(|credential| {
                let configured = credential.backend.is_some()
                    || inner.plaintext_values.contains_key(&credential.id);
                credential.response(configured)
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<CredentialMetadataResponse, CredentialError> {
        let inner = self.inner.read();
        let Some(credential) = inner.credentials.get(id) else {
            return Err(CredentialError::NotFound(id.to_string()));
        };
        let configured = credential.backend.is_some() || inner.plaintext_values.contains_key(id);
        Ok(credential.response(configured))
    }

    pub fn delete(&self, id: &str) -> Result<(), CredentialError> {
        let mut inner = self.inner.write();
        if inner.credentials.remove(id).is_none() {
            return Err(CredentialError::NotFound(id.to_string()));
        }
        inner.plaintext_values.remove(id);
        for lease in inner.leases.values_mut() {
            if lease.credential_id == id && lease.state == CredentialLeaseState::Active {
                lease.state = CredentialLeaseState::Revoked;
                lease.revoked_at = Some(Utc::now());
            }
        }
        drop(inner);
        self.persist()
    }

    pub fn issue_lease(
        &self,
        credential_id: &str,
        request: IssueCredentialLeaseRequest,
    ) -> Result<CredentialLeaseResponse, CredentialError> {
        self.validate_lease_request(&request)?;
        if request.ttl_seconds <= 0 {
            return Err(CredentialError::LeaseDenied(
                "ttl_seconds must be greater than zero".to_string(),
            ));
        }

        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(request.ttl_seconds);
        let mut inner = self.inner.write();
        let Some(credential) = inner.credentials.get(credential_id).cloned() else {
            return Err(CredentialError::NotFound(credential_id.to_string()));
        };
        let configured =
            credential.backend.is_some() || inner.plaintext_values.contains_key(credential_id);
        if !configured {
            return Err(CredentialError::NotConfigured(credential_id.to_string()));
        }
        if credential.provider != request.provider {
            return Err(CredentialError::LeaseDenied(format!(
                "provider mismatch for credential {credential_id}"
            )));
        }
        if !credential.allowed_uses.is_empty()
            && !credential
                .allowed_uses
                .iter()
                .any(|allowed| allowed == &request.allowed_use)
        {
            return Err(CredentialError::LeaseDenied(format!(
                "use {} is not allowed for credential {credential_id}",
                request.allowed_use
            )));
        }

        let lease = CredentialLease {
            id: format!("lease_{}", uuid::Uuid::now_v7().simple()),
            credential_id: credential_id.to_string(),
            agent_id: request.agent_id,
            instance_id: request.instance_id,
            session_id: request.session_id,
            provider: request.provider,
            allowed_use: request.allowed_use,
            issued_at: now,
            expires_at,
            state: CredentialLeaseState::Active,
            revoked_at: None,
        };
        inner.leases.insert(lease.id.clone(), lease.clone());
        drop(inner);
        self.persist()?;
        Ok(lease.response())
    }

    pub fn list_leases(&self) -> Vec<CredentialLeaseResponse> {
        self.inner
            .read()
            .leases
            .values()
            .map(CredentialLease::response)
            .collect()
    }

    pub fn get_lease(&self, id: &str) -> Result<CredentialLeaseResponse, CredentialError> {
        self.inner
            .read()
            .leases
            .get(id)
            .map(CredentialLease::response)
            .ok_or_else(|| CredentialError::LeaseNotFound(id.to_string()))
    }

    pub fn plaintext_for_active_lease(
        &self,
        lease_id: &str,
        agent_id: &str,
        instance_id: &str,
        session_id: &str,
    ) -> Result<String, CredentialError> {
        let backend = {
            let inner = self.inner.read();
            let Some(lease) = inner.leases.get(lease_id) else {
                return Err(CredentialError::LeaseNotFound(lease_id.to_string()));
            };
            if lease.state != CredentialLeaseState::Active || lease.expires_at <= Utc::now() {
                return Err(CredentialError::LeaseDenied(format!(
                    "lease {lease_id} is not active"
                )));
            }
            if lease.agent_id != agent_id
                || lease.instance_id != instance_id
                || lease.session_id != session_id
            {
                return Err(CredentialError::LeaseDenied(format!(
                    "lease {lease_id} scope mismatch"
                )));
            }
            if let Some(value) = inner.plaintext_values.get(&lease.credential_id) {
                return Ok(value.clone());
            }
            let Some(credential) = inner.credentials.get(&lease.credential_id) else {
                return Err(CredentialError::NotFound(lease.credential_id.clone()));
            };
            credential
                .backend
                .clone()
                .ok_or_else(|| CredentialError::NotConfigured(lease.credential_id.clone()))?
        };
        resolve_backend_plaintext(&backend)
    }

    pub fn revoke_lease(&self, id: &str) -> Result<CredentialLeaseResponse, CredentialError> {
        let mut inner = self.inner.write();
        let Some(lease) = inner.leases.get_mut(id) else {
            return Err(CredentialError::LeaseNotFound(id.to_string()));
        };
        if lease.state == CredentialLeaseState::Active {
            lease.state = CredentialLeaseState::Revoked;
            lease.revoked_at = Some(Utc::now());
        }
        let response = lease.response();
        drop(inner);
        self.persist()?;
        Ok(response)
    }

    pub fn revoke_leases_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<CredentialLeaseResponse>, CredentialError> {
        let mut inner = self.inner.write();
        let now = Utc::now();
        let mut responses = Vec::new();
        for lease in inner.leases.values_mut() {
            if lease.session_id == session_id && lease.state == CredentialLeaseState::Active {
                lease.state = CredentialLeaseState::Revoked;
                lease.revoked_at = Some(now);
                responses.push(lease.response());
            }
        }
        drop(inner);
        self.persist()?;
        Ok(responses)
    }

    #[cfg(test)]
    fn plaintext_value_for_test(&self, id: &str) -> Option<String> {
        self.inner.read().plaintext_values.get(id).cloned()
    }

    fn validate_request(&self, request: &UpsertCredentialRequest) -> Result<(), CredentialError> {
        if request.id.trim().is_empty() {
            return Err(CredentialError::MissingId);
        }
        if request.provider.trim().is_empty() {
            return Err(CredentialError::MissingProvider);
        }
        if request.credential_type.trim().is_empty() {
            return Err(CredentialError::MissingType);
        }
        Ok(())
    }

    fn validate_lease_request(
        &self,
        request: &IssueCredentialLeaseRequest,
    ) -> Result<(), CredentialError> {
        for (field, value) in [
            ("agent_id", &request.agent_id),
            ("instance_id", &request.instance_id),
            ("session_id", &request.session_id),
            ("provider", &request.provider),
            ("allowed_use", &request.allowed_use),
        ] {
            if value.trim().is_empty() {
                return Err(CredentialError::LeaseDenied(format!("{field} is required")));
            }
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), CredentialError> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let inner = self.inner.read();
        let file = CredentialStoreFile {
            version: 1,
            credentials: inner.credentials.values().cloned().collect(),
            leases: inner.leases.values().cloned().collect(),
        };
        let json = serde_json::to_vec_pretty(&file)?;
        fs::write(path, json)?;
        Ok(())
    }
}

fn secret_plaintext(request: &UpsertCredentialRequest) -> Option<String> {
    let value = request.value.as_ref()?;
    if value.kind == "write_only" {
        return value.plaintext.clone();
    }
    None
}

fn resolve_backend_plaintext(backend: &CredentialBackendRef) -> Result<String, CredentialError> {
    match backend.kind.as_str() {
        "file" | "local_file" => {
            let path = PathBuf::from(&backend.reference);
            if !path.is_absolute() {
                return Err(CredentialError::LeaseDenied(
                    "file credential backend references must be absolute paths".to_string(),
                ));
            }
            fs::read_to_string(&path).map_err(|source| CredentialError::BackendRead {
                kind: backend.kind.clone(),
                reference: backend.reference.clone(),
                source,
            })
        }
        kind => Err(CredentialError::UnsupportedBackend(kind.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_secret(secret: &str) -> UpsertCredentialRequest {
        UpsertCredentialRequest {
            id: "cred_openai_test".to_string(),
            provider: "openai".to_string(),
            credential_type: "api_key".to_string(),
            owner: Some("platform".to_string()),
            scopes: vec!["codex:run".to_string()],
            allowed_uses: vec!["session.launch".to_string()],
            backend: None,
            value: Some(CredentialValueInput {
                kind: "write_only".to_string(),
                plaintext: Some(secret.to_string()),
            }),
        }
    }

    fn lease_request(allowed_use: &str) -> IssueCredentialLeaseRequest {
        IssueCredentialLeaseRequest {
            agent_id: "agent-01".to_string(),
            instance_id: "instance-01".to_string(),
            session_id: "session-01".to_string(),
            provider: "openai".to_string(),
            allowed_use: allowed_use.to_string(),
            ttl_seconds: 60,
        }
    }

    #[test]
    fn write_only_value_is_not_in_response_json() {
        let broker = CredentialBroker::new_in_memory();
        let response = broker
            .create(request_with_secret("sk-test-secret"))
            .unwrap();

        assert!(response.configured);
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("sk-test-secret"));
        assert!(!json.contains("plaintext"));
        assert_eq!(
            broker.plaintext_value_for_test("cred_openai_test"),
            Some("sk-test-secret".to_string())
        );
    }

    #[test]
    fn write_only_value_is_not_persisted_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let broker = CredentialBroker::open(&path).unwrap();
        broker
            .create(request_with_secret("sk-persist-secret"))
            .unwrap();

        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("cred_openai_test"));
        assert!(!persisted.contains("sk-persist-secret"));
        assert!(!persisted.contains("plaintext"));

        let reloaded = CredentialBroker::open(&path).unwrap();
        let response = reloaded.get("cred_openai_test").unwrap();
        assert!(!response.configured);
        assert_eq!(reloaded.plaintext_value_for_test("cred_openai_test"), None);
    }

    #[test]
    fn backend_reference_can_be_persisted_without_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let broker = CredentialBroker::open(&path).unwrap();
        broker
            .create(UpsertCredentialRequest {
                id: "cred_github".to_string(),
                provider: "github".to_string(),
                credential_type: "token".to_string(),
                owner: None,
                scopes: vec!["repo:read".to_string()],
                allowed_uses: vec!["session.launch".to_string()],
                backend: Some(CredentialBackendRef {
                    kind: "vault_ref".to_string(),
                    reference: "kv/agentic/github".to_string(),
                }),
                value: None,
            })
            .unwrap();

        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("vault_ref"));
        assert!(!persisted.contains("token_value"));
        assert!(
            CredentialBroker::open(&path)
                .unwrap()
                .get("cred_github")
                .unwrap()
                .configured
        );
    }

    #[test]
    fn file_backend_materializes_only_for_active_matching_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("credentials.json");
        let secret_path = dir.path().join("github-token");
        fs::write(&secret_path, "ghp-file-secret").unwrap();

        let broker = CredentialBroker::open(&store_path).unwrap();
        broker
            .create(UpsertCredentialRequest {
                id: "cred_github_file".to_string(),
                provider: "github".to_string(),
                credential_type: "token".to_string(),
                owner: None,
                scopes: vec!["repo:read".to_string()],
                allowed_uses: vec!["session.launch".to_string()],
                backend: Some(CredentialBackendRef {
                    kind: "file".to_string(),
                    reference: secret_path.to_string_lossy().into_owned(),
                }),
                value: None,
            })
            .unwrap();

        let persisted = fs::read_to_string(&store_path).unwrap();
        assert!(persisted.contains("cred_github_file"));
        assert!(persisted.contains("github-token"));
        assert!(!persisted.contains("ghp-file-secret"));

        let reloaded = CredentialBroker::open(&store_path).unwrap();
        let lease = reloaded
            .issue_lease(
                "cred_github_file",
                IssueCredentialLeaseRequest {
                    agent_id: "agent-01".to_string(),
                    instance_id: "instance-01".to_string(),
                    session_id: "session-01".to_string(),
                    provider: "github".to_string(),
                    allowed_use: "session.launch".to_string(),
                    ttl_seconds: 60,
                },
            )
            .unwrap();

        assert_eq!(
            reloaded
                .plaintext_for_active_lease(&lease.id, "agent-01", "instance-01", "session-01")
                .unwrap(),
            "ghp-file-secret"
        );
        assert!(matches!(
            reloaded.plaintext_for_active_lease(&lease.id, "agent-01", "instance-02", "session-01"),
            Err(CredentialError::LeaseDenied(_))
        ));
    }

    #[test]
    fn lease_issuance_is_scoped_and_persisted_without_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let broker = CredentialBroker::open(&path).unwrap();
        broker
            .create(request_with_secret("sk-lease-secret"))
            .unwrap();

        let lease = broker
            .issue_lease("cred_openai_test", lease_request("session.launch"))
            .unwrap();
        assert_eq!(lease.credential_id, "cred_openai_test");
        assert_eq!(lease.agent_id, "agent-01");
        assert_eq!(lease.instance_id, "instance-01");
        assert_eq!(lease.session_id, "session-01");
        assert_eq!(lease.provider, "openai");
        assert_eq!(lease.allowed_use, "session.launch");
        assert_eq!(lease.state, CredentialLeaseState::Active);

        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains(&lease.id));
        assert!(persisted.contains("session-01"));
        assert!(!persisted.contains("sk-lease-secret"));
        assert!(!persisted.contains("plaintext"));
    }

    #[test]
    fn plaintext_materialization_requires_active_matching_lease_scope() {
        let broker = CredentialBroker::new_in_memory();
        broker
            .create(request_with_secret("sk-materialize-secret"))
            .unwrap();
        let lease = broker
            .issue_lease("cred_openai_test", lease_request("session.launch"))
            .unwrap();

        let materialized = broker
            .plaintext_for_active_lease(&lease.id, "agent-01", "instance-01", "session-01")
            .unwrap();
        assert_eq!(materialized, "sk-materialize-secret");

        assert!(matches!(
            broker.plaintext_for_active_lease(&lease.id, "agent-02", "instance-01", "session-01"),
            Err(CredentialError::LeaseDenied(_))
        ));

        broker.revoke_lease(&lease.id).unwrap();
        assert!(matches!(
            broker.plaintext_for_active_lease(&lease.id, "agent-01", "instance-01", "session-01"),
            Err(CredentialError::LeaseDenied(_))
        ));
    }

    #[test]
    fn lease_issuance_denies_wrong_provider_or_use() {
        let broker = CredentialBroker::new_in_memory();
        broker
            .create(request_with_secret("sk-denied-secret"))
            .unwrap();

        let mut wrong_provider = lease_request("session.launch");
        wrong_provider.provider = "anthropic".to_string();
        assert!(matches!(
            broker.issue_lease("cred_openai_test", wrong_provider),
            Err(CredentialError::LeaseDenied(_))
        ));

        assert!(matches!(
            broker.issue_lease("cred_openai_test", lease_request("readiness.probe")),
            Err(CredentialError::LeaseDenied(_))
        ));
    }

    #[test]
    fn lease_issuance_requires_configured_credential() {
        let broker = CredentialBroker::new_in_memory();
        let mut request = request_with_secret("sk-unused");
        request.value = None;
        broker.create(request).unwrap();

        assert!(matches!(
            broker.issue_lease("cred_openai_test", lease_request("session.launch")),
            Err(CredentialError::NotConfigured(id)) if id == "cred_openai_test"
        ));
    }

    #[test]
    fn revoke_lease_marks_metadata_without_deleting_record() {
        let broker = CredentialBroker::new_in_memory();
        broker
            .create(request_with_secret("sk-revoke-secret"))
            .unwrap();
        let lease = broker
            .issue_lease("cred_openai_test", lease_request("session.launch"))
            .unwrap();

        let revoked = broker.revoke_lease(&lease.id).unwrap();
        assert_eq!(revoked.state, CredentialLeaseState::Revoked);
        assert!(revoked.revoked_at.is_some());
        assert_eq!(
            broker.get_lease(&lease.id).unwrap().state,
            CredentialLeaseState::Revoked
        );
    }

    #[test]
    fn revoke_leases_for_session_only_revokes_matching_active_leases() {
        let broker = CredentialBroker::new_in_memory();
        broker
            .create(request_with_secret("sk-session-revoke-secret"))
            .unwrap();

        let lease_a = broker
            .issue_lease("cred_openai_test", lease_request("session.launch"))
            .unwrap();
        let mut other_session_request = lease_request("session.launch");
        other_session_request.session_id = "session-02".to_string();
        let lease_b = broker
            .issue_lease("cred_openai_test", other_session_request)
            .unwrap();

        let revoked = broker.revoke_leases_for_session("session-01").unwrap();
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].id, lease_a.id);
        assert_eq!(
            broker.get_lease(&lease_a.id).unwrap().state,
            CredentialLeaseState::Revoked
        );
        assert_eq!(
            broker.get_lease(&lease_b.id).unwrap().state,
            CredentialLeaseState::Active
        );
    }
}
