//! Gateway-mediated SSH byte-stream connector.
//!
//! This connector intentionally does not implement the SSH protocol and does
//! not inspect SSH payload bytes. A client sends one newline-delimited JSON
//! prelude naming the actor and instance, then the connector proxies the
//! remaining stream to the configured runtime SSH endpoint. `sandboxctl` can
//! hide this prelude behind an OpenSSH ProxyCommand in #532.

use crate::audit::{AuditEvent, AuditEventType, AuditLogger, AuditOutcome};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

const PRELUDE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PRELUDE_BYTES: usize = 4096;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SshGatewayConnectRequest {
    pub actor: String,
    pub instance_id: String,
    #[serde(default = "default_access_mode")]
    pub access_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshGatewayTarget {
    pub instance_id: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SshGatewayConnectError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("authorization denied")]
    AuthorizationDenied,
    #[error("instance not found: {0}")]
    InstanceNotFound(String),
    #[error("runtime unreachable: {0}")]
    RuntimeUnreachable(String),
    #[error("ssh handshake failed: {0}")]
    SshHandshakeFailed(String),
    #[error("ssh stream failed: {0}")]
    StreamFailed(String),
}

pub trait SshGatewayTargetResolver: Send + Sync {
    fn resolve(
        &self,
        request: &SshGatewayConnectRequest,
    ) -> Result<SshGatewayTarget, SshGatewayConnectError>;
}

pub trait SshGatewayAuthorizer: Send + Sync {
    fn authorize(
        &self,
        request: &SshGatewayConnectRequest,
        target: &SshGatewayTarget,
    ) -> Result<(), SshGatewayConnectError>;
}

#[derive(Default)]
pub struct AllowAllSshGatewayAuthorizer;

impl SshGatewayAuthorizer for AllowAllSshGatewayAuthorizer {
    fn authorize(
        &self,
        _request: &SshGatewayConnectRequest,
        _target: &SshGatewayTarget,
    ) -> Result<(), SshGatewayConnectError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct StaticSshGatewayAuthorizer {
    rules: Arc<HashSet<(String, String)>>,
}

impl StaticSshGatewayAuthorizer {
    pub fn new(rules: HashSet<(String, String)>) -> Self {
        Self {
            rules: Arc::new(rules),
        }
    }

    pub fn from_env() -> Result<Option<Self>> {
        let raw = match std::env::var("AGENTIC_GATEWAY_SSH_ALLOWLIST") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };
        let mut rules = HashSet::new();
        for entry in raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let (actor, instance_id) = entry
                .split_once('=')
                .ok_or_else(|| anyhow!("invalid AGENTIC_GATEWAY_SSH_ALLOWLIST entry: {entry}"))?;
            let actor = actor.trim();
            let instance_id = instance_id.trim();
            if actor.is_empty() || instance_id.is_empty() {
                return Err(anyhow!(
                    "invalid AGENTIC_GATEWAY_SSH_ALLOWLIST entry: actor and instance id are required"
                ));
            }
            rules.insert((actor.to_string(), instance_id.to_string()));
        }
        if rules.is_empty() {
            return Err(anyhow!(
                "AGENTIC_GATEWAY_SSH_ALLOWLIST did not contain any allow rules"
            ));
        }
        Ok(Some(Self::new(rules)))
    }

    fn allows(&self, actor: &str, instance_id: &str) -> bool {
        let actor = actor.trim();
        let instance_id = instance_id.trim();
        self.rules
            .contains(&(actor.to_string(), instance_id.to_string()))
            || self.rules.contains(&(actor.to_string(), "*".to_string()))
            || self
                .rules
                .contains(&("*".to_string(), instance_id.to_string()))
            || self.rules.contains(&("*".to_string(), "*".to_string()))
    }
}

impl SshGatewayAuthorizer for StaticSshGatewayAuthorizer {
    fn authorize(
        &self,
        request: &SshGatewayConnectRequest,
        _target: &SshGatewayTarget,
    ) -> Result<(), SshGatewayConnectError> {
        if self.allows(&request.actor, &request.instance_id) {
            Ok(())
        } else {
            Err(SshGatewayConnectError::AuthorizationDenied)
        }
    }
}

#[derive(Clone, Default)]
pub struct StaticSshGatewayTargetResolver {
    targets: Arc<HashMap<String, SshGatewayTarget>>,
}

