//! Claude Code task executor
//!
//! Provides structured execution of Claude Code CLI with output streaming
//! and proper error handling.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for a Claude Code task
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClaudeTaskConfig {
    /// Task identifier
    pub task_id: String,

    /// Prompt to send to Claude
    pub prompt: String,

    /// Working directory for execution
    pub working_dir: String,

    /// Session ID for Claude (enables session continuity)
    pub session_id: String,

    /// MCP server configuration JSON
    #[serde(default)]
    pub mcp_config: Option<String>,

    /// Allowed tools (if not specified, all tools are allowed)
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Model to use (defaults to Claude's default)
    #[serde(default)]
    pub model: Option<String>,

    /// Environment variable name containing API key
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

fn default_api_key_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}

// =============================================================================
// Output Streaming
// =============================================================================

/// A chunk of output from Claude Code execution
#[derive(Debug, Clone)]
pub struct OutputChunk {
    /// Stream identifier ("stdout" or "stderr")
    pub stream: String,

    /// Output data
    pub data: String,

    /// Timestamp (Unix milliseconds)
    pub timestamp: i64,
}

impl OutputChunk {
    fn stdout(data: String) -> Self {
        Self {
            stream: "stdout".to_string(),
            data,
            timestamp: current_timestamp_ms(),
        }
    }

    fn stderr(data: String) -> Self {
        Self {
            stream: "stderr".to_string(),
            data,
            timestamp: current_timestamp_ms(),
        }
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during Claude Code execution
#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    /// Claude Code CLI not found in PATH
    #[error("Claude Code not found in PATH")]
    NotFound,

    /// API key not set in environment
    #[error("API key not set: {0}")]
    ApiKeyMissing(String),

    /// Failed to spawn Claude process
    #[error("Failed to spawn: {0}")]
    SpawnFailed(#[from] std::io::Error),

    /// Working directory does not exist
    #[error("Working directory missing: {0}")]
    WorkingDirMissing(String),

    /// Process was killed by signal
    #[error("Process killed by signal")]
    Killed,
}

// =============================================================================
// Claude Runner
// =============================================================================

/// Executor for Claude Code tasks
pub struct ClaudeRunner {
    config: ClaudeTaskConfig,
}

impl ClaudeRunner {
    /// Create a new Claude runner with the given configuration
    pub fn new(config: ClaudeTaskConfig) -> Self {
        Self { config }
    }

    /// Check if the claude CLI is available in PATH
    pub async fn check_available() -> bool {
        Command::new("claude")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Validate the configuration before execution
    fn validate(&self) -> Result<(), ClaudeError> {
        // Check working directory exists
        if !std::path::Path::new(&self.config.working_dir).exists() {
            return Err(ClaudeError::WorkingDirMissing(
                self.config.working_dir.clone(),
            ));
        }

        // Check API key is set in environment
        if std::env::var(&self.config.api_key_env).is_err() {
            return Err(ClaudeError::ApiKeyMissing(
                self.config.api_key_env.clone(),
            ));
        }

        Ok(())
    }

    /// Build the command arguments for Claude CLI
    fn build_args(&self) -> Vec<String> {
        let mut args = vec![
            "--print".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        // Add session ID
        if !self.config.session_id.is_empty() {
            args.push("--session-id".to_string());
            args.push(self.config.session_id.clone());
        }

        // Add model if specified
        if let Some(ref model) = self.config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        // Add MCP config if specified
        if let Some(ref mcp_config) = self.config.mcp_config {
            args.push("--mcp-config".to_string());
            args.push(mcp_config.clone());
        }

        // Add allowed tools if specified
        if !self.config.allowed_tools.is_empty() {
            args.push("--allowedTools".to_string());
            args.push(self.config.allowed_tools.join(","));
        }

        // Add prompt (must be last positional argument)
        args.push(self.config.prompt.clone());

        args
    }

    /// Execute the Claude Code task and stream output
    ///
    /// Returns the exit code on success.
    pub async fn run(
        &self,
        output_tx: mpsc::Sender<OutputChunk>,
    ) -> Result<i32, ClaudeError> {
        // Validate configuration
        self.validate()?;

        let args = self.build_args();

        info!(
            "[{}] Running Claude with {} args",
            self.config.task_id,
            args.len()
        );
        debug!("[{}] Claude args: {:?}", self.config.task_id, args);

        // Spawn Claude process
        let mut child = Command::new("claude")
            .args(&args)
            .current_dir(&self.config.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ClaudeError::NotFound
                } else {
                    ClaudeError::SpawnFailed(e)
                }
            })?;

        // Extract stdout and stderr
        let stdout = child
            .stdout
            .take()
            .expect("stdout should be piped");
        let stderr = child
            .stderr
            .take()
            .expect("stderr should be piped");

        // Spawn task to stream stdout
        let tx_stdout = output_tx.clone();
        let task_id = self.config.task_id.clone();
        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let chunk = OutputChunk::stdout(format!("{}\n", line));
                if tx_stdout.send(chunk).await.is_err() {
                    warn!("[{}] Output receiver dropped", task_id);
                    break;
                }
            }
        });

        // Spawn task to stream stderr
        let tx_stderr = output_tx.clone();
        let task_id = self.config.task_id.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let chunk = OutputChunk::stderr(format!("{}\n", line));
                if tx_stderr.send(chunk).await.is_err() {
                    warn!("[{}] Output receiver dropped", task_id);
                    break;
                }
            }
        });

        // Wait for process to complete
        let exit_status = child.wait().await?;

        // Wait for output tasks to finish
        let _ = tokio::join!(stdout_task, stderr_task);

        // Get exit code
        let exit_code = match exit_status.code() {
            Some(code) => {
                info!("[{}] Claude exited with code {}", self.config.task_id, code);
                code
            }
            None => {
                error!("[{}] Claude killed by signal", self.config.task_id);
                return Err(ClaudeError::Killed);
            }
        };

        Ok(exit_code)
    }
}

