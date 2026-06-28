//! Ready-event startup profile executor.
//!
//! This component bridges durable startup policy to the existing dispatcher.
//! It does not carry credential material; it only obtains broker leases before
//! launching a managed session.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

use crate::credentials::{CredentialBroker, IssueCredentialLeaseRequest};
use crate::dispatch::{CommandDispatcher, SessionType};
use crate::proto::ExecOutput;
use crate::startup_profiles::{
    StartupCredentialRef, StartupCredentialTargetType, StartupProfile, StartupProfileStore,
    StartupReadinessProbe, StartupState,
};

const STARTUP_LEASE_TTL_SECONDS: i64 = 900;
const STARTUP_SETUP_TIMEOUT_GRACE_SECONDS: u64 = 30;

struct PreparedStartup {
    setup_env: HashMap<String, String>,
    provider_env: HashMap<String, String>,
    setup_script: String,
    provider_script: String,
}

#[derive(Clone)]
pub struct StartupExecutor {
    profiles: Arc<StartupProfileStore>,
    credentials: Arc<CredentialBroker>,
    dispatcher: Arc<CommandDispatcher>,
}

impl StartupExecutor {
    pub fn new(
        profiles: Arc<StartupProfileStore>,
        credentials: Arc<CredentialBroker>,
        dispatcher: Arc<CommandDispatcher>,
    ) -> Self {
        Self {
            profiles,
            credentials,
            dispatcher,
        }
    }

    pub async fn handle_agent_ready(&self, agent_id: &str, instance_id: &str, loadout: &str) {
        let profiles = self
            .profiles
            .matching_ready_profiles(agent_id, instance_id, loadout);

        for profile in profiles {
            if let Err(err) = self
                .launch_profile(profile, agent_id, instance_id, loadout)
                .await
            {
                warn!(
                    agent_id = %agent_id,
                    instance_id = %instance_id,
                    error = %err,
                    "startup profile launch failed"
                );
            }
        }
    }

    async fn launch_profile(
        &self,
        profile: StartupProfile,
        agent_id: &str,
        instance_id: &str,
        _loadout: &str,
    ) -> Result<(), String> {
        let session_name = startup_session_name(&profile);
        let session_id = uuid::Uuid::now_v7().to_string();

        self.profiles
            .update_status(
                &profile.id,
                StartupState::WaitingForCredentials,
                None,
                None,
                None,
            )
            .map_err(|err| err.to_string())?;

        let prepared =
            match self.prepare_startup(&profile, agent_id, instance_id, &session_id, &session_name)
            {
                Ok(prepared) => prepared,
                Err(err) => {
                    self.revoke_startup_session_leases(&profile.id, &session_id, "prepare_failed");
                    return Err(err);
                }
            };

        if !prepared.setup_script.is_empty() {
            let setup_timeout = startup_setup_timeout_seconds(&profile);
            let (setup_command_id, setup_output_rx) = self
                .dispatcher
                .dispatch(
                    agent_id,
                    "bash".to_string(),
                    vec!["-lc".to_string(), prepared.setup_script],
                    profile.session.workdir.clone(),
                    prepared.setup_env,
                    setup_timeout as u32,
                )
                .await
                .map_err(|err| err.to_string())?;
            if let Err(err) = await_command_success(setup_output_rx, setup_timeout).await {
                let reason = format!(
                    "startup setup/readiness failed for {} ({}): {}",
                    session_name, setup_command_id, err
                );
                self.profiles
                    .update_status(
                        &profile.id,
                        StartupState::Blocked,
                        Some(reason.clone()),
                        None,
                        Some(setup_command_id),
                    )
                    .map_err(|status_err| status_err.to_string())?;
                self.revoke_startup_session_leases(&profile.id, &session_id, "setup_failed");
                return Err(reason);
            }
        }

        self.profiles
            .update_status(&profile.id, StartupState::Launching, None, None, None)
            .map_err(|err| err.to_string())?;

        let (command_id, output_rx) = match self
            .dispatcher
            .create_session_with_env_and_id(
                agent_id,
                session_name.clone(),
                SessionType::Interactive,
                "bash".to_string(),
                vec!["-lc".to_string(), prepared.provider_script],
                Some(profile.session.workdir.clone()),
                prepared.provider_env,
                profile.session.cols as u32,
                profile.session.rows as u32,
                Some(session_id.clone()),
            )
            .await
        {
            Ok(session) => session,
            Err(err) => {
                let reason = format!("startup session launch failed for {session_name}: {err}");
                let _ = self.profiles.update_status(
                    &profile.id,
                    StartupState::Failed,
                    Some(reason.clone()),
                    None,
                    None,
                );
                self.revoke_startup_session_leases(&profile.id, &session_id, "launch_failed");
                return Err(reason);
            }
        };

        let session_id = self
            .dispatcher
            .session_id_for_command(&command_id)
            .unwrap_or(session_id);

        self.profiles
            .update_status(
                &profile.id,
                StartupState::Running,
                None,
                Some(session_id.clone()),
                Some(command_id.clone()),
            )
            .map_err(|err| err.to_string())?;

        info!(
            profile_id = %profile.id,
            agent_id = %agent_id,
            instance_id = %instance_id,
            session_id = %session_id,
            command_id = %command_id,
            "startup profile launched"
        );

        self.spawn_session_lease_reaper(profile.id.clone(), session_id, command_id, output_rx);

        Ok(())
    }

