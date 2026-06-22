use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::client::http::{ClientError, HttpClient};
use crate::config::{ContextEntry, ContextsFile};

const DEFAULT_GATEWAY_ADDR: &str = "127.0.0.1:8124";
const DEFAULT_SSH_PRINCIPAL: &str = "agent";
const DEFAULT_LEASE_TTL_SECONDS: i64 = 900;
const GATEWAY_ERROR_PREFIX: &str = "gateway ssh error:";
const FIRST_GATEWAY_READ_TIMEOUT: Duration = Duration::from_millis(75);

#[derive(Debug, Clone)]
pub struct SshOptions {
    pub instance_id: String,
    pub gateway: Option<String>,
    pub actor: Option<String>,
    pub user: String,
    pub identity: Option<PathBuf>,
    pub public_key: Option<PathBuf>,
    pub ttl_seconds: i64,
    pub no_lease: bool,
    pub ssh_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SshConfigOptions {
    pub instance_id: String,
    pub host: Option<String>,
    pub gateway: Option<String>,
    pub actor: Option<String>,
    pub user: String,
    pub identity: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshProxyOptions {
    pub instance_id: String,
    pub gateway: Option<String>,
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SshGatewayPrelude<'a> {
    actor: &'a str,
    instance_id: &'a str,
    access_mode: &'static str,
}

#[derive(Debug, Serialize)]
struct IssueSshLeaseRequest {
    actor: String,
    instance_id: String,
    principal: String,
    access_mode: String,
    public_key: String,
    ttl_seconds: i64,
}

#[derive(Debug, Deserialize)]
struct SshLeaseResponse {
    id: String,
    expires_at: String,
    #[serde(default)]
    certificate: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewaySshErrorKind {
    AuthzDenied,
    MissingInstance,
    GatewayUnavailable,
    ExpiredCredential,
    RuntimeUnavailable,
    InvalidRequest,
    Unknown,
}

impl GatewaySshErrorKind {
    fn label(self) -> &'static str {
        match self {
            GatewaySshErrorKind::AuthzDenied => "authz denied",
            GatewaySshErrorKind::MissingInstance => "missing instance",
            GatewaySshErrorKind::GatewayUnavailable => "gateway unavailable",
            GatewaySshErrorKind::ExpiredCredential => "expired credential",
            GatewaySshErrorKind::RuntimeUnavailable => "runtime unavailable",
            GatewaySshErrorKind::InvalidRequest => "invalid request",
            GatewaySshErrorKind::Unknown => "gateway ssh failure",
        }
    }
}

impl std::fmt::Display for GatewaySshErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct GatewaySshCliError {
    pub kind: GatewaySshErrorKind,
    pub message: String,
}

pub async fn open(
    c: &HttpClient,
    contexts: &ContextsFile,
    server_override: Option<&str>,
    opts: SshOptions,
) -> Result<()> {
    let gateway = resolve_gateway(opts.gateway.as_deref());
    let actor = resolve_actor(opts.actor.as_deref(), contexts);
    let binary = current_binary_name();
    let context_name = context_arg(contexts);
    let public_key = opts.public_key.clone().or_else(|| {
        opts.identity
            .as_ref()
            .map(|identity| PathBuf::from(format!("{}.pub", identity.display())))
            .filter(|path| path.exists())
            .or_else(default_public_key_path)
    });

    let certificate_file = if opts.no_lease {
        None
    } else if let Some(public_key) = public_key.as_ref() {
        match issue_lease(
            c,
            &actor,
            &opts.instance_id,
            &opts.user,
            public_key,
            opts.ttl_seconds,
        )
        .await
        {
            Ok(Some(certificate)) => Some(certificate),
            Ok(None) => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };

    let proxy_command = proxy_command(
        &binary,
        context_name.as_deref(),
        server_override,
        &opts.instance_id,
        &gateway,
        &actor,
    );
    let args = ssh_invocation_args(SshInvocation {
        instance_id: &opts.instance_id,
        user: &opts.user,
        proxy_command: &proxy_command,
        identity: opts.identity.as_deref(),
        certificate_file: certificate_file.as_deref(),
        extra_args: &opts.ssh_args,
    });
    let status = std::process::Command::new("ssh")
        .args(args)
        .status()
        .context("spawning ssh");
    remove_temp_file(certificate_file.as_deref());
    exit_status_result(status?)
}

pub fn print_config(
    contexts: &ContextsFile,
    server_override: Option<&str>,
    opts: SshConfigOptions,
) {
    let gateway = resolve_gateway(opts.gateway.as_deref());
    let actor = resolve_actor(opts.actor.as_deref(), contexts);
    let binary = current_binary_name();
    let context_name = context_arg(contexts);
    let host = opts.host.as_deref().unwrap_or(&opts.instance_id);
    let proxy = proxy_command(
        &binary,
        context_name.as_deref(),
        server_override,
        &opts.instance_id,
        &gateway,
        &actor,
    );

    print!(
        "{}",
        render_ssh_config(host, &opts.user, &proxy, opts.identity.as_deref())
    );
}

pub async fn proxy(contexts: &ContextsFile, opts: SshProxyOptions) -> Result<()> {
    let gateway = resolve_gateway(opts.gateway.as_deref());
    let actor = resolve_actor(opts.actor.as_deref(), contexts);
    let prelude = gateway_prelude(&actor, &opts.instance_id)?;
    let mut stream = TcpStream::connect(&gateway).await.map_err(|error| {
        anyhow!(GatewaySshCliError {
            kind: GatewaySshErrorKind::GatewayUnavailable,
            message: format!("failed to connect to {gateway}: {error}"),
        })
    })?;
    stream.write_all(prelude.as_bytes()).await?;

    let first = match tokio::time::timeout(FIRST_GATEWAY_READ_TIMEOUT, read_some(&mut stream)).await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => return Err(error),
        Err(_) => Vec::new(),
    };
    if let Some(error) = classify_gateway_bytes(&first) {
        return Err(anyhow!(error));
    }
    if !first.is_empty() {
        tokio::io::stdout().write_all(&first).await?;
    }

    let (mut gateway_read, mut gateway_write) = stream.into_split();
    let to_gateway = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        tokio::io::copy(&mut stdin, &mut gateway_write).await
    });
    let from_gateway = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        tokio::io::copy(&mut gateway_read, &mut stdout).await
    });

    let (sent, received) = tokio::try_join!(to_gateway, from_gateway)?;
    sent?;
    received?;
    Ok(())
}

