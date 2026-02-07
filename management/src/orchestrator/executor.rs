//! Task executor - VM lifecycle and Claude execution
#![allow(dead_code)] // Fields reserved for future execution modes
//!
//! Handles VM provisioning, Claude Code execution, and cleanup.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::secrets::SecretResolver;
use super::storage::TaskStorage;
use super::task::Task;

/// VM information after provisioning
#[derive(Debug, Clone)]
pub struct VmInfo {
    pub name: String,
    pub ip: String,
    pub ssh_key_path: String,
}

/// Executes tasks by managing VM lifecycle and Claude execution
pub struct TaskExecutor {
    storage: Arc<TaskStorage>,
    secrets: Arc<SecretResolver>,
    agentshare_root: String,
    provision_script: String,
    destroy_script: String,
}

impl TaskExecutor {
    pub fn new(
        storage: Arc<TaskStorage>,
        secrets: Arc<SecretResolver>,
        agentshare_root: String,
    ) -> Self {
        // Find scripts relative to management server
        // These are in images/qemu/ from the project root
        let provision_script = std::env::var("PROVISION_SCRIPT")
            .unwrap_or_else(|_| "/opt/agentic-sandbox/images/qemu/provision-vm.sh".to_string());
        let destroy_script = std::env::var("DESTROY_SCRIPT")
            .unwrap_or_else(|_| "/opt/agentic-sandbox/scripts/destroy-vm.sh".to_string());

        Self {
            storage,
            secrets,
            agentshare_root,
            provision_script,
            destroy_script,
        }
    }

    /// Stage the task: clone repo, prepare inbox
    pub async fn stage_task(&self, task: &Arc<RwLock<Task>>) -> Result<(), ExecutorError> {
        let t = task.read().await;
        let task_id = t.id.clone();
        let repo_url = t.repository.url.clone();
        let branch = t.repository.branch.clone();
        let commit = t.repository.commit.clone();
        let prompt = t.claude.prompt.clone();
        drop(t);

        let inbox_path = self.storage.inbox_path(&task_id);
        info!("Staging task {} to {:?}", task_id, inbox_path);

        // Clone repository
        let git_args = vec![
            "clone".to_string(),
            "--depth".to_string(), "1".to_string(),
            "--branch".to_string(), branch.clone(),
            repo_url.clone(),
            inbox_path.to_string_lossy().to_string(),
        ];

        let output = Command::new("git")
            .args(&git_args)
            .output()
            .await
            .map_err(|e| ExecutorError::CommandFailed(format!("git clone: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExecutorError::CommandFailed(format!(
                "git clone failed: {}",
                stderr
            )));
        }

        // Checkout specific commit if provided
        if let Some(commit_sha) = commit {
            let output = Command::new("git")
                .args(["-C", &inbox_path.to_string_lossy(), "checkout", &commit_sha])
                .output()
                .await
                .map_err(|e| ExecutorError::CommandFailed(format!("git checkout: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("git checkout {} failed: {}", commit_sha, stderr);
            }
        }

        // Write TASK.md with prompt
        self.storage.write_task_prompt(&task_id, &prompt).await
            .map_err(|e| ExecutorError::StorageError(e.to_string()))?;

        info!("Task {} staged successfully", task_id);
        Ok(())
    }

    /// Provision VM for the task
    pub async fn provision_vm(&self, task: &Arc<RwLock<Task>>) -> Result<VmInfo, ExecutorError> {
        let t = task.read().await;
        let task_id = t.id.clone();
        let vm_config = t.vm.clone();
        drop(t);

        // Generate VM name from task ID (shorter, valid hostname)
        let vm_name = format!("task-{}", &task_id[..8]);

        info!("Provisioning VM {} for task {}", vm_name, task_id);

        // Build provision command
        let args = vec![
            "--profile".to_string(), vm_config.profile,
            "--cpus".to_string(), vm_config.cpus.to_string(),
            "--memory".to_string(), vm_config.memory,
            "--task-id".to_string(), task_id.clone(),
            "--start".to_string(),
            "--wait".to_string(),
            vm_name.clone(),
        ];

        let output = Command::new(&self.provision_script)
            .args(&args)
            .output()
            .await
            .map_err(|e| ExecutorError::CommandFailed(format!("provision-vm.sh: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExecutorError::ProvisionFailed(format!(
                "VM provisioning failed: {}",
                stderr
            )));
        }

        // Parse output to get IP and SSH key path
        // The script outputs vm-info.json which we can read
        let vm_info_path = format!(
            "/var/lib/agentic-sandbox/vms/{}/vm-info.json",
            vm_name
        );

        let vm_info_content = tokio::fs::read_to_string(&vm_info_path)
            .await
            .map_err(|e| ExecutorError::CommandFailed(format!("Failed to read vm-info.json: {}", e)))?;

        let vm_info_json: serde_json::Value = serde_json::from_str(&vm_info_content)
            .map_err(|e| ExecutorError::CommandFailed(format!("Failed to parse vm-info.json: {}", e)))?;

        let ip = vm_info_json["ip"]
            .as_str()
            .ok_or_else(|| ExecutorError::CommandFailed("No IP in vm-info.json".to_string()))?
            .to_string();

        let ssh_key_path = vm_info_json["management"]["ssh_key_path"]
            .as_str()
            .ok_or_else(|| ExecutorError::CommandFailed("No ssh_key_path in vm-info.json".to_string()))?
            .to_string();

        info!("VM {} provisioned with IP {}", vm_name, ip);

        Ok(VmInfo {
            name: vm_name,
            ip,
            ssh_key_path,
        })
    }