impl StaticSshGatewayTargetResolver {
    pub fn new(targets: HashMap<String, SshGatewayTarget>) -> Self {
        Self {
            targets: Arc::new(targets),
        }
    }

    pub fn from_env() -> Result<Option<Self>> {
        let raw = match std::env::var("AGENTIC_GATEWAY_SSH_TARGETS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };
        let mut targets = HashMap::new();
        for entry in raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let (instance_id, endpoint) = entry
                .split_once('=')
                .ok_or_else(|| anyhow!("invalid AGENTIC_GATEWAY_SSH_TARGETS entry: {entry}"))?;
            let instance_id = instance_id.trim();
            if instance_id.is_empty() {
                return Err(anyhow!(
                    "invalid AGENTIC_GATEWAY_SSH_TARGETS entry: instance id is required"
                ));
            }
            let (host, port) = parse_host_port(endpoint)
                .with_context(|| format!("invalid SSH target endpoint for {instance_id}"))?;
            let target = SshGatewayTarget {
                instance_id: instance_id.to_string(),
                host,
                port,
            };
            targets.insert(target.instance_id.clone(), target);
        }
        if targets.is_empty() {
            return Err(anyhow!(
                "AGENTIC_GATEWAY_SSH_TARGETS did not contain any targets"
            ));
        }
        Ok(Some(Self::new(targets)))
    }
}

impl SshGatewayTargetResolver for StaticSshGatewayTargetResolver {
    fn resolve(
        &self,
        request: &SshGatewayConnectRequest,
    ) -> Result<SshGatewayTarget, SshGatewayConnectError> {
        self.targets
            .get(request.instance_id.trim())
            .cloned()
            .ok_or_else(|| SshGatewayConnectError::InstanceNotFound(request.instance_id.clone()))
    }
}

#[derive(Clone)]
pub struct SshGatewayConnector {
    resolver: Arc<dyn SshGatewayTargetResolver>,
    authorizer: Arc<dyn SshGatewayAuthorizer>,
    audit_logger: Option<Arc<AuditLogger>>,
}

impl SshGatewayConnector {
    pub fn new(resolver: Arc<dyn SshGatewayTargetResolver>) -> Self {
        Self {
            resolver,
            authorizer: Arc::new(AllowAllSshGatewayAuthorizer),
            audit_logger: None,
        }
    }

    pub fn with_authorizer(mut self, authorizer: Arc<dyn SshGatewayAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    pub fn with_audit_logger(mut self, audit_logger: Option<Arc<AuditLogger>>) -> Self {
        self.audit_logger = audit_logger;
        self
    }

    pub async fn serve(self, listen_addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(listen_addr)
            .await
            .with_context(|| format!("failed to bind gateway SSH listener on {listen_addr}"))?;
        tracing::info!(addr = %listen_addr, "gateway SSH connector listening");
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let connector = self.clone();
            tokio::spawn(async move {
                if let Err(error) = connector.handle_connection(stream, peer_addr).await {
                    tracing::warn!(peer_addr = %peer_addr, error = %error, "gateway SSH connection failed");
                }
            });
        }
    }

    pub async fn handle_connection(
        &self,
        stream: TcpStream,
        peer_addr: SocketAddr,
    ) -> Result<(), SshGatewayConnectError> {
        let mut reader = BufReader::new(stream);
        let request = match read_prelude(&mut reader).await {
            Ok(request) => request,
            Err(error) => {
                let _ = reader
                    .get_mut()
                    .write_all(format!("gateway ssh error: {error}\n").as_bytes())
                    .await;
                return Err(error);
            }
        };
        let audit_base = SshSessionAudit {
            actor: request.actor.clone(),
            instance_id: request.instance_id.clone(),
            peer_addr,
        };

        let result = self
            .proxy_after_prelude(reader, &request, &audit_base)
            .await;
        if let Err(error) = &result {
            self.audit_session(
                &audit_base,
                "gateway_ssh_session_failed",
                audit_outcome(error),
                json!({ "error": error.to_string() }),
            )
            .await;
        }
        result
    }

