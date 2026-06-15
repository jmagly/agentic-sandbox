//! Startup profile policy store.
//!
//! Startup profiles are durable, non-secret launch policies. They reference
//! workload credentials by id and scope; they never carry provider tokens or
//! key material directly.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum StartupProfileError {
    #[error("startup profile not found: {0}")]
    NotFound(String),
    #[error("startup profile already exists: {0}")]
    AlreadyExists(String),
    #[error("startup profile validation failed: {0}")]
    Validation(String),
    #[error("startup profile persistence failed: {0}")]
    Persistence(#[from] std::io::Error),
    #[error("startup profile serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupTrigger {
    OnInstanceReady,
    OnReconnect,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    Pending,
    WaitingForAgent,
    WaitingForCredentials,
    Launching,
    Running,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupSessionBackend {
    Tmux,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupSessionClass {
    Managed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupTarget {
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub loadout: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupSessionSpec {
    pub command: String,
    #[serde(default = "default_workdir")]
    pub workdir: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default = "default_session_backend")]
    pub backend: StartupSessionBackend,
    #[serde(default = "default_session_class")]
    pub class: StartupSessionClass,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartupCredentialRef {
    pub id: String,
    pub provider: String,
    pub allowed_use: String,
    #[serde(default = "default_required")]
    pub required: bool,
    pub target: StartupCredentialTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartupCredentialTarget {
    #[serde(rename = "type")]
    pub target_type: StartupCredentialTargetType,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupCredentialTargetType {
    Env,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupReadinessProbe {
    pub kind: String,
    pub command: String,
    #[serde(default = "default_readiness_timeout_seconds")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupObservationPolicy {
    #[serde(default = "default_true")]
    pub transcript_enabled: bool,
    #[serde(default = "default_retention_class")]
    pub retention_class: String,
    #[serde(default = "default_redaction_profile")]
    pub redaction_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupControlPolicy {
    #[serde(default = "default_observer_role")]
    pub default_role: String,
    #[serde(default)]
    pub controller_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupRestartMode {
    Never,
    OnFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupRestartPolicy {
    #[serde(default = "default_restart_mode")]
    pub mode: StartupRestartMode,
    #[serde(default)]
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupStatus {
    pub state: StartupState,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub command_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupProfile {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    pub trigger: StartupTrigger,
    pub target: StartupTarget,
    pub session: StartupSessionSpec,
    #[serde(default)]
    pub credential_refs: Vec<StartupCredentialRef>,
    #[serde(default)]
    pub readiness_probes: Vec<StartupReadinessProbe>,
    #[serde(default = "StartupObservationPolicy::default")]
    pub observation: StartupObservationPolicy,
    #[serde(default = "StartupControlPolicy::default")]
    pub control: StartupControlPolicy,
    #[serde(default = "StartupRestartPolicy::default")]
    pub restart: StartupRestartPolicy,
    pub status: StartupStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupProfileBinding {
    pub instance_id: String,
    pub profile_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertStartupProfileRequest {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_trigger")]
    pub trigger: StartupTrigger,
    #[serde(default)]
    pub target: Option<StartupTarget>,
    pub session: StartupSessionSpec,
    #[serde(default)]
    pub credential_refs: Vec<StartupCredentialRef>,
    #[serde(default)]
    pub readiness_probes: Vec<StartupReadinessProbe>,
    #[serde(default = "StartupObservationPolicy::default")]
    pub observation: StartupObservationPolicy,
    #[serde(default = "StartupControlPolicy::default")]
    pub control: StartupControlPolicy,
    #[serde(default = "StartupRestartPolicy::default")]
    pub restart: StartupRestartPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartupProfileStoreFile {
    version: u32,
    profiles: Vec<StartupProfile>,
    #[serde(default)]
    bindings: Vec<StartupProfileBinding>,
}

#[derive(Default)]
struct StartupProfileStoreInner {
    profiles: BTreeMap<String, StartupProfile>,
    bindings: BTreeMap<String, StartupProfileBinding>,
}

#[derive(Clone)]
pub struct StartupProfileStore {
    inner: Arc<RwLock<StartupProfileStoreInner>>,
    store_path: Option<PathBuf>,
}

impl Default for StartupProfileStore {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

impl StartupProfileStore {
    pub fn new_in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StartupProfileStoreInner::default())),
            store_path: None,
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StartupProfileError> {
        let path = path.as_ref().to_path_buf();
        let store = Self {
            inner: Arc::new(RwLock::new(StartupProfileStoreInner::default())),
            store_path: Some(path.clone()),
        };
        if path.exists() {
            let bytes = fs::read(&path)?;
            let parsed: StartupProfileStoreFile = serde_json::from_slice(&bytes)?;
            let mut inner = store.inner.write();
            inner.profiles = parsed
                .profiles
                .into_iter()
                .map(|profile| (profile.id.clone(), profile))
                .collect();
            inner.bindings = parsed
                .bindings
                .into_iter()
                .map(|binding| (binding.instance_id.clone(), binding))
                .collect();
        }
        Ok(store)
    }

    pub fn create(
        &self,
        request: UpsertStartupProfileRequest,
    ) -> Result<StartupProfile, StartupProfileError> {
        let id = request
            .id
            .clone()
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("startup_{}", uuid::Uuid::now_v7().simple()));
        validate_profile_request(&id, &request)?;

        let now = Utc::now();
        let profile = StartupProfile {
            id: id.clone(),
            description: request.description,
            trigger: request.trigger,
            target: request.target.unwrap_or_default(),
            session: request.session,
            credential_refs: request.credential_refs,
            readiness_probes: request.readiness_probes,
            observation: request.observation,
            control: request.control,
            restart: request.restart,
            status: StartupStatus {
                state: StartupState::Pending,
                reason: None,
                session_id: None,
                command_id: None,
                updated_at: now,
            },
            created_at: now,
            updated_at: now,
        };

        let mut inner = self.inner.write();
        if inner.profiles.contains_key(&id) {
            return Err(StartupProfileError::AlreadyExists(id));
        }
        inner.profiles.insert(id, profile.clone());
        self.persist_locked(&inner)?;
        Ok(profile)
    }

    pub fn update(
        &self,
        id: &str,
        request: UpsertStartupProfileRequest,
    ) -> Result<StartupProfile, StartupProfileError> {
        validate_profile_request(id, &request)?;
        let mut inner = self.inner.write();
        let Some(existing) = inner.profiles.get(id).cloned() else {
            return Err(StartupProfileError::NotFound(id.to_string()));
        };
        let now = Utc::now();
        let updated = StartupProfile {
            id: id.to_string(),
            description: request.description,
            trigger: request.trigger,
            target: request.target.unwrap_or_default(),
            session: request.session,
            credential_refs: request.credential_refs,
            readiness_probes: request.readiness_probes,
            observation: request.observation,
            control: request.control,
            restart: request.restart,
            status: existing.status,
            created_at: existing.created_at,
            updated_at: now,
        };
        inner.profiles.insert(id.to_string(), updated.clone());
        self.persist_locked(&inner)?;
        Ok(updated)
    }

    pub fn list(&self) -> Vec<StartupProfile> {
        self.inner.read().profiles.values().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Result<StartupProfile, StartupProfileError> {
        self.inner
            .read()
            .profiles
            .get(id)
            .cloned()
            .ok_or_else(|| StartupProfileError::NotFound(id.to_string()))
    }

    pub fn matching_ready_profiles(
        &self,
        agent_id: &str,
        instance_id: &str,
        loadout: &str,
    ) -> Vec<StartupProfile> {
        let inner = self.inner.read();
        if let Some(binding) = inner.bindings.get(instance_id) {
            return inner
                .profiles
                .get(&binding.profile_id)
                .filter(|profile| ready_profile_can_launch(profile))
                .filter(|profile| target_matches(&profile.target, agent_id, instance_id, loadout))
                .cloned()
                .into_iter()
                .collect();
        }

        inner
            .profiles
            .values()
            .filter(|profile| ready_profile_can_launch(profile))
            .filter(|profile| target_matches(&profile.target, agent_id, instance_id, loadout))
            .cloned()
            .collect()
    }

    pub fn bind_instance_profile(
        &self,
        instance_id: &str,
        profile_id: &str,
    ) -> Result<StartupProfileBinding, StartupProfileError> {
        if instance_id.trim().is_empty() {
            return Err(StartupProfileError::Validation(
                "instance_id is required".to_string(),
            ));
        }
        if profile_id.trim().is_empty() {
            return Err(StartupProfileError::Validation(
                "profile_id is required".to_string(),
            ));
        }

        let mut inner = self.inner.write();
        if !inner.profiles.contains_key(profile_id) {
            return Err(StartupProfileError::NotFound(profile_id.to_string()));
        }
        let binding = StartupProfileBinding {
            instance_id: instance_id.to_string(),
            profile_id: profile_id.to_string(),
            created_at: Utc::now(),
        };
        inner
            .bindings
            .insert(instance_id.to_string(), binding.clone());
        self.persist_locked(&inner)?;
        Ok(binding)
    }

    pub fn unbind_instance_profile(&self, instance_id: &str) -> Result<(), StartupProfileError> {
        let mut inner = self.inner.write();
        inner.bindings.remove(instance_id);
        self.persist_locked(&inner)
    }

    pub fn bound_profile_id(&self, instance_id: &str) -> Option<String> {
        self.inner
            .read()
            .bindings
            .get(instance_id)
            .map(|binding| binding.profile_id.clone())
    }

    pub fn update_status(
        &self,
        id: &str,
        state: StartupState,
        reason: Option<String>,
        session_id: Option<String>,
        command_id: Option<String>,
    ) -> Result<StartupProfile, StartupProfileError> {
        let mut inner = self.inner.write();
        let Some(profile) = inner.profiles.get_mut(id) else {
            return Err(StartupProfileError::NotFound(id.to_string()));
        };
        let now = Utc::now();
        profile.status = StartupStatus {
            state,
            reason,
            session_id,
            command_id,
            updated_at: now,
        };
        profile.updated_at = now;
        let updated = profile.clone();
        self.persist_locked(&inner)?;
        Ok(updated)
    }

    pub fn delete(&self, id: &str) -> Result<(), StartupProfileError> {
        let mut inner = self.inner.write();
        if inner.profiles.remove(id).is_none() {
            return Err(StartupProfileError::NotFound(id.to_string()));
        }
        inner.bindings.retain(|_, binding| binding.profile_id != id);
        self.persist_locked(&inner)
    }

    fn persist_locked(&self, inner: &StartupProfileStoreInner) -> Result<(), StartupProfileError> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let payload = StartupProfileStoreFile {
            version: 2,
            profiles: inner.profiles.values().cloned().collect(),
            bindings: inner.bindings.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&payload)?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

impl Default for StartupTarget {
    fn default() -> Self {
        Self {
            instance_id: None,
            agent_id: None,
            loadout: None,
            runtime: None,
            provider: None,
        }
    }
}

impl Default for StartupObservationPolicy {
    fn default() -> Self {
        Self {
            transcript_enabled: true,
            retention_class: default_retention_class(),
            redaction_profile: default_redaction_profile(),
        }
    }
}

impl Default for StartupControlPolicy {
    fn default() -> Self {
        Self {
            default_role: default_observer_role(),
            controller_allowed: false,
        }
    }
}

impl Default for StartupRestartPolicy {
    fn default() -> Self {
        Self {
            mode: StartupRestartMode::Never,
            max_attempts: 0,
        }
    }
}

fn validate_profile_request(
    id: &str,
    request: &UpsertStartupProfileRequest,
) -> Result<(), StartupProfileError> {
    if id.trim().is_empty() {
        return Err(StartupProfileError::Validation(
            "profile id is required".to_string(),
        ));
    }
    if request.session.command.trim().is_empty() {
        return Err(StartupProfileError::Validation(
            "session.command is required".to_string(),
        ));
    }
    if request.session.workdir.trim().is_empty() {
        return Err(StartupProfileError::Validation(
            "session.workdir is required".to_string(),
        ));
    }
    if request.session.cols == 0 || request.session.rows == 0 {
        return Err(StartupProfileError::Validation(
            "session cols/rows must be greater than zero".to_string(),
        ));
    }
    if request.restart.mode == StartupRestartMode::Never && request.restart.max_attempts > 0 {
        return Err(StartupProfileError::Validation(
            "restart.max_attempts requires restart.mode=on_failure".to_string(),
        ));
    }

    for (index, credential_ref) in request.credential_refs.iter().enumerate() {
        if credential_ref.id.trim().is_empty()
            || credential_ref.provider.trim().is_empty()
            || credential_ref.allowed_use.trim().is_empty()
        {
            return Err(StartupProfileError::Validation(format!(
                "credential_refs[{index}] requires id, provider, and allowed_use"
            )));
        }
        if credential_ref.target.name.trim().is_empty() {
            return Err(StartupProfileError::Validation(format!(
                "credential_refs[{index}].target.name is required"
            )));
        }
    }

    for (index, probe) in request.readiness_probes.iter().enumerate() {
        if probe.kind.trim().is_empty() || probe.command.trim().is_empty() {
            return Err(StartupProfileError::Validation(format!(
                "readiness_probes[{index}] requires kind and command"
            )));
        }
        if probe.timeout_seconds == 0 {
            return Err(StartupProfileError::Validation(format!(
                "readiness_probes[{index}].timeout_seconds must be greater than zero"
            )));
        }
    }

    Ok(())
}

fn ready_profile_can_launch(profile: &StartupProfile) -> bool {
    profile.trigger == StartupTrigger::OnInstanceReady
        && !matches!(
            profile.status.state,
            StartupState::Launching | StartupState::Running
        )
}

fn target_matches(
    target: &StartupTarget,
    agent_id: &str,
    instance_id: &str,
    loadout: &str,
) -> bool {
    target
        .agent_id
        .as_deref()
        .is_none_or(|wanted| wanted == agent_id)
        && target
            .instance_id
            .as_deref()
            .is_none_or(|wanted| wanted == instance_id)
        && target
            .loadout
            .as_deref()
            .is_none_or(|wanted| wanted == loadout)
}

fn default_trigger() -> StartupTrigger {
    StartupTrigger::OnInstanceReady
}

fn default_workdir() -> String {
    "/home/agent/workspace".to_string()
}

fn default_session_backend() -> StartupSessionBackend {
    StartupSessionBackend::Tmux
}

fn default_session_class() -> StartupSessionClass {
    StartupSessionClass::Managed
}

fn default_cols() -> u16 {
    220
}

fn default_rows() -> u16 {
    50
}

fn default_required() -> bool {
    true
}

fn default_readiness_timeout_seconds() -> u64 {
    30
}

fn default_true() -> bool {
    true
}

fn default_retention_class() -> String {
    "credentialed_session".to_string()
}

fn default_redaction_profile() -> String {
    "credentialed".to_string()
}

fn default_observer_role() -> String {
    "observer".to_string()
}

fn default_restart_mode() -> StartupRestartMode {
    StartupRestartMode::Never
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request_json() -> serde_json::Value {
        json!({
            "id": "startup_codex",
            "description": "Launch Codex after Ready",
            "trigger": "on_instance_ready",
            "target": {
                "loadout": "automation-control",
                "provider": "codex"
            },
            "session": {
                "command": "agentic-codex-automation --profile startup_codex",
                "workdir": "/home/agent/workspace",
                "backend": "tmux",
                "class": "managed"
            },
            "credential_refs": [
                {
                    "id": "cred_openai_api",
                    "provider": "codex",
                    "allowed_use": "provider_api",
                    "target": { "type": "env", "name": "OPENAI_API_KEY" }
                }
            ],
            "readiness_probes": [
                {
                    "kind": "command",
                    "command": "agentic-provider-readiness codex",
                    "timeout_seconds": 10
                }
            ],
            "observation": {
                "transcript_enabled": true,
                "retention_class": "credentialed_session",
                "redaction_profile": "credentialed"
            },
            "control": {
                "default_role": "observer",
                "controller_allowed": false
            },
            "restart": {
                "mode": "on_failure",
                "max_attempts": 2
            }
        })
    }

    #[test]
    fn startup_profile_persists_without_secret_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup-profiles.json");
        let store = StartupProfileStore::open(&path).unwrap();
        let request: UpsertStartupProfileRequest = serde_json::from_value(request_json()).unwrap();

        let created = store.create(request).unwrap();
        assert_eq!(created.id, "startup_codex");
        assert_eq!(created.status.state, StartupState::Pending);

        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("credential_refs"));
        assert!(persisted.contains("cred_openai_api"));
        assert!(!persisted.contains("plaintext"));
        assert!(!persisted.contains("sk-"));

        let reopened = StartupProfileStore::open(&path).unwrap();
        assert_eq!(reopened.get("startup_codex").unwrap().id, "startup_codex");
    }

    #[test]
    fn instance_binding_limits_ready_selection_to_bound_profile() {
        let store = StartupProfileStore::new_in_memory();
        store
            .create(serde_json::from_value(request_json()).unwrap())
            .unwrap();

        let mut second = request_json();
        second["id"] = json!("startup_claude");
        second["target"]["loadout"] = json!("automation-control");
        store
            .create(serde_json::from_value(second).unwrap())
            .unwrap();

        store
            .bind_instance_profile("instance-1", "startup_claude")
            .unwrap();

        let matched = store.matching_ready_profiles("agent-1", "instance-1", "automation-control");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "startup_claude");

        let generic = store.matching_ready_profiles("agent-2", "instance-2", "automation-control");
        assert_eq!(generic.len(), 2);
    }

    #[test]
    fn instance_binding_persists_and_is_removed_with_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup-profiles.json");
        let store = StartupProfileStore::open(&path).unwrap();
        store
            .create(serde_json::from_value(request_json()).unwrap())
            .unwrap();
        store
            .bind_instance_profile("instance-1", "startup_codex")
            .unwrap();

        let reopened = StartupProfileStore::open(&path).unwrap();
        assert_eq!(
            reopened.bound_profile_id("instance-1").as_deref(),
            Some("startup_codex")
        );

        reopened.delete("startup_codex").unwrap();
        assert_eq!(reopened.bound_profile_id("instance-1"), None);
    }

    #[test]
    fn startup_profile_rejects_inline_secret_like_fields() {
        let mut payload = request_json();
        payload["credential_refs"][0]["value"] = json!("sk-not-real");
        let parsed = serde_json::from_value::<UpsertStartupProfileRequest>(payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn startup_profile_validates_required_session_and_credential_scope() {
        let store = StartupProfileStore::new_in_memory();
        let mut payload = request_json();
        payload["session"]["command"] = json!("");
        let request: UpsertStartupProfileRequest = serde_json::from_value(payload).unwrap();
        let err = store.create(request).unwrap_err();
        assert!(matches!(err, StartupProfileError::Validation(_)));

        let mut payload = request_json();
        payload["credential_refs"][0]["allowed_use"] = json!("");
        let request: UpsertStartupProfileRequest = serde_json::from_value(payload).unwrap();
        let err = store.create(request).unwrap_err();
        assert!(matches!(err, StartupProfileError::Validation(_)));
    }
}
