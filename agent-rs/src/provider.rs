//! Provider-neutral task execution and the built-in provider registry.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TaskConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub task_id: String,
    pub prompt: String,
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub mcp_config: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

fn default_provider() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone)]
pub struct OutputChunk {
    pub stream: String,
    pub data: String,
    pub timestamp: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("unknown task provider: {0}")]
    UnknownProvider(String),
    #[error("provider executable not found: {0}")]
    NotFound(String),
    #[error("provider credential not available: {0}")]
    CredentialMissing(String),
    #[error("working directory missing: {0}")]
    WorkingDirMissing(String),
    #[error("failed to spawn provider: {0}")]
    SpawnFailed(#[from] std::io::Error),
    #[error("provider process killed by signal")]
    Killed,
}

/// Provider-specific command construction. Identity gating, validation,
/// spawning, output streaming, and errors remain single-sourced in `run`.
pub trait ProviderExecutor: Send + Sync {
    fn name(&self) -> &'static str;
    fn executable(&self) -> &'static str;
    fn credential_env(&self, config: &TaskConfig) -> String;
    fn credential_file(&self) -> &'static str;
    fn args(&self, config: &TaskConfig) -> Vec<String>;
}

struct ClaudeExecutor;
impl ProviderExecutor for ClaudeExecutor {
    fn name(&self) -> &'static str {
        "claude"
    }
    fn executable(&self) -> &'static str {
        "claude"
    }
    fn credential_env(&self, config: &TaskConfig) -> String {
        config
            .api_key_env
            .clone()
            .unwrap_or_else(|| "ANTHROPIC_API_KEY".into())
    }
    fn credential_file(&self) -> &'static str {
        "anthropic_api_key"
    }
    fn args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec![
            "--print".into(),
            "--dangerously-skip-permissions".into(),
            "--output-format".into(),
            "stream-json".into(),
        ];
        if !config.session_id.is_empty() {
            args.extend(["--session-id".into(), config.session_id.clone()]);
        }
        if let Some(model) = &config.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(mcp) = &config.mcp_config {
            args.extend(["--mcp-config".into(), mcp.clone()]);
        }
        if !config.allowed_tools.is_empty() {
            args.extend(["--allowedTools".into(), config.allowed_tools.join(",")]);
        }
        args.push(config.prompt.clone());
        args
    }
}

struct DshExecutor;
impl ProviderExecutor for DshExecutor {
    fn name(&self) -> &'static str {
        "dsh"
    }
    fn executable(&self) -> &'static str {
        "dsh"
    }
    fn credential_env(&self, config: &TaskConfig) -> String {
        config
            .api_key_env
            .clone()
            .unwrap_or_else(|| "OPENROUTER_API_KEY".into())
    }
    fn credential_file(&self) -> &'static str {
        "openrouter_api_key"
    }
    fn args(&self, config: &TaskConfig) -> Vec<String> {
        vec!["--profile".into(), "headless".into(), config.prompt.clone()]
    }
}

fn registry() -> &'static HashMap<&'static str, Arc<dyn ProviderExecutor>> {
    static REGISTRY: OnceLock<HashMap<&'static str, Arc<dyn ProviderExecutor>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let providers: Vec<Arc<dyn ProviderExecutor>> =
            vec![Arc::new(ClaudeExecutor), Arc::new(DshExecutor)];
        providers.into_iter().map(|p| (p.name(), p)).collect()
    })
}

pub fn provider(name: &str) -> Result<Arc<dyn ProviderExecutor>, ProviderError> {
    registry()
        .get(name)
        .cloned()
        .ok_or_else(|| ProviderError::UnknownProvider(name.to_string()))
}

pub async fn run(
    config: &TaskConfig,
    output_tx: mpsc::Sender<OutputChunk>,
) -> Result<i32, ProviderError> {
    let executor = provider(&config.provider)?;
    if !Path::new(&config.working_dir).is_dir() {
        return Err(ProviderError::WorkingDirMissing(config.working_dir.clone()));
    }
    let credential = executor.credential_env(config);
    let leased_credential = resolve_credential(&credential, executor.credential_file());
    if leased_credential.is_none() {
        return Err(ProviderError::CredentialMissing(credential.clone()));
    }

    let args = executor.args(config);
    info!(task_id = %config.task_id, provider = executor.name(), argument_count = args.len(), "Running task provider");
    if unsafe_arg_logging_enabled() {
        debug!(task_id = %config.task_id, provider = executor.name(), ?args, "Provider arguments");
    }

    let mut command = Command::new(executor.executable());
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if std::env::var_os(&credential).is_none() {
        command.env(&credential, leased_credential.expect("credential resolved"));
    }
    crate::workload_identity::configure_command_in_dir(
        &mut command,
        Path::new(&config.working_dir),
    )?;
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ProviderError::NotFound(executor.executable().into())
        } else {
            ProviderError::SpawnFailed(e)
        }
    })?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let task_id = config.task_id.clone();
    let out_tx = output_tx.clone();
    let out = tokio::spawn(async move { forward_lines(stdout, "stdout", task_id, out_tx).await });
    let task_id = config.task_id.clone();
    let err =
        tokio::spawn(async move { forward_lines(stderr, "stderr", task_id, output_tx).await });
    let status = child.wait().await?;
    let _ = tokio::join!(out, err);
    status.code().ok_or_else(|| {
        error!(task_id = %config.task_id, "Provider killed by signal");
        ProviderError::Killed
    })
}

async fn forward_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stream: &str,
    task_id: String,
    tx: mpsc::Sender<OutputChunk>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let chunk = OutputChunk {
            stream: stream.into(),
            data: format!("{line}\n"),
            timestamp: now_ms(),
        };
        if tx.send(chunk).await.is_err() {
            warn!(%task_id, "Provider output receiver dropped");
            break;
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
fn unsafe_arg_logging_enabled() -> bool {
    matches!(
        std::env::var("AGENTIC_UNSAFE_LOG_PROVIDER_ARGS")
            .ok()
            .as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn resolve_credential(env_name: &str, file_name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(env_name) {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }
    let directory = std::env::var("AGENTIC_CREDENTIAL_DIR")
        .unwrap_or_else(|_| "/run/agentic-sandbox/credentials".into());
    std::fs::read_to_string(Path::new(&directory).join(file_name))
        .ok()
        .map(|v| v.trim_end_matches(['\r', '\n']).to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str) -> TaskConfig {
        TaskConfig {
            provider: provider.into(),
            task_id: "t1".into(),
            prompt: "fix tests".into(),
            working_dir: "/tmp".into(),
            session_id: String::new(),
            mcp_config: None,
            allowed_tools: vec![],
            model: None,
            api_key_env: None,
        }
    }

    #[test]
    fn absent_provider_defaults_to_claude() {
        let c: TaskConfig = serde_json::from_str(r#"{"prompt":"hello"}"#).unwrap();
        assert_eq!(c.provider, "claude");
    }

    #[test]
    fn registry_contains_claude_and_dsh() {
        assert_eq!(provider("claude").unwrap().executable(), "claude");
        assert_eq!(provider("dsh").unwrap().executable(), "dsh");
        assert!(matches!(
            provider("missing"),
            Err(ProviderError::UnknownProvider(_))
        ));
    }

    #[test]
    fn dsh_uses_headless_contract_and_openrouter_credential() {
        let c = config("dsh");
        let p = provider("dsh").unwrap();
        assert_eq!(p.args(&c), vec!["--profile", "headless", "fix tests"]);
        assert_eq!(p.credential_env(&c), "OPENROUTER_API_KEY");
        assert_eq!(p.credential_file(), "openrouter_api_key");
    }
}