    async fn proxy_after_prelude(
        &self,
        mut reader: BufReader<TcpStream>,
        request: &SshGatewayConnectRequest,
        audit_base: &SshSessionAudit,
    ) -> Result<(), SshGatewayConnectError> {
        if let Err(error) = validate_request(request) {
            write_gateway_error(reader.get_mut(), &error).await;
            return Err(error);
        }
        let target = match self.resolver.resolve(request) {
            Ok(target) => target,
            Err(error) => {
                write_gateway_error(reader.get_mut(), &error).await;
                return Err(error);
            }
        };
        if let Err(error) = self.authorizer.authorize(request, &target) {
            write_gateway_error(reader.get_mut(), &error).await;
            return Err(error);
        }
        self.audit_session(
            audit_base,
            "gateway_ssh_session_started",
            AuditOutcome::Success,
            json!({
                "target_host": target.host,
                "target_port": target.port,
                "access_mode": request.access_mode,
            }),
        )
        .await;

        let mut upstream = match TcpStream::connect((target.host.as_str(), target.port)).await {
            Ok(upstream) => upstream,
            Err(error) => {
                let error = SshGatewayConnectError::RuntimeUnreachable(error.to_string());
                write_gateway_error(reader.get_mut(), &error).await;
                return Err(error);
            }
        };
        let buffered = reader.buffer().to_vec();
        if !buffered.is_empty() {
            upstream
                .write_all(&buffered)
                .await
                .map_err(|error| SshGatewayConnectError::StreamFailed(error.to_string()))?;
            reader.consume(buffered.len());
        }
        let (_client_to_runtime, runtime_to_client) =
            tokio::io::copy_bidirectional(reader.get_mut(), &mut upstream)
                .await
                .map_err(|error| SshGatewayConnectError::StreamFailed(error.to_string()))?;
        if runtime_to_client == 0 {
            let error = SshGatewayConnectError::SshHandshakeFailed(
                "runtime closed before sending an SSH banner".to_string(),
            );
            write_gateway_error(reader.get_mut(), &error).await;
            return Err(error);
        }
        self.audit_session(
            audit_base,
            "gateway_ssh_session_ended",
            AuditOutcome::Success,
            json!({
                "target_host": target.host,
                "target_port": target.port,
                "access_mode": request.access_mode,
            }),
        )
        .await;
        Ok(())
    }

    async fn audit_session(
        &self,
        audit_base: &SshSessionAudit,
        action: &'static str,
        outcome: AuditOutcome,
        details: serde_json::Value,
    ) {
        let Some(logger) = self.audit_logger.as_ref() else {
            return;
        };
        let details = json!({
            "instance_id": audit_base.instance_id,
            "peer_addr": audit_base.peer_addr.to_string(),
            "details": details,
        });
        let event = AuditEvent::new(
            AuditEventType::GatewaySshSession,
            audit_base.actor.clone(),
            audit_base.instance_id.clone(),
            action,
            outcome,
        )
        .with_details(details);
        if let Err(error) = logger.log(event).await {
            tracing::warn!(error = %error, action, "failed to append gateway SSH session audit event");
        }
    }
}

async fn write_gateway_error(stream: &mut TcpStream, error: &SshGatewayConnectError) {
    let _ = stream
        .write_all(format!("gateway ssh error: {error}\n").as_bytes())
        .await;
}

#[derive(Debug)]
struct SshSessionAudit {
    actor: String,
    instance_id: String,
    peer_addr: SocketAddr,
}

fn default_access_mode() -> String {
    "ssh".to_string()
}