    fn prepare_startup(
        &self,
        profile: &StartupProfile,
        agent_id: &str,
        instance_id: &str,
        session_id: &str,
        session_name: &str,
    ) -> Result<PreparedStartup, String> {
        let credential_dir = format!("/run/agentic-sandbox/credentials/{session_id}");
        let mut setup_env = HashMap::new();
        setup_env.insert("AGENTIC_CREDENTIAL_DIR".to_string(), credential_dir.clone());
        setup_env.insert("AGENTIC_STARTUP_PROFILE_ID".to_string(), profile.id.clone());
        let mut provider_env = HashMap::new();
        provider_env.insert("AGENTIC_CREDENTIAL_DIR".to_string(), credential_dir.clone());
        provider_env.insert("AGENTIC_STARTUP_PROFILE_ID".to_string(), profile.id.clone());

        let mut setup_script =
            String::from("set -euo pipefail\numask 077\nmkdir -p \"$AGENTIC_CREDENTIAL_DIR\"\n");

        for (index, credential_ref) in profile.credential_refs.iter().enumerate() {
            match self.issue_and_materialize_ref(
                credential_ref,
                agent_id,
                instance_id,
                session_id,
                session_name,
            ) {
                Ok(value) => {
                    let value_env = format!("AGENTIC_LEASED_CREDENTIAL_{index}");
                    let file_name = credential_file_name(credential_ref)?;
                    let file_path = format!("$AGENTIC_CREDENTIAL_DIR/{file_name}");
                    let provider_file_path = format!("{credential_dir}/{file_name}");
                    setup_env.insert(value_env.clone(), value);

                    setup_script.push_str(&format!(
                        "printf '%s' \"${value_env}\" > \"{file_path}\"\nunset {value_env}\n"
                    ));
                    if credential_ref.target.target_type == StartupCredentialTargetType::Env {
                        let env_name = format!("{}_FILE", credential_ref.target.name);
                        setup_script.push_str(&format!("export {env_name}=\"{file_path}\"\n"));
                        provider_env.insert(env_name, provider_file_path);
                    }
                }
                Err(err) if credential_ref.required => {
                    let reason = format!(
                        "required credential {} unavailable for {}: {}",
                        credential_ref.id, session_name, err
                    );
                    self.profiles
                        .update_status(
                            &profile.id,
                            StartupState::Blocked,
                            Some(reason.clone()),
                            None,
                            None,
                        )
                        .map_err(|status_err| status_err.to_string())?;
                    return Err(reason);
                }
                Err(err) => {
                    warn!(
                        profile_id = %profile.id,
                        credential_id = %credential_ref.id,
                        error = %err,
                        "optional startup credential unavailable"
                    );
                }
            }
        }

        for probe in &profile.readiness_probes {
            setup_script.push_str(&readiness_probe_script_line(probe));
        }

        let setup_script =
            if profile.credential_refs.is_empty() && profile.readiness_probes.is_empty() {
                String::new()
            } else {
                setup_script
            };

        Ok(PreparedStartup {
            setup_env,
            provider_env,
            setup_script,
            provider_script: format!("exec {}\n", profile.session.command),
        })
    }