// =============================================================================
// Utilities
// =============================================================================

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_deserialization() {
        let json = r#"{
            "task_id": "test-123",
            "prompt": "write a hello world program",
            "working_dir": "/tmp/workspace",
            "session_id": "session-456"
        }"#;

        let config: ClaudeTaskConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.task_id, "test-123");
        assert_eq!(config.prompt, "write a hello world program");
        assert_eq!(config.working_dir, "/tmp/workspace");
        assert_eq!(config.session_id, "session-456");
        assert_eq!(config.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_config_with_optional_fields() {
        let json = r#"{
            "task_id": "test-123",
            "prompt": "test prompt",
            "working_dir": "/tmp",
            "session_id": "sess-1",
            "model": "claude-opus-4-5-20251101",
            "allowed_tools": ["bash", "read", "write"],
            "mcp_config": "/path/to/mcp.json",
            "api_key_env": "CLAUDE_KEY"
        }"#;

        let config: ClaudeTaskConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, Some("claude-opus-4-5-20251101".to_string()));
        assert_eq!(config.allowed_tools, vec!["bash", "read", "write"]);
        assert_eq!(config.mcp_config, Some("/path/to/mcp.json".to_string()));
        assert_eq!(config.api_key_env, "CLAUDE_KEY");
    }

    #[test]
    fn test_build_args_minimal() {
        let config = ClaudeTaskConfig {
            task_id: "test-1".to_string(),
            prompt: "hello world".to_string(),
            working_dir: "/tmp".to_string(),
            session_id: "sess-1".to_string(),
            mcp_config: None,
            allowed_tools: vec![],
            model: None,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        };

        let runner = ClaudeRunner::new(config);
        let args = runner.build_args();

        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&"sess-1".to_string()));
        assert_eq!(args.last(), Some(&"hello world".to_string()));
    }

    #[test]
    fn test_build_args_with_options() {
        let config = ClaudeTaskConfig {
            task_id: "test-1".to_string(),
            prompt: "test".to_string(),
            working_dir: "/tmp".to_string(),
            session_id: "sess-1".to_string(),
            mcp_config: Some("/mcp.json".to_string()),
            allowed_tools: vec!["bash".to_string(), "read".to_string()],
            model: Some("claude-sonnet-4-5".to_string()),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        };

        let runner = ClaudeRunner::new(config);
        let args = runner.build_args();

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"claude-sonnet-4-5".to_string()));
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/mcp.json".to_string()));
        assert!(args.contains(&"--allowedTools".to_string()));
        assert!(args.contains(&"bash,read".to_string()));
    }

    #[test]
    fn test_validate_missing_directory() {
        let config = ClaudeTaskConfig {
            task_id: "test-1".to_string(),
            prompt: "test".to_string(),
            working_dir: "/nonexistent/directory".to_string(),
            session_id: "sess-1".to_string(),
            mcp_config: None,
            allowed_tools: vec![],
            model: None,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        };

        let runner = ClaudeRunner::new(config);
        let result = runner.validate();

        assert!(result.is_err());
        match result {
            Err(ClaudeError::WorkingDirMissing(path)) => {
                assert_eq!(path, "/nonexistent/directory");
            }
            _ => panic!("Expected WorkingDirMissing error"),
        }
    }

    #[test]
    fn test_validate_missing_api_key() {
        // Create temp directory for test
        let temp_dir = std::env::temp_dir();

        // Ensure the custom env var is NOT set
        std::env::remove_var("TEST_MISSING_KEY");

        let config = ClaudeTaskConfig {
            task_id: "test-1".to_string(),
            prompt: "test".to_string(),
            working_dir: temp_dir.to_string_lossy().to_string(),
            session_id: "sess-1".to_string(),
            mcp_config: None,
            allowed_tools: vec![],
            model: None,
            api_key_env: "TEST_MISSING_KEY".to_string(),
        };

        let runner = ClaudeRunner::new(config);
        let result = runner.validate();

        assert!(result.is_err());
        match result {
            Err(ClaudeError::ApiKeyMissing(key)) => {
                assert_eq!(key, "TEST_MISSING_KEY");
            }
            _ => panic!("Expected ApiKeyMissing error"),
        }
    }

    #[test]
    fn test_output_chunk_creation() {
        let chunk = OutputChunk::stdout("test output\n".to_string());
        assert_eq!(chunk.stream, "stdout");
        assert_eq!(chunk.data, "test output\n");
        assert!(chunk.timestamp > 0);

        let chunk = OutputChunk::stderr("error message\n".to_string());
        assert_eq!(chunk.stream, "stderr");
        assert_eq!(chunk.data, "error message\n");
    }

    #[tokio::test]
    async fn test_check_available_with_mock() {
        // This test will fail if claude is not installed, which is expected
        // In a real test environment, we'd mock the command
        let available = ClaudeRunner::check_available().await;
        // We don't assert here since claude may or may not be installed
        println!("Claude available: {}", available);
    }
}