async fn issue_lease(
    c: &HttpClient,
    actor: &str,
    instance_id: &str,
    principal: &str,
    public_key_path: &Path,
    ttl_seconds: i64,
) -> Result<Option<PathBuf>> {
    let public_key = std::fs::read_to_string(public_key_path)
        .with_context(|| format!("reading SSH public key {}", public_key_path.display()))?;
    let request = IssueSshLeaseRequest {
        actor: actor.to_string(),
        instance_id: instance_id.to_string(),
        principal: principal.to_string(),
        access_mode: "ssh".to_string(),
        public_key,
        ttl_seconds,
    };
    let response: SshLeaseResponse = c
        .post_json("/api/v2/gateway/ssh/leases", Some(&request))
        .await
        .map_err(map_lease_error)?;
    let Some(certificate) = response.certificate else {
        return Ok(None);
    };
    let path = std::env::temp_dir().join(format!("sandboxctl-{}-cert.pub", response.id));
    std::fs::write(&path, certificate).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    eprintln!(
        "issued SSH gateway credential {} expiring at {}",
        response.id, response.expires_at
    );
    Ok(Some(path))
}

fn map_lease_error(error: ClientError) -> anyhow::Error {
    let text = error.to_string();
    let kind = if matches!(error, ClientError::Auth { .. }) {
        GatewaySshErrorKind::AuthzDenied
    } else {
        classify_gateway_error(&text)
    };
    anyhow!(GatewaySshCliError {
        kind,
        message: text,
    })
}

async fn read_some(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);
    Ok(buf)
}

fn classify_gateway_bytes(bytes: &[u8]) -> Option<GatewaySshCliError> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let message = text.strip_prefix(GATEWAY_ERROR_PREFIX)?.trim().to_string();
    Some(GatewaySshCliError {
        kind: classify_gateway_error(&message),
        message,
    })
}

