//! Task struct and state machine
//!
//! Represents a task being orchestrated through its lifecycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::manifest::TaskManifest;
use super::multi_agent::ChildrenConfig;
use super::OrchestratorError;

/// Task lifecycle states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Staging,
    Provisioning,
    Ready,
    Running,
    Completing,
    Completed,
    Failed,
    FailedPreserved,
    Cancelled,
}

impl TaskState {
    /// Check if transition to next state is valid
    pub fn can_transition_to(&self, next: TaskState) -> bool {
        use TaskState::*;
        match (self, next) {
            // Normal forward progression
            (Pending, Staging) => true,
            (Staging, Provisioning) => true,
            (Provisioning, Ready) => true,
            (Ready, Running) => true,
            (Running, Completing) => true,
            (Completing, Completed) => true,

            // Failure transitions
            (Staging, Failed) => true,
            (Provisioning, Failed) => true,
            (Ready, Failed) => true,
            (Running, Failed) => true,
            (Running, FailedPreserved) => true,
            (Completing, Failed) => true,

            // Cancellation from any active state
            (Pending, Cancelled) => true,
            (Staging, Cancelled) => true,
            (Provisioning, Cancelled) => true,
            (Ready, Cancelled) => true,
            (Running, Cancelled) => true,
            (Completing, Cancelled) => true,

            _ => false,
        }
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::FailedPreserved
                | TaskState::Cancelled
        )
    }

    /// Convert to proto enum value
    pub fn to_proto(&self) -> i32 {
        match self {
            TaskState::Pending => 1,
            TaskState::Staging => 2,
            TaskState::Provisioning => 3,
            TaskState::Ready => 4,
            TaskState::Running => 5,
            TaskState::Completing => 6,
            TaskState::Completed => 7,
            TaskState::Failed => 8,
            TaskState::FailedPreserved => 9,
            TaskState::Cancelled => 10,
        }
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskState::Pending => write!(f, "pending"),
            TaskState::Staging => write!(f, "staging"),
            TaskState::Provisioning => write!(f, "provisioning"),
            TaskState::Ready => write!(f, "ready"),
            TaskState::Running => write!(f, "running"),
            TaskState::Completing => write!(f, "completing"),
            TaskState::Completed => write!(f, "completed"),
            TaskState::Failed => write!(f, "failed"),
            TaskState::FailedPreserved => write!(f, "failed_preserved"),
            TaskState::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Repository configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryConfig {
    pub url: String,
    pub branch: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub subpath: Option<String>,
}

/// Claude execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub prompt: String,
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default = "default_true")]
    pub skip_permissions: bool,
    #[serde(default = "default_output_format")]
    pub output_format: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub mcp_config: Option<serde_json::Value>,
    #[serde(default)]
    pub max_turns: Option<u32>,
}

fn default_true() -> bool {
    true
}
fn default_output_format() -> String {
    "stream-json".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-5-20250929".to_string()
}

/// VM resource configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default = "default_cpus")]
    pub cpus: u32,
    #[serde(default = "default_memory")]
    pub memory: String,
    #[serde(default = "default_disk")]
    pub disk: String,
    #[serde(default)]
    pub network_mode: NetworkMode,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

fn default_profile() -> String {
    "agentic-dev".to_string()
}
fn default_cpus() -> u32 {
    4
}
fn default_memory() -> String {
    "8G".to_string()
}
fn default_disk() -> String {
    "40G".to_string()
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            cpus: default_cpus(),
            memory: default_memory(),
            disk: default_disk(),
            network_mode: NetworkMode::default(),
            allowed_hosts: vec![],
        }
    }
}

/// Network isolation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    #[default]
    Isolated,
    Outbound,
    Full,
}

/// Secret reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRef {
    pub name: String,
    pub source: String,
    pub key: String,
}

/// Lifecycle configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleConfig {
    #[serde(default = "default_timeout")]
    pub timeout: String,
    #[serde(default = "default_failure_action")]
    pub failure_action: String,
    #[serde(default)]
    pub artifact_patterns: Vec<String>,
}

fn default_timeout() -> String {
    "24h".to_string()
}
fn default_failure_action() -> String {
    "destroy".to_string()
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            failure_action: default_failure_action(),
            artifact_patterns: vec![],
        }
    }
}

/// Execution progress tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskProgress {
    pub output_bytes: u64,
    pub tool_calls: u32,
    pub current_tool: Option<String>,
    pub last_activity_at: Option<DateTime<Utc>>,
}

/// A task being orchestrated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub labels: HashMap<String, String>,

    pub repository: RepositoryConfig,
    pub claude: ClaudeConfig,
    pub vm: VmConfig,
    pub secrets: Vec<SecretRef>,
    pub lifecycle: LifecycleConfig,

    // Multi-agent orchestration fields
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: ChildrenConfig,

    // Runtime state
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub state_changed_at: DateTime<Utc>,
    pub state_message: Option<String>,

    // VM info (set when provisioned)
    pub vm_name: Option<String>,
    pub vm_ip: Option<String>,

    // Completion info
    pub exit_code: Option<i32>,
    pub error: Option<String>,

    // Progress tracking
    pub progress: TaskProgress,
}

impl Task {
    /// Create a new task from manifest
    pub fn from_manifest(manifest: TaskManifest) -> Result<Self, OrchestratorError> {
        let now = Utc::now();
        Ok(Self {
            id: manifest.metadata.id,
            name: manifest.metadata.name,
            labels: manifest.metadata.labels,
            repository: manifest.repository,
            claude: manifest.claude,
            vm: manifest.vm,
            secrets: manifest.secrets,
            lifecycle: manifest.lifecycle,
            parent_id: manifest.parent_id,
            children: manifest.children,
            state: TaskState::Pending,
            created_at: now,
            started_at: None,
            state_changed_at: now,
            state_message: None,
            vm_name: None,
            vm_ip: None,
            exit_code: None,
            error: None,
            progress: TaskProgress::default(),
        })
    }

    /// Transition to a new state
    pub fn transition_to(&mut self, next: TaskState) -> Result<(), OrchestratorError> {
        if !self.state.can_transition_to(next) {
            return Err(OrchestratorError::InvalidTransition(
                self.state.to_string(),
                next.to_string(),
            ));
        }

        let now = Utc::now();
        self.state = next;
        self.state_changed_at = now;

        // Set started_at on first active state
        if self.started_at.is_none() && next != TaskState::Pending {
            self.started_at = Some(now);
        }

        // Set state message
        self.state_message = Some(match next {
            TaskState::Pending => "Waiting to start".to_string(),
            TaskState::Staging => "Cloning repository and preparing workspace".to_string(),
            TaskState::Provisioning => "Creating VM".to_string(),
            TaskState::Ready => "VM ready, starting execution".to_string(),
            TaskState::Running => "Claude Code executing".to_string(),
            TaskState::Completing => "Collecting artifacts".to_string(),
            TaskState::Completed => "Task completed successfully".to_string(),
            TaskState::Failed => "Task failed".to_string(),
            TaskState::FailedPreserved => "Task failed, VM preserved for debugging".to_string(),
            TaskState::Cancelled => "Task cancelled".to_string(),
        });

        Ok(())
    }

    /// Update progress
    pub fn update_progress(
        &mut self,
        bytes: u64,
        tool_calls: Option<u32>,
        current_tool: Option<String>,
    ) {
        self.progress.output_bytes += bytes;
        if let Some(tc) = tool_calls {
            self.progress.tool_calls = tc;
        }
        self.progress.current_tool = current_tool;
        self.progress.last_activity_at = Some(Utc::now());
    }
}
