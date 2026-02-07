//! Task manifest parsing and validation
//!
//! Parses YAML task manifests and validates their structure.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::task::{ClaudeConfig, LifecycleConfig, RepositoryConfig, SecretRef, VmConfig};
use super::multi_agent::ChildrenConfig;

/// Task manifest as loaded from YAML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskManifest {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub metadata: ManifestMetadata,
    pub repository: RepositoryConfig,
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub vm: VmConfig,
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: ChildrenConfig,
}

fn default_version() -> String { "1".to_string() }
fn default_kind() -> String { "Task".to_string() }

/// Manifest metadata section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestMetadata {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

impl TaskManifest {
    /// Parse manifest from YAML string
    pub fn from_yaml(yaml: &str) -> Result<Self, ManifestError> {
        serde_yaml::from_str(yaml).map_err(ManifestError::ParseError)
    }

    /// Parse manifest from JSON string
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        serde_json::from_str(json).map_err(ManifestError::JsonParseError)
    }

    /// Validate manifest completeness and correctness
    pub fn validate(&self) -> Result<(), ManifestError> {
        // Check version
        if self.version != "1" {
            return Err(ManifestError::UnsupportedVersion(self.version.clone()));
        }

        // Check kind
        if self.kind != "Task" {
            return Err(ManifestError::InvalidKind(self.kind.clone()));
        }

        // Check required fields
        if self.metadata.id.is_empty() {
            return Err(ManifestError::MissingField("metadata.id".to_string()));
        }

        if self.metadata.name.is_empty() {
            return Err(ManifestError::MissingField("metadata.name".to_string()));
        }

        if self.repository.url.is_empty() {
            return Err(ManifestError::MissingField("repository.url".to_string()));
        }

        if self.repository.branch.is_empty() {
            return Err(ManifestError::MissingField("repository.branch".to_string()));
        }

        if self.claude.prompt.is_empty() {
            return Err(ManifestError::MissingField("claude.prompt".to_string()));
        }

        // Validate repository URL format
        if !self.repository.url.starts_with("git@")
            && !self.repository.url.starts_with("https://")
            && !self.repository.url.starts_with("http://")
        {
            return Err(ManifestError::InvalidField(
                "repository.url".to_string(),
                "Must be a valid git URL (https:// or git@)".to_string(),
            ));
        }

        // Validate VM config
        if self.vm.cpus == 0 {
            return Err(ManifestError::InvalidField(
                "vm.cpus".to_string(),
                "Must be at least 1".to_string(),
            ));
        }

        // Validate memory format
        if !self.vm.memory.ends_with('G') && !self.vm.memory.ends_with('M') {
            return Err(ManifestError::InvalidField(
                "vm.memory".to_string(),
                "Must end with G or M (e.g., 8G, 4096M)".to_string(),
            ));
        }

        // Validate timeout format
        if !self.lifecycle.timeout.is_empty()
            && !self.lifecycle.timeout.ends_with('h')
                && !self.lifecycle.timeout.ends_with('m')
                && !self.lifecycle.timeout.ends_with('s')
            {
                return Err(ManifestError::InvalidField(
                    "lifecycle.timeout".to_string(),
                    "Must end with h, m, or s (e.g., 24h, 30m)".to_string(),
                ));
            }

        // Validate failure action
        let valid_actions = ["destroy", "preserve"];
        if !valid_actions.contains(&self.lifecycle.failure_action.as_str()) {
            return Err(ManifestError::InvalidField(
                "lifecycle.failure_action".to_string(),
                format!("Must be one of: {:?}", valid_actions),
            ));
        }

        // Validate secrets
        for secret in &self.secrets {
            if secret.name.is_empty() {
                return Err(ManifestError::MissingField("secrets[].name".to_string()));
            }
            if secret.source.is_empty() {
                return Err(ManifestError::MissingField("secrets[].source".to_string()));
            }
            let valid_sources = ["env", "vault", "file"];
            if !valid_sources.contains(&secret.source.as_str()) {
                return Err(ManifestError::InvalidField(
                    format!("secrets[{}].source", secret.name),
                    format!("Must be one of: {:?}", valid_sources),
                ));
            }
        }

        Ok(())
    }

    /// Generate a new task ID if not provided
    pub fn with_generated_id(mut self) -> Self {
        if self.metadata.id.is_empty() {
            self.metadata.id = uuid::Uuid::new_v4().to_string();
        }
        self
    }
}

/// Manifest parsing and validation errors
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("Failed to parse YAML: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("Failed to parse JSON: {0}")]
    JsonParseError(#[from] serde_json::Error),

    #[error("Unsupported manifest version: {0}")]
    UnsupportedVersion(String),

    #[error("Invalid manifest kind: {0}")]
    InvalidKind(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid field {0}: {1}")]
    InvalidField(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let yaml = r#"
version: "1"
kind: Task
metadata:
  id: "test-task-1"
  name: "Test Task"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Fix the bug"
"#;

        let manifest = TaskManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.metadata.id, "test-task-1");
        assert_eq!(manifest.claude.headless, true);
    }

    #[test]
    fn test_parse_full_manifest() {
        let yaml = r#"
version: "1"
kind: Task
metadata:
  id: "full-task"
  name: "Full Task"
  labels:
    team: platform
    priority: high
repository:
  url: "git@github.com:org/repo.git"
  branch: "feature-branch"
  commit: "abc123"
claude:
  prompt: "Implement the feature"
  headless: true
  skip_permissions: true
  output_format: "stream-json"
  model: "claude-sonnet-4-5-20250929"
  allowed_tools:
    - Read
    - Write
    - Edit
    - Bash
vm:
  profile: "agentic-dev"
  cpus: 8
  memory: "16G"
  network_mode: outbound
  allowed_hosts:
    - "api.github.com"
secrets:
  - name: "ANTHROPIC_API_KEY"
    source: "env"
    key: "ANTHROPIC_API_KEY"
lifecycle:
  timeout: "6h"
  failure_action: "preserve"
  artifact_patterns:
    - "*.patch"
    - "reports/*.json"
"#;

        let manifest = TaskManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.vm.cpus, 8);
        assert_eq!(manifest.secrets.len(), 1);
    }

    #[test]
    fn test_parse_manifest_with_parent() {
        let yaml = r#"
version: "1"
kind: Task
metadata:
  id: "child-task"
  name: "Child Task"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Subtask"
parent_id: "parent-task-123"
"#;

        let manifest = TaskManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.parent_id, Some("parent-task-123".to_string()));
    }

    #[test]
    fn test_parse_manifest_with_children_config() {
        let yaml = r#"
version: "1"
kind: Task
metadata:
  id: "parent-task"
  name: "Parent Task"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Coordinate subtasks"
children:
  max_concurrent: 5
  wait_for_children: true
  aggregate_artifacts: true
"#;

        let manifest = TaskManifest::from_yaml(yaml).unwrap();
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.children.max_concurrent, Some(5));
        assert_eq!(manifest.children.wait_for_children, true);
        assert_eq!(manifest.children.aggregate_artifacts, true);
    }
}