pub fn classify_gateway_error(message: &str) -> GatewaySshErrorKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("authorization denied")
        || lower.contains("auth required")
        || lower.contains("permission denied")
    {
        GatewaySshErrorKind::AuthzDenied
    } else if lower.contains("instance not found") || lower.contains("not found (404)") {
        GatewaySshErrorKind::MissingInstance
    } else if lower.contains("expired credential")
        || lower.contains("credential expired")
        || lower.contains("lease expired")
    {
        GatewaySshErrorKind::ExpiredCredential
    } else if lower.contains("runtime unreachable") || lower.contains("ssh handshake failed") {
        GatewaySshErrorKind::RuntimeUnavailable
    } else if lower.contains("invalid request") || lower.contains("unsupported access mode") {
        GatewaySshErrorKind::InvalidRequest
    } else {
        GatewaySshErrorKind::Unknown
    }
}

fn exit_status_result(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("ssh exited with {}", status))
    }
}

fn remove_temp_file(path: Option<&Path>) {
    if let Some(path) = path {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => eprintln!("warning: failed to remove {}: {}", path.display(), error),
        }
    }
}

fn default_public_key_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".ssh/id_ed25519.pub"),
        home.join(".ssh/id_ecdsa.pub"),
        home.join(".ssh/id_rsa.pub"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn current_binary_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| "sandboxctl".to_string())
}

fn context_arg(contexts: &ContextsFile) -> Option<String> {
    contexts
        .current_context
        .clone()
        .filter(|value| !value.is_empty())
}

fn resolve_gateway(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AGENTIC_GATEWAY_SSH_CONNECT").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GATEWAY_ADDR.to_string())
}

fn resolve_actor(value: Option<&str>, contexts: &ContextsFile) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AGENTIC_GATEWAY_SSH_ACTOR").ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            contexts
                .active()
                .map(|(_, context)| actor_from_context(context))
        })
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "operator".to_string())
}

fn actor_from_context(context: &ContextEntry) -> String {
    context.role.clone()
}

fn gateway_prelude(actor: &str, instance_id: &str) -> Result<String> {
    let prelude = SshGatewayPrelude {
        actor,
        instance_id,
        access_mode: "ssh",
    };
    Ok(format!("{}\n", serde_json::to_string(&prelude)?))
}

fn proxy_command(
    binary: &str,
    context_name: Option<&str>,
    server_override: Option<&str>,
    instance_id: &str,
    gateway: &str,
    actor: &str,
) -> String {
    let mut parts = vec![shell_quote(binary)];
    if let Some(context_name) = context_name {
        parts.push("--context".to_string());
        parts.push(shell_quote(context_name));
    }
    if let Some(server_override) = server_override {
        parts.push("--server".to_string());
        parts.push(shell_quote(server_override));
    }
    parts.extend([
        "ssh-proxy".to_string(),
        shell_quote(instance_id),
        "--gateway".to_string(),
        shell_quote(gateway),
        "--actor".to_string(),
        shell_quote(actor),
    ]);
    parts.join(" ")
}

fn render_ssh_config(
    host: &str,
    user: &str,
    proxy_command: &str,
    identity: Option<&Path>,
) -> String {
    let mut out = format!(
        "Host {host}\n  HostName {host}\n  User {user}\n  ProxyCommand {proxy_command}\n  ServerAliveInterval 30\n"
    );
    if let Some(identity) = identity {
        out.push_str(&format!("  IdentityFile {}\n", identity.display()));
    }
    out
}

struct SshInvocation<'a> {
    instance_id: &'a str,
    user: &'a str,
    proxy_command: &'a str,
    identity: Option<&'a Path>,
    certificate_file: Option<&'a Path>,
    extra_args: &'a [String],
}

fn ssh_invocation_args(invocation: SshInvocation<'_>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-o"),
        OsString::from(format!("ProxyCommand={}", invocation.proxy_command)),
        OsString::from("-l"),
        OsString::from(invocation.user),
    ];
    if let Some(identity) = invocation.identity {
        args.push(OsString::from("-i"));
        args.push(identity.as_os_str().to_os_string());
    }
    if let Some(certificate_file) = invocation.certificate_file {
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "CertificateFile={}",
            certificate_file.display()
        )));
    }
    args.push(OsString::from(invocation.instance_id));
    args.extend(invocation.extra_args.iter().map(OsString::from));
    args
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' | b':' | b'@' | b'='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn default_user() -> String {
    DEFAULT_SSH_PRINCIPAL.to_string()
}