async fn read_prelude(
    reader: &mut BufReader<TcpStream>,
) -> Result<SshGatewayConnectRequest, SshGatewayConnectError> {
    let mut line = Vec::new();
    loop {
        let available = timeout(PRELUDE_TIMEOUT, reader.fill_buf())
            .await
            .map_err(|_| SshGatewayConnectError::InvalidRequest("prelude timeout".to_string()))?
            .map_err(|error| SshGatewayConnectError::InvalidRequest(error.to_string()))?;
        if available.is_empty() {
            if line.is_empty() {
                return Err(SshGatewayConnectError::InvalidRequest(
                    "missing prelude".to_string(),
                ));
            }
            return Err(SshGatewayConnectError::InvalidRequest(
                "unterminated prelude".to_string(),
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map(|idx| idx + 1).unwrap_or(available.len());
        if line.len() + take > MAX_PRELUDE_BYTES {
            return Err(SshGatewayConnectError::InvalidRequest(
                "prelude too large".to_string(),
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    serde_json::from_slice(&line)
        .map_err(|error| SshGatewayConnectError::InvalidRequest(error.to_string()))
}

fn validate_request(request: &SshGatewayConnectRequest) -> Result<(), SshGatewayConnectError> {
    if request.actor.trim().is_empty() {
        return Err(SshGatewayConnectError::AuthorizationDenied);
    }
    if request.instance_id.trim().is_empty() {
        return Err(SshGatewayConnectError::InvalidRequest(
            "instance_id is required".to_string(),
        ));
    }
    if !request.access_mode.trim().eq_ignore_ascii_case("ssh") {
        return Err(SshGatewayConnectError::InvalidRequest(format!(
            "unsupported access mode: {}",
            request.access_mode
        )));
    }
    Ok(())
}

fn audit_outcome(error: &SshGatewayConnectError) -> AuditOutcome {
    match error {
        SshGatewayConnectError::AuthorizationDenied => AuditOutcome::Denied,
        _ => AuditOutcome::Failure,
    }
}

fn parse_host_port(endpoint: &str) -> Result<(String, u16)> {
    let endpoint = endpoint.trim();
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("endpoint must be host:port"))?;
    let port = port.parse::<u16>()?;
    if host.trim().is_empty() {
        return Err(anyhow!("endpoint host is required"));
    }
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditConfig, AuditQueryFilter};
    use chrono::Utc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn resolver_for(addr: SocketAddr) -> StaticSshGatewayTargetResolver {
        let mut targets = HashMap::new();
        targets.insert(
            "instance-1".to_string(),
            SshGatewayTarget {
                instance_id: "instance-1".to_string(),
                host: addr.ip().to_string(),
                port: addr.port(),
            },
        );
        StaticSshGatewayTargetResolver::new(targets)
    }

    async fn connect_through(connector: SshGatewayConnector) -> SshGatewayConnectError {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            connector.handle_connection(stream, peer).await.unwrap_err()
        });
        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(
                br#"{"actor":"operator@example.test","instance_id":"instance-1","access_mode":"ssh"}
SSH-2.0-test-client
"#,
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        connector_task.await.unwrap()
    }

    async fn connect_through_and_read_error(
        connector: SshGatewayConnector,
    ) -> (SshGatewayConnectError, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            connector.handle_connection(stream, peer).await.unwrap_err()
        });
        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(
                br#"{"actor":"operator@example.test","instance_id":"instance-1","access_mode":"ssh"}
SSH-2.0-test-client
"#,
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        (connector_task.await.unwrap(), response)
    }

    fn request_for(actor: &str, instance_id: &str) -> SshGatewayConnectRequest {
        SshGatewayConnectRequest {
            actor: actor.to_string(),
            instance_id: instance_id.to_string(),
            access_mode: "ssh".to_string(),
        }
    }

    #[tokio::test]
    async fn static_resolver_routes_known_instance() {
        let mut targets = HashMap::new();
        targets.insert(
            "instance-1".to_string(),
            SshGatewayTarget {
                instance_id: "instance-1".to_string(),
                host: "127.0.0.1".to_string(),
                port: 22,
            },
        );
        let resolver = StaticSshGatewayTargetResolver::new(targets);
        let target = resolver
            .resolve(&SshGatewayConnectRequest {
                actor: "operator@example.test".to_string(),
                instance_id: "instance-1".to_string(),
                access_mode: "ssh".to_string(),
            })
            .unwrap();
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 22);
    }

    #[test]
    fn static_authorizer_allows_exact_and_wildcard_rules() {
        let mut rules = HashSet::new();
        rules.insert((
            "operator@example.test".to_string(),
            "instance-1".to_string(),
        ));
        rules.insert(("admin@example.test".to_string(), "*".to_string()));
        rules.insert(("*".to_string(), "break-glass-1".to_string()));
        let authorizer = StaticSshGatewayAuthorizer::new(rules);
        let target = SshGatewayTarget {
            instance_id: "instance-1".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
        };

        assert!(authorizer
            .authorize(&request_for("operator@example.test", "instance-1"), &target)
            .is_ok());
        assert!(authorizer
            .authorize(&request_for("admin@example.test", "instance-2"), &target)
            .is_ok());
        assert!(authorizer
            .authorize(
                &request_for("anyone@example.test", "break-glass-1"),
                &target
            )
            .is_ok());
        assert_eq!(
            authorizer
                .authorize(&request_for("operator@example.test", "instance-2"), &target)
                .unwrap_err(),
            SshGatewayConnectError::AuthorizationDenied
        );
    }

    #[tokio::test]
    async fn connector_proxies_bytes_after_json_prelude() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut buf = [0u8; 8];
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"SSH-2.0");
            stream.write_all(b"SSH-2.0-runtime\r\n").await.unwrap();
        });

        let mut targets = HashMap::new();
        targets.insert(
            "instance-1".to_string(),
            SshGatewayTarget {
                instance_id: "instance-1".to_string(),
                host: upstream_addr.ip().to_string(),
                port: upstream_addr.port(),
            },
        );
        let connector =
            SshGatewayConnector::new(Arc::new(StaticSshGatewayTargetResolver::new(targets)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            connector.handle_connection(stream, peer).await.unwrap();
        });

        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(
                br#"{"actor":"operator@example.test","instance_id":"instance-1","access_mode":"ssh"}
SSH-2.0"#,
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"SSH-2.0-runtime\r\n");

        upstream_task.await.unwrap();
        connector_task.await.unwrap();
    }

    #[tokio::test]
    async fn connector_emits_start_and_end_audit_events() {
        const SSH_PAYLOAD_SECRET: &str = "GATEWAY_SSH_PAYLOAD_SECRET_SHOULD_NOT_BE_AUDITED";

        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            stream.write_all(b"SSH-2.0-runtime\r\n").await.unwrap();
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf).await.unwrap();
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let audit_logger = AuditLogger::new(AuditConfig {
            log_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        })
        .await
        .unwrap();
        let connector = SshGatewayConnector::new(Arc::new(resolver_for(upstream_addr)))
            .with_audit_logger(Some(Arc::new(audit_logger)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            connector.handle_connection(stream, peer).await.unwrap();
            connector.audit_logger.unwrap()
        });

        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "{{\"actor\":\"operator@example.test\",\"instance_id\":\"instance-1\",\"access_mode\":\"ssh\"}}\nSSH-2.0-test-client {SSH_PAYLOAD_SECRET}\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"SSH-2.0-runtime\r\n");

        let logger = connector_task.await.unwrap();
        upstream_task.await.unwrap();
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let events = logger
            .query(
                &date,
                Some(AuditQueryFilter {
                    event_type: Some(AuditEventType::GatewaySshSession),
                    actor: Some("operator@example.test".to_string()),
                    resource: Some("instance-1".to_string()),
                    outcome: None,
                    limit: None,
                }),
            )
            .await
            .unwrap();
        let actions: Vec<_> = events.iter().map(|event| event.action.as_str()).collect();
        assert_eq!(
            actions,
            vec!["gateway_ssh_session_started", "gateway_ssh_session_ended"]
        );
        assert!(events
            .iter()
            .all(|event| event.outcome == AuditOutcome::Success));
        assert!(events.iter().all(|event| {
            let line = serde_json::to_string(event).unwrap();
            !line.contains(SSH_PAYLOAD_SECRET)
                && !line.contains("SSH-2.0-test-client")
                && !line.contains("private_key")
                && !line.contains("certificate")
                && !line.contains("session_id")
                && !line.contains("command_id")
                && !line.contains("transcript")
                && !line.contains("replay")
        }));
    }

    #[tokio::test]
    async fn connector_distinguishes_instance_not_found() {
        let connector = SshGatewayConnector::new(Arc::new(StaticSshGatewayTargetResolver::new(
            HashMap::new(),
        )));
        let error = connect_through(connector).await;
        assert!(matches!(error, SshGatewayConnectError::InstanceNotFound(_)));
    }

    #[tokio::test]
    async fn connector_writes_post_prelude_gateway_errors_to_client() {
        let connector = SshGatewayConnector::new(Arc::new(StaticSshGatewayTargetResolver::new(
            HashMap::new(),
        )));
        let (error, response) = connect_through_and_read_error(connector).await;
        assert!(matches!(error, SshGatewayConnectError::InstanceNotFound(_)));
        assert!(response.contains("gateway ssh error: instance not found: instance-1"));
    }

    #[tokio::test]
    async fn connector_rejects_oversized_prelude_before_newline() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connector =
            SshGatewayConnector::new(Arc::new(resolver_for(upstream.local_addr().unwrap())));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            connector.handle_connection(stream, peer).await.unwrap_err()
        });

        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(&vec![b'a'; MAX_PRELUDE_BYTES + 1])
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        let error = connector_task.await.unwrap();
        assert_eq!(
            error,
            SshGatewayConnectError::InvalidRequest("prelude too large".to_string())
        );
        assert!(response.contains("gateway ssh error: invalid request: prelude too large"));
        assert!(
            timeout(Duration::from_millis(100), upstream.accept())
                .await
                .is_err(),
            "oversized prelude must fail before connecting to runtime SSH"
        );
    }

    #[tokio::test]
    async fn connector_distinguishes_runtime_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let connector = SshGatewayConnector::new(Arc::new(resolver_for(addr)));
        let error = connect_through(connector).await;
        assert!(matches!(
            error,
            SshGatewayConnectError::RuntimeUnreachable(_)
        ));
    }

    #[tokio::test]
    async fn connector_distinguishes_ssh_handshake_failure() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (_stream, _) = upstream.accept().await.unwrap();
        });
        let connector = SshGatewayConnector::new(Arc::new(resolver_for(upstream_addr)));
        let error = connect_through(connector).await;
        assert!(matches!(
            error,
            SshGatewayConnectError::SshHandshakeFailed(_)
        ));
        upstream_task.await.unwrap();
    }

    struct DenyAuthorizer;

    impl SshGatewayAuthorizer for DenyAuthorizer {
        fn authorize(
            &self,
            _request: &SshGatewayConnectRequest,
            _target: &SshGatewayTarget,
        ) -> Result<(), SshGatewayConnectError> {
            Err(SshGatewayConnectError::AuthorizationDenied)
        }
    }

    #[tokio::test]
    async fn connector_distinguishes_authorization_denied() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connector =
            SshGatewayConnector::new(Arc::new(resolver_for(upstream.local_addr().unwrap())))
                .with_authorizer(Arc::new(DenyAuthorizer));
        let error = connect_through(connector).await;
        assert_eq!(error, SshGatewayConnectError::AuthorizationDenied);
    }

    #[tokio::test]
    async fn connector_denied_policy_emits_denied_audit_without_touching_runtime_or_payload() {
        const SSH_PAYLOAD_SECRET: &str = "DENIED_GATEWAY_SSH_PAYLOAD_SECRET";

        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let audit_logger = AuditLogger::new(AuditConfig {
            log_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        })
        .await
        .unwrap();
        let connector = SshGatewayConnector::new(Arc::new(resolver_for(upstream_addr)))
            .with_authorizer(Arc::new(DenyAuthorizer))
            .with_audit_logger(Some(Arc::new(audit_logger)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let connector_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let error = connector.handle_connection(stream, peer).await.unwrap_err();
            (error, connector.audit_logger.unwrap())
        });

        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "{{\"actor\":\"operator@example.test\",\"instance_id\":\"instance-1\",\"access_mode\":\"ssh\"}}\nSSH-2.0-test-client {SSH_PAYLOAD_SECRET}\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("gateway ssh error: authorization denied"));

        let (error, logger) = connector_task.await.unwrap();
        assert_eq!(error, SshGatewayConnectError::AuthorizationDenied);
        assert!(
            timeout(Duration::from_millis(100), upstream.accept())
                .await
                .is_err(),
            "denied gateway SSH policy must not connect to the runtime SSH endpoint"
        );

        let date = Utc::now().format("%Y-%m-%d").to_string();
        let events = logger
            .query(
                &date,
                Some(AuditQueryFilter {
                    event_type: Some(AuditEventType::GatewaySshSession),
                    actor: Some("operator@example.test".to_string()),
                    resource: Some("instance-1".to_string()),
                    outcome: None,
                    limit: None,
                }),
            )
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "gateway_ssh_session_failed");
        assert_eq!(events[0].outcome, AuditOutcome::Denied);
        let audit_line = serde_json::to_string(&events[0]).unwrap();
        assert!(audit_line.contains("authorization denied"));
        assert!(!audit_line.contains(SSH_PAYLOAD_SECRET));
        assert!(!audit_line.contains("SSH-2.0-test-client"));
        assert!(!audit_line.contains("private_key"));
        assert!(!audit_line.contains("certificate"));
        assert!(!audit_line.contains("session_id"));
        assert!(!audit_line.contains("command_id"));
        assert!(!audit_line.contains("transcript"));
        assert!(!audit_line.contains("replay"));
    }

    #[test]
    fn parse_targets_from_env_format() {
        let (host, port) = parse_host_port("127.0.0.1:2222").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 2222);
    }
}
