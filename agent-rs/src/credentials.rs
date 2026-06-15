//! Workload credential reference contract.
//!
//! The agent receives credential references as metadata only. Secret values are
//! resolved later through scoped leases and materialized under the runtime
//! credential directory.

use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const DEFAULT_CREDENTIAL_DIR: &str = "/run/agentic-sandbox/credentials";

#[derive(Debug, thiserror::Error)]
pub enum CredentialContractError {
    #[error("failed to read credential refs policy {path}: {source}")]
    ReadPolicy {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse credential refs policy {path}: {source}")]
    ParsePolicy {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("credential refs policy version must be 1, got {0}")]
    UnsupportedVersion(u32),

    #[error("credential_refs[{index}] requires non-empty id, provider, and allowed_use")]
    MissingRequiredField { index: usize },

    #[error("credential_refs[{index}].target requires type env|file and non-empty name")]
    InvalidTarget { index: usize },

    #[error("failed to create credential runtime directory {path}: {source}")]
    CreateRuntimeDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to chmod credential runtime directory {path}: {source}")]
    ChmodRuntimeDir {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialRefPolicy {
    pub version: u32,
    pub credential_refs: Vec<CredentialRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub id: String,
    pub provider: String,
    pub allowed_use: String,
    #[serde(default = "default_required")]
    pub required: bool,
    pub target: CredentialTarget,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialTarget {
    #[serde(rename = "type")]
    pub target_type: CredentialTargetType,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTargetType {
    Env,
    File,
}

#[derive(Debug, Clone)]
pub struct CredentialContract {
    policy_path: PathBuf,
    runtime_dir: PathBuf,
    policy: CredentialRefPolicy,
}

impl CredentialContract {
    pub fn load(
        policy_path: impl AsRef<Path>,
        runtime_dir: impl AsRef<Path>,
    ) -> Result<Self, CredentialContractError> {
        let policy_path = policy_path.as_ref().to_path_buf();
        let raw = fs::read_to_string(&policy_path).map_err(|source| {
            CredentialContractError::ReadPolicy {
                path: policy_path.clone(),
                source,
            }
        })?;
        let policy: CredentialRefPolicy =
            serde_json::from_str(&raw).map_err(|source| CredentialContractError::ParsePolicy {
                path: policy_path.clone(),
                source,
            })?;
        validate_policy(&policy)?;

        Ok(Self {
            policy_path,
            runtime_dir: runtime_dir.as_ref().to_path_buf(),
            policy,
        })
    }

    pub fn ensure_runtime_dir(&self) -> Result<(), CredentialContractError> {
        fs::create_dir_all(&self.runtime_dir).map_err(|source| {
            CredentialContractError::CreateRuntimeDir {
                path: self.runtime_dir.clone(),
                source,
            }
        })?;
        fs::set_permissions(&self.runtime_dir, fs::Permissions::from_mode(0o700)).map_err(
            |source| CredentialContractError::ChmodRuntimeDir {
                path: self.runtime_dir.clone(),
                source,
            },
        )
    }

    pub fn policy_path(&self) -> &Path {
        &self.policy_path
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn credential_refs(&self) -> &[CredentialRef] {
        &self.policy.credential_refs
    }
}

pub fn initialize_from_env() -> Result<Option<CredentialContract>, CredentialContractError> {
    let Some(policy_path) = env_nonempty("AGENTIC_CREDENTIAL_REFS") else {
        return Ok(None);
    };
    let runtime_dir = env_nonempty("AGENTIC_CREDENTIAL_DIR")
        .unwrap_or_else(|| DEFAULT_CREDENTIAL_DIR.to_string());
    let contract = CredentialContract::load(policy_path, runtime_dir)?;
    contract.ensure_runtime_dir()?;
    Ok(Some(contract))
}

fn validate_policy(policy: &CredentialRefPolicy) -> Result<(), CredentialContractError> {
    if policy.version != 1 {
        return Err(CredentialContractError::UnsupportedVersion(policy.version));
    }

    for (index, credential_ref) in policy.credential_refs.iter().enumerate() {
        if credential_ref.id.trim().is_empty()
            || credential_ref.provider.trim().is_empty()
            || credential_ref.allowed_use.trim().is_empty()
        {
            return Err(CredentialContractError::MissingRequiredField { index });
        }
        if credential_ref.target.name.trim().is_empty() {
            return Err(CredentialContractError::InvalidTarget { index });
        }
    }

    Ok(())
}

fn default_required() -> bool {
    true
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_policy(dir: &tempfile::TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("credential-refs.json");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn loads_metadata_only_policy_and_prepares_runtime_dir() {
        let dir = tempfile::tempdir().unwrap();
        let policy_path = write_policy(
            &dir,
            r#"{
              "version": 1,
              "credential_refs": [
                {
                  "id": "cred_anthropic_api",
                  "provider": "claude",
                  "allowed_use": "provider_api",
                  "required": true,
                  "target": { "type": "env", "name": "ANTHROPIC_API_KEY" }
                }
              ]
            }"#,
        );
        let runtime_dir = dir.path().join("runtime");

        let contract = CredentialContract::load(&policy_path, &runtime_dir).unwrap();
        contract.ensure_runtime_dir().unwrap();

        assert_eq!(contract.credential_refs().len(), 1);
        assert_eq!(contract.credential_refs()[0].id, "cred_anthropic_api");
        assert_eq!(
            contract.credential_refs()[0].target.target_type,
            CredentialTargetType::Env
        );
        assert_eq!(
            fs::metadata(&runtime_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn rejects_secret_like_inline_value_fields() {
        let dir = tempfile::tempdir().unwrap();
        let policy_path = write_policy(
            &dir,
            r#"{
              "version": 1,
              "credential_refs": [
                {
                  "id": "cred_bad",
                  "provider": "claude",
                  "allowed_use": "provider_api",
                  "value": "sk-ant-not-real",
                  "target": { "type": "env", "name": "ANTHROPIC_API_KEY" }
                }
              ]
            }"#,
        );

        let err = CredentialContract::load(&policy_path, dir.path().join("runtime")).unwrap_err();
        assert!(matches!(err, CredentialContractError::ParsePolicy { .. }));
    }

    #[test]
    fn rejects_empty_required_scope_fields() {
        let dir = tempfile::tempdir().unwrap();
        let policy_path = write_policy(
            &dir,
            r#"{
              "version": 1,
              "credential_refs": [
                {
                  "id": "cred_bad",
                  "provider": "",
                  "allowed_use": "provider_api",
                  "target": { "type": "file", "name": "provider-key" }
                }
              ]
            }"#,
        );

        let err = CredentialContract::load(&policy_path, dir.path().join("runtime")).unwrap_err();
        assert!(matches!(
            err,
            CredentialContractError::MissingRequiredField { index: 0 }
        ));
    }
}