pub fn default_ttl_seconds() -> i64 {
    DEFAULT_LEASE_TTL_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contexts() -> ContextsFile {
        let mut cfg = ContextsFile::default();
        cfg.set_context(
            "lab",
            "http://localhost:8122".to_string(),
            String::new(),
            "operator@example.test".to_string(),
        );
        cfg.use_context("lab").unwrap();
        cfg
    }

    #[test]
    fn renders_openssh_config_with_proxy_command() {
        let cfg = render_ssh_config(
            "agent-01",
            "agent",
            "sandboxctl ssh-proxy agent-01 --gateway 127.0.0.1:8124",
            Some(Path::new("/tmp/id_ed25519")),
        );
        assert!(cfg.contains("Host agent-01"));
        assert!(cfg.contains("User agent"));
        assert!(cfg.contains("ProxyCommand sandboxctl ssh-proxy agent-01"));
        assert!(cfg.contains("IdentityFile /tmp/id_ed25519"));
    }

    #[test]
    fn proxy_command_preserves_context_server_gateway_and_actor() {
        let command = proxy_command(
            "sandboxctl",
            Some("lab"),
            Some("http://localhost:8122"),
            "agent-01",
            "127.0.0.1:8124",
            "operator@example.test",
        );
        assert_eq!(
            command,
            "sandboxctl --context lab --server http://localhost:8122 ssh-proxy agent-01 --gateway 127.0.0.1:8124 --actor operator@example.test"
        );
    }

    #[test]
    fn gateway_prelude_is_newline_delimited_json() {
        let prelude = gateway_prelude("operator@example.test", "agent-01").unwrap();
        assert_eq!(
            prelude,
            r#"{"actor":"operator@example.test","instance_id":"agent-01","access_mode":"ssh"}"#
                .to_string()
                + "\n"
        );
    }

    #[test]
    fn classifies_required_cli_errors() {
        assert_eq!(
            classify_gateway_error("authorization denied"),
            GatewaySshErrorKind::AuthzDenied
        );
        assert_eq!(
            classify_gateway_error("instance not found: agent-01"),
            GatewaySshErrorKind::MissingInstance
        );
        assert_eq!(
            classify_gateway_error("expired credential for sshlease_1"),
            GatewaySshErrorKind::ExpiredCredential
        );
        assert_eq!(
            classify_gateway_error("runtime unreachable: connection refused"),
            GatewaySshErrorKind::RuntimeUnavailable
        );
    }

    #[test]
    fn classify_gateway_bytes_requires_gateway_prefix() {
        let error = classify_gateway_bytes(b"gateway ssh error: instance not found: agent-01\n")
            .expect("classified");
        assert_eq!(error.kind, GatewaySshErrorKind::MissingInstance);
        assert!(classify_gateway_bytes(b"SSH-2.0-runtime\r\n").is_none());
    }

    #[test]
    fn ssh_invocation_uses_proxy_command_and_certificate_file() {
        let args = ssh_invocation_args(SshInvocation {
            instance_id: "agent-01",
            user: "agent",
            proxy_command: "sandboxctl ssh-proxy agent-01",
            identity: Some(Path::new("/tmp/id_ed25519")),
            certificate_file: Some(Path::new("/tmp/id_ed25519-cert.pub")),
            extra_args: &["-N".to_string()],
        });
        let rendered: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert_eq!(rendered[0], "-o");
        assert_eq!(rendered[1], "ProxyCommand=sandboxctl ssh-proxy agent-01");
        assert!(rendered.contains(&std::borrow::Cow::Borrowed(
            "CertificateFile=/tmp/id_ed25519-cert.pub"
        )));
        assert_eq!(rendered.last().unwrap(), "-N");
    }

    #[test]
    fn resolves_actor_from_context_role() {
        let cfg = contexts();
        assert_eq!(resolve_actor(None, &cfg), "operator@example.test");
    }
}