    fn issue_and_materialize_ref(
        &self,
        credential_ref: &StartupCredentialRef,
        agent_id: &str,
        instance_id: &str,
        session_id: &str,
        _session_name: &str,
    ) -> Result<String, String> {
        let lease = self
            .credentials
            .issue_lease(
                &credential_ref.id,
                IssueCredentialLeaseRequest {
                    agent_id: agent_id.to_string(),
                    instance_id: instance_id.to_string(),
                    session_id: session_id.to_string(),
                    provider: credential_ref.provider.clone(),
                    allowed_use: credential_ref.allowed_use.clone(),
                    ttl_seconds: STARTUP_LEASE_TTL_SECONDS,
                    proxy_policy: None,
                },
            )
            .map_err(|err| err.to_string())?;
        self.credentials
            .plaintext_for_active_lease(&lease.id, agent_id, instance_id, session_id)
            .map_err(|err| {
                let _ = self.credentials.revoke_lease(&lease.id);
                err.to_string()
            })
    }

    fn revoke_startup_session_leases(&self, profile_id: &str, session_id: &str, reason: &str) {
        match self.credentials.revoke_leases_for_session(session_id) {
            Ok(revoked) if !revoked.is_empty() => {
                info!(
                    profile_id = %profile_id,
                    session_id = %session_id,
                    reason = %reason,
                    revoked_leases = revoked.len(),
                    "revoked startup credential leases"
                );
            }
            Ok(_) => {}
            Err(err) => {
                warn!(
                    profile_id = %profile_id,
                    session_id = %session_id,
                    reason = %reason,
                    error = %err,
                    "failed to revoke startup credential leases"
                );
            }
        }
    }

    fn spawn_session_lease_reaper(
        &self,
        profile_id: String,
        session_id: String,
        command_id: String,
        mut output_rx: mpsc::Receiver<ExecOutput>,
    ) {
        let credentials = self.credentials.clone();
        tokio::spawn(async move {
            while let Some(output) = output_rx.recv().await {
                if !output.complete {
                    continue;
                }
                match credentials.revoke_leases_for_session(&session_id) {
                    Ok(revoked) if !revoked.is_empty() => {
                        info!(
                            profile_id = %profile_id,
                            session_id = %session_id,
                            command_id = %command_id,
                            exit_code = output.exit_code,
                            revoked_leases = revoked.len(),
                            "revoked startup credential leases after session completion"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            profile_id = %profile_id,
                            session_id = %session_id,
                            command_id = %command_id,
                            error = %err,
                            "failed to revoke startup credential leases after session completion"
                        );
                    }
                }
                break;
            }
        });
    }
}

fn startup_session_name(profile: &StartupProfile) -> String {
    profile
        .session
        .session_name
        .clone()
        .unwrap_or_else(|| format!("startup-{}", profile.id))
}

fn credential_file_name(credential_ref: &StartupCredentialRef) -> Result<String, String> {
    let raw = match credential_ref.target.target_type {
        StartupCredentialTargetType::Env => credential_ref
            .target
            .name
            .strip_suffix("_FILE")
            .unwrap_or(&credential_ref.target.name)
            .to_ascii_lowercase(),
        StartupCredentialTargetType::File => credential_ref.target.name.clone(),
    };
    let safe = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() || safe == "." || safe == ".." || safe.contains('/') {
        return Err(format!(
            "invalid credential target file name for {}",
            credential_ref.id
        ));
    }
    Ok(safe)
}

fn startup_setup_timeout_seconds(profile: &StartupProfile) -> u64 {
    profile
        .readiness_probes
        .iter()
        .map(|probe| probe.timeout_seconds)
        .sum::<u64>()
        .saturating_add(STARTUP_SETUP_TIMEOUT_GRACE_SECONDS)
}