    /// Execute Claude Code in the VM
    pub async fn execute_claude(&self, task: &Arc<RwLock<Task>>) -> Result<i32, ExecutorError> {
        let t = task.read().await;
        let task_id = t.id.clone();
        let vm_ip = t.vm_ip.clone().ok_or(ExecutorError::VmNotReady)?;
        let claude_config = t.claude.clone();
        let secrets_config = t.secrets.clone();
        drop(t);

        info!("Executing Claude Code for task {}", task_id);

        // Resolve secrets
        let mut env_vars = HashMap::new();
        for secret_ref in &secrets_config {
            match self.secrets.resolve(&secret_ref.source, &secret_ref.key).await {
                Ok(value) => {
                    env_vars.insert(secret_ref.name.clone(), value);
                }
                Err(e) => {
                    warn!("Failed to resolve secret {}: {}", secret_ref.name, e);
                }
            }
        }

        // Build Claude command
        let mut claude_args = vec!["--print".to_string()];

        if claude_config.skip_permissions {
            claude_args.push("--dangerously-skip-permissions".to_string());
        }

        if claude_config.output_format == "stream-json" {
            claude_args.push("--output-format".to_string());
            claude_args.push("stream-json".to_string());
        }

        if let Some(max_turns) = claude_config.max_turns {
            claude_args.push("--max-turns".to_string());
            claude_args.push(max_turns.to_string());
        }

        // Add allowed tools
        for tool in &claude_config.allowed_tools {
            claude_args.push("--allowedTools".to_string());
            claude_args.push(tool.clone());
        }

        // The prompt comes from TASK.md in the inbox
        claude_args.push("--prompt".to_string());
        claude_args.push("Read TASK.md for your instructions".to_string());

        // Build SSH command
        let ssh_key_path = format!("/var/lib/agentic-sandbox/secrets/ssh-keys/task-{}", &task_id[..8]);

        // Build env export commands
        let env_exports: Vec<String> = env_vars.iter()
            .map(|(k, v)| format!("export {}='{}'", k, v.replace("'", "'\\''")))
            .collect();
        let env_cmd = env_exports.join(" && ");

        // Full command to run on VM
        let remote_cmd = format!(
            "cd ~/workspace && {} && claude {}",
            if env_cmd.is_empty() { "true".to_string() } else { env_cmd },
            claude_args.join(" ")
        );

        info!("SSH to {} to execute Claude", vm_ip);

        // Execute via SSH
        let mut ssh_command = Command::new("ssh");
        ssh_command
            .arg("-i")
            .arg(&ssh_key_path)
            .arg("-o").arg("StrictHostKeyChecking=no")
            .arg("-o").arg("UserKnownHostsFile=/dev/null")
            .arg("-o").arg("BatchMode=yes")
            .arg(format!("agent@{}", vm_ip))
            .arg(&remote_cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = ssh_command.spawn()
            .map_err(|e| ExecutorError::CommandFailed(format!("SSH spawn failed: {}", e)))?;

        // Stream output to storage
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let storage = self.storage.clone();
        let task_id_stdout = task_id.clone();
        let task_id_stderr = task_id.clone();

        // Spawn stdout reader
        let stdout_handle = if let Some(stdout) = stdout {
            let storage = storage.clone();
            Some(tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let data = format!("{}\n", line);
                    if let Err(e) = storage.append_stdout(&task_id_stdout, data.as_bytes()).await {
                        error!("Failed to write stdout: {}", e);
                    }
                }
            }))
        } else {
            None
        };

        // Spawn stderr reader
        let stderr_handle = if let Some(stderr) = stderr {
            let storage = storage.clone();
            Some(tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let data = format!("{}\n", line);
                    if let Err(e) = storage.append_stderr(&task_id_stderr, data.as_bytes()).await {
                        error!("Failed to write stderr: {}", e);
                    }
                }
            }))
        } else {
            None
        };

        // Wait for completion
        let status = child.wait().await
            .map_err(|e| ExecutorError::CommandFailed(format!("SSH wait failed: {}", e)))?;

        // Wait for output handlers
        if let Some(h) = stdout_handle { let _ = h.await; }
        if let Some(h) = stderr_handle { let _ = h.await; }

        let exit_code = status.code().unwrap_or(-1);
        info!("Claude execution completed with exit code {}", exit_code);

        Ok(exit_code)
    }

    /// Cleanup VM after task completion
    pub async fn cleanup_vm(&self, task: &Arc<RwLock<Task>>) -> Result<(), ExecutorError> {
        let t = task.read().await;
        let vm_name = match &t.vm_name {
            Some(name) => name.clone(),
            None => return Ok(()), // No VM to cleanup
        };
        drop(t);

        info!("Cleaning up VM {}", vm_name);

        // Run destroy script
        let output = Command::new(&self.destroy_script)
            .arg(&vm_name)
            .output()
            .await
            .map_err(|e| ExecutorError::CommandFailed(format!("destroy-vm.sh: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("VM cleanup had issues: {}", stderr);
        }

        info!("VM {} cleaned up", vm_name);
        Ok(())
    }
}

/// Executor errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("VM provisioning failed: {0}")]
    ProvisionFailed(String),

    #[error("VM not ready - no IP assigned")]
    VmNotReady,

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Secret resolution failed: {0}")]
    SecretError(String),
}