fn readiness_probe_script_line(probe: &StartupReadinessProbe) -> String {
    format!(
        "timeout {} sh -lc {}\n",
        probe.timeout_seconds,
        shell_single_quote(&probe.command)
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

async fn await_command_success(
    mut output_rx: mpsc::Receiver<ExecOutput>,
    timeout_seconds: u64,
) -> Result<(), String> {
    let wait = async move {
        while let Some(output) = output_rx.recv().await {
            if output.complete {
                return if output.exit_code == 0 {
                    Ok(())
                } else {
                    Err(if output.error.is_empty() {
                        format!("exit code {}", output.exit_code)
                    } else {
                        output.error
                    })
                };
            }
        }
        Err("command output closed before completion".to_string())
    };

    timeout(Duration::from_secs(timeout_seconds), wait)
        .await
        .map_err(|_| format!("timed out after {timeout_seconds}s"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::credentials::{CredentialLeaseState, CredentialValueInput, UpsertCredentialRequest};
    use crate::proto::{
        management_message, AgentRegistration, CommandResult, ManagementMessage, SystemInfo,
    };
    use crate::registry::AgentRegistry;
    use crate::startup_profiles::{
        StartupCredentialRef, StartupCredentialTarget, StartupCredentialTargetType,
        StartupSessionBackend, StartupSessionClass, StartupSessionSpec, StartupTarget,
        StartupTrigger, UpsertStartupProfileRequest,
    };
    use tokio::sync::mpsc;
    use tokio::time::{sleep, timeout};

    fn test_registration(agent_id: &str, instance_id: &str, loadout: &str) -> AgentRegistration {
        AgentRegistration {
            agent_id: agent_id.to_string(),
            hostname: format!("{agent_id}.local"),
            ip_address: "127.0.0.1".to_string(),
            profile: "test".to_string(),
            labels: Default::default(),
            system: Some(SystemInfo {
                os: "Linux".to_string(),
                kernel: "6.1.0".to_string(),
                cpu_cores: 2,
                memory_bytes: 1024 * 1024 * 1024,
                disk_bytes: 10 * 1024 * 1024 * 1024,
            }),
            loadout: loadout.to_string(),
            aiwg_frameworks: vec![],
            instance_id: instance_id.to_string(),
        }
    }

    fn startup_request(credential_id: &str) -> UpsertStartupProfileRequest {
        UpsertStartupProfileRequest {
            id: Some("startup_codex".to_string()),
            description: None,
            trigger: StartupTrigger::OnInstanceReady,
            target: Some(StartupTarget {
                instance_id: None,
                agent_id: None,
                loadout: Some("automation-control".to_string()),
                runtime: None,
                provider: None,
            }),
            session: StartupSessionSpec {
                command: "agentic-codex-automation --profile startup_codex".to_string(),
                workdir: "/home/agent/workspace".to_string(),
                session_name: Some("codex-startup".to_string()),
                backend: StartupSessionBackend::Tmux,
                class: StartupSessionClass::Managed,
                cols: 120,
                rows: 40,
            },
            credential_refs: vec![StartupCredentialRef {
                id: credential_id.to_string(),
                provider: "codex".to_string(),
                allowed_use: "provider_api".to_string(),
                required: true,
                target: StartupCredentialTarget {
                    target_type: StartupCredentialTargetType::Env,
                    name: "OPENAI_API_KEY".to_string(),
                },
            }],
            readiness_probes: vec![],
            observation: Default::default(),
            control: Default::default(),
            restart: Default::default(),
        }
    }

    fn create_credential(broker: &CredentialBroker, id: &str) {
        broker
            .create(UpsertCredentialRequest {
                id: id.to_string(),
                provider: "codex".to_string(),
                credential_type: "api_key".to_string(),
                owner: None,
                scopes: vec![],
                allowed_uses: vec!["provider_api".to_string()],
                backend: None,
                value: Some(CredentialValueInput {
                    kind: "write_only".to_string(),
                    plaintext: Some("sk-test".to_string()),
                }),
            })
            .expect("credential should be created");
    }

    async fn wait_for_lease_state(
        broker: &CredentialBroker,
        lease_id: &str,
        state: CredentialLeaseState,
    ) {
        timeout(Duration::from_millis(500), async {
            loop {
                if broker.get_lease(lease_id).unwrap().state == state {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("lease should reach expected state");
    }

    async fn setup(
        credential_id: &str,
        create_secret: bool,
    ) -> (
        Arc<StartupProfileStore>,
        Arc<CredentialBroker>,
        StartupExecutor,
        Arc<CommandDispatcher>,
        mpsc::Receiver<ManagementMessage>,
    ) {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let profiles = Arc::new(StartupProfileStore::new_in_memory());
        let credentials = Arc::new(CredentialBroker::new_in_memory());

        if create_secret {
            create_credential(&credentials, credential_id);
        }

        profiles
            .create(startup_request(credential_id))
            .expect("startup profile should be created");

        let (tx, rx) = mpsc::channel::<ManagementMessage>(8);
        registry.register(
            test_registration(
                "agent-ready",
                "019f0000-0000-7000-8000-000000000123",
                "automation-control",
            ),
            tx,
        );

        let executor =
            StartupExecutor::new(profiles.clone(), credentials.clone(), dispatcher.clone());
        (profiles, credentials, executor, dispatcher, rx)
    }

    #[tokio::test]
    async fn ready_event_launches_profile_after_required_lease() {
        let (profiles, credentials, executor, dispatcher, mut rx) =
            setup("cred_openai_api", true).await;

        let launch = tokio::spawn(async move {
            executor
                .handle_agent_ready(
                    "agent-ready",
                    "019f0000-0000-7000-8000-000000000123",
                    "automation-control",
                )
                .await;
        });

        let setup_msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("setup command should be sent")
            .expect("setup command message should exist");
        let Some(management_message::Payload::Command(setup_cmd)) = setup_msg.payload else {
            panic!("expected setup command payload");
        };
        assert_eq!(setup_cmd.command, "bash");
        assert!(!setup_cmd.allocate_pty);
        assert_eq!(setup_cmd.working_dir, "/home/agent/workspace");
        assert_eq!(
            setup_cmd
                .env
                .get("AGENTIC_LEASED_CREDENTIAL_0")
                .map(String::as_str),
            Some("sk-test")
        );
        let setup_args = setup_cmd.args.join(" ");
        assert!(setup_args.contains("OPENAI_API_KEY_FILE"));
        assert!(setup_args.contains("AGENTIC_LEASED_CREDENTIAL_0"));
        assert!(!setup_args.contains("sk-test"));

        dispatcher.handle_result(CommandResult {
            command_id: setup_cmd.command_id.clone(),
            exit_code: 0,
            success: true,
            error: String::new(),
            duration_ms: 10,
        });

        let msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("provider command should be sent")
            .expect("provider command message should exist");
        let Some(management_message::Payload::Command(cmd)) = msg.payload else {
            panic!("expected command payload");
        };
        launch.await.expect("startup task should complete");

        assert_eq!(cmd.command, "tmux");
        assert!(cmd.allocate_pty);
        assert_eq!(cmd.working_dir, "/home/agent/workspace");
        assert!(cmd.args.iter().any(|arg| arg == "codex-startup"));
        let args_joined = cmd.args.join(" ");
        assert!(args_joined.contains("agentic-codex-automation --profile startup_codex"));
        assert!(
            !args_joined.contains("AGENTIC_LEASED_CREDENTIAL_0"),
            "transient secret env names should stay out of provider command args"
        );
        assert!(
            !args_joined.contains("sk-test"),
            "secret value must not be present in PTY command args"
        );
        assert_eq!(
            cmd.env
                .get("OPENAI_API_KEY_FILE")
                .map(|value| value.starts_with("/run/agentic-sandbox/credentials/")),
            Some(true)
        );
        assert!(!cmd.env.contains_key("AGENTIC_LEASED_CREDENTIAL_0"));
        assert!(cmd
            .env
            .get("AGENTIC_CREDENTIAL_DIR")
            .is_some_and(|dir| dir.starts_with("/run/agentic-sandbox/credentials/")));

        let profile = profiles.get("startup_codex").expect("profile should exist");
        assert_eq!(profile.status.state, StartupState::Running);
        assert_eq!(
            profile.status.command_id.as_deref(),
            Some(cmd.command_id.as_str())
        );
        let session_id = profile
            .status
            .session_id
            .as_deref()
            .expect("session id should be recorded");

        let leases = credentials.list_leases();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].credential_id, "cred_openai_api");
        assert_eq!(leases[0].agent_id, "agent-ready");
        assert_eq!(leases[0].session_id, session_id);
        assert_eq!(leases[0].state, CredentialLeaseState::Active);

        dispatcher.handle_result(CommandResult {
            command_id: cmd.command_id.clone(),
            exit_code: 0,
            success: true,
            error: String::new(),
            duration_ms: 20,
        });
        wait_for_lease_state(&credentials, &leases[0].id, CredentialLeaseState::Revoked).await;
    }

    #[tokio::test]
    async fn readiness_probe_failure_blocks_before_provider_launch() {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let profiles = Arc::new(StartupProfileStore::new_in_memory());
        let credentials = Arc::new(CredentialBroker::new_in_memory());
        create_credential(&credentials, "cred_openai_api");

        let mut request = startup_request("cred_openai_api");
        request.readiness_probes = vec![crate::startup_profiles::StartupReadinessProbe {
            kind: "command".to_string(),
            command: "agentic-provider-readiness codex".to_string(),
            timeout_seconds: 7,
        }];
        profiles.create(request).unwrap();

        let (tx, mut rx) = mpsc::channel::<ManagementMessage>(8);
        registry.register(
            test_registration(
                "agent-ready",
                "019f0000-0000-7000-8000-000000000123",
                "automation-control",
            ),
            tx,
        );

        let executor = StartupExecutor::new(profiles.clone(), credentials, dispatcher.clone());
        let launch = tokio::spawn(async move {
            executor
                .handle_agent_ready(
                    "agent-ready",
                    "019f0000-0000-7000-8000-000000000123",
                    "automation-control",
                )
                .await;
        });

        let setup_msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("setup command should be sent")
            .expect("setup command message should exist");
        let Some(management_message::Payload::Command(setup_cmd)) = setup_msg.payload else {
            panic!("expected setup command payload");
        };
        assert!(setup_cmd
            .args
            .join(" ")
            .contains("agentic-provider-readiness codex"));

        dispatcher.handle_result(CommandResult {
            command_id: setup_cmd.command_id.clone(),
            exit_code: 1,
            success: false,
            error: "auth failed".to_string(),
            duration_ms: 10,
        });

        launch.await.expect("startup task should complete");
        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());

        let profile = profiles.get("startup_codex").expect("profile should exist");
        assert_eq!(profile.status.state, StartupState::Blocked);
        assert!(profile
            .status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("auth failed")));
        assert_eq!(
            profile.status.command_id.as_deref(),
            Some(setup_cmd.command_id.as_str())
        );
    }

    #[tokio::test]
    async fn missing_required_credential_blocks_before_dispatch() {
        let (profiles, _credentials, executor, _dispatcher, mut rx) =
            setup("missing_credential", false).await;

        executor
            .handle_agent_ready(
                "agent-ready",
                "019f0000-0000-7000-8000-000000000123",
                "automation-control",
            )
            .await;

        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        let profile = profiles.get("startup_codex").expect("profile should exist");
        assert_eq!(profile.status.state, StartupState::Blocked);
        assert!(profile
            .status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("missing_credential")));
    }

    #[tokio::test]
    async fn running_profile_does_not_launch_again_on_reconnect() {
        let (_profiles, _credentials, executor, dispatcher, mut rx) =
            setup("cred_openai_api", true).await;

        let first_launch = tokio::spawn(async move {
            executor
                .handle_agent_ready(
                    "agent-ready",
                    "019f0000-0000-7000-8000-000000000123",
                    "automation-control",
                )
                .await;
        });
        let setup_msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("setup command should be sent")
            .expect("setup command message should exist");
        let Some(management_message::Payload::Command(setup_cmd)) = setup_msg.payload else {
            panic!("expected setup command payload");
        };
        dispatcher.handle_result(CommandResult {
            command_id: setup_cmd.command_id,
            exit_code: 0,
            success: true,
            error: String::new(),
            duration_ms: 10,
        });
        let first = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("provider command should be sent");
        assert!(first.is_some());
        first_launch.await.expect("startup task should complete");

        let executor =
            StartupExecutor::new(_profiles.clone(), _credentials.clone(), dispatcher.clone());
        executor
            .handle_agent_ready(
                "agent-ready",
                "019f0000-0000-7000-8000-000000000123",
                "automation-control",
            )
            .await;

        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
    }
}
