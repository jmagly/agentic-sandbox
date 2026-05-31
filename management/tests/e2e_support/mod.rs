#![allow(dead_code)]

use std::{
    collections::VecDeque,
    env,
    io::{self, Read},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::TempDir;
use tokio::net::TcpStream as TokioTcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

static SERVER_START_LOCK: Mutex<()> = Mutex::new(());
const TEST_SECRET: &str = "e2e0000000000000000000000000000000000000000000000000000000000";

pub struct Ports {
    pub grpc: u16,
    pub ws: u16,
    pub http: u16,
}

pub struct ManagementServer {
    child: Child,
    _secrets_dir: TempDir,
    pub ports: Ports,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

pub struct AgentProcess {
    agent_id: String,
    child: Child,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

pub struct WsTestClient {
    ws: WebSocketStream<MaybeTlsStream<TokioTcpStream>>,
    inbox: VecDeque<Value>,
}

pub struct VmTestTarget {
    pub vm_name: String,
    pub ip: String,
    ssh_key: Option<PathBuf>,
}

pub struct VmManagementServer {
    child: Child,
    _secrets_dir: TempDir,
    ports: Ports,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
}

#[derive(Default)]
struct CapturedOutput {
    handle: Option<thread::JoinHandle<String>>,
}

impl CapturedOutput {
    fn capture<R>(mut reader: R) -> Self
    where
        R: Read + Send + 'static,
    {
        let handle = thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        });
        Self {
            handle: Some(handle),
        }
    }

    fn take(&mut self) -> String {
        self.handle
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    }
}

impl ManagementServer {
    pub fn start() -> anyhow::Result<Self> {
        let _start_guard = SERVER_START_LOCK
            .lock()
            .expect("server start lock poisoned");
        let ports = allocate_ports()?;
        let secrets_dir = tempfile::Builder::new()
            .prefix("rust-e2e-secrets-")
            .tempdir()?;
        let binary = management_binary();

        let mut child = Command::new(&binary)
            .env("LISTEN_ADDR", format!("127.0.0.1:{}", ports.grpc))
            .env("SECRETS_DIR", secrets_dir.path())
            .env("HEARTBEAT_TIMEOUT", "30")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to start {}: {err}", binary.display()))?;

        let stdout = child
            .stdout
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();
        let stderr = child
            .stderr
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();

        let mut server = Self {
            child,
            _secrets_dir: secrets_dir,
            ports,
            stdout,
            stderr,
        };
        server.wait_healthy(Duration::from_secs(15))?;
        Ok(server)
    }

    pub fn http_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.ports.http, path)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.ports.ws)
    }

    pub fn start_agent(&self, suffix: &str) -> anyhow::Result<AgentProcess> {
        let binary = agent_binary()?;
        let agent_id = format!(
            "rust-e2e-{}-{}",
            std::process::id(),
            suffix.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
        );

        let mut child = Command::new(&binary)
            .env("AGENT_ID", &agent_id)
            .env("AGENT_SECRET", TEST_SECRET)
            .env(
                "MANAGEMENT_SERVER",
                format!("127.0.0.1:{}", self.ports.grpc),
            )
            .env("HEARTBEAT_INTERVAL", "10")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to start {}: {err}", binary.display()))?;

        let stdout = child
            .stdout
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();
        let stderr = child
            .stderr
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();

        let process = AgentProcess {
            agent_id,
            child,
            stdout,
            stderr,
        };
        self.wait_for_agent(&process.agent_id, Duration::from_secs(30))?;
        Ok(process)
    }

    pub fn wait_for_agent_absent(&self, agent_id: &str, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if !self.agent_ids()?.iter().any(|seen| seen == agent_id) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!("agent {agent_id} was still registered after {timeout:?}")
    }

    fn wait_for_agent(&self, agent_id: &str, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            match self.agent_ids() {
                Ok(ids) if ids.iter().any(|seen| seen == agent_id) => return Ok(()),
                Ok(ids) => last_error = format!("registry had {:?}", ids),
                Err(err) => last_error = err.to_string(),
            }
            thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!("agent {agent_id} did not register within {timeout:?}; {last_error}")
    }

    pub fn agent_ids(&self) -> anyhow::Result<Vec<String>> {
        let value = http_get_json(self.ports.http, "/api/v1/agents")?;
        let agents = value
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("missing agents array in {value}"))?;

        Ok(agents
            .iter()
            .filter_map(|agent| agent.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect())
    }

    fn wait_healthy(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                anyhow::bail!(
                    "management exited during health check with {status}; stderr: {}",
                    self.stderr.take()
                );
            }

            match probe_http_ok(self.ports.http, "/api/v1/health") {
                Ok(true) => return Ok(()),
                Ok(false) => last_error = "non-200 health response".to_string(),
                Err(err) => last_error = err.to_string(),
            }

            thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!(
            "management did not become healthy within {:?}; last error: {last_error}",
            timeout
        )
    }
}

impl AgentProcess {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if self.child.try_wait()?.is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = self.stdout.take();
        let _ = self.stderr.take();
        Ok(())
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl Drop for ManagementServer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = self.stdout.take();
        let _ = self.stderr.take();
    }
}

impl VmTestTarget {
    pub fn from_env() -> anyhow::Result<Self> {
        let vm_name = env::var("TEST_VM")
            .map_err(|_| anyhow::anyhow!("TEST_VM must be set for VM-backed Rust E2E tests"))?;
        let ip = vm_ip(&vm_name)?;
        let ssh_key = vm_ssh_key(&vm_name);

        let target = Self {
            vm_name,
            ip,
            ssh_key,
        };
        if !target.is_alive() {
            anyhow::bail!("VM {} is not reachable over SSH", target.vm_name);
        }
        Ok(target)
    }

    pub fn ssh(&self, command: &str, timeout: Duration) -> anyhow::Result<SshOutput> {
        let mut args = Vec::new();
        if let Some(key) = &self.ssh_key {
            args.push("-i".to_string());
            args.push(key.display().to_string());
        }
        args.extend([
            "-o".to_string(),
            "ConnectTimeout=5".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "UserKnownHostsFile=/dev/null".to_string(),
            "-o".to_string(),
            "LogLevel=ERROR".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            format!("agent@{}", self.ip),
            command.to_string(),
        ]);

        let output = run_with_timeout(
            Command::new("sudo").arg("ssh").args(args),
            timeout,
            "ssh command",
        )?;

        Ok(SshOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    pub fn is_alive(&self) -> bool {
        self.ssh("echo alive", Duration::from_secs(10))
            .map(|output| output.stdout.contains("alive"))
            .unwrap_or(false)
    }

    pub fn agent_service(&self) -> anyhow::Result<String> {
        for service in ["agent-client", "agentic-agent"] {
            let output = self.ssh(
                &format!("systemctl is-active {service}"),
                Duration::from_secs(10),
            )?;
            if output.status == 0 && output.stdout.trim() == "active" {
                return Ok(service.to_string());
            }
        }

        anyhow::bail!("no active agent service found on {}", self.vm_name)
    }

    pub fn restart_agent_service(&self) -> anyhow::Result<String> {
        let service = self.agent_service()?;
        let output = self.ssh(
            &format!("sudo systemctl restart {service}"),
            Duration::from_secs(20),
        )?;
        if output.status != 0 {
            anyhow::bail!(
                "failed to restart {service}: stdout={} stderr={}",
                output.stdout,
                output.stderr
            );
        }
        Ok(service)
    }
}

impl VmManagementServer {
    pub fn start(vm: &VmTestTarget) -> anyhow::Result<Self> {
        let _start_guard = SERVER_START_LOCK
            .lock()
            .expect("server start lock poisoned");
        let ports = vm_e2e_ports()?;
        ensure_port_free(ports.grpc)?;
        ensure_port_free(ports.ws)?;
        ensure_port_free(ports.http)?;

        let secrets_dir = tempfile::Builder::new()
            .prefix("rust-vm-e2e-secrets-")
            .tempdir()?;
        let binary = management_binary();
        let listen_addr = format!("0.0.0.0:{}", ports.grpc);

        let mut child = Command::new(&binary)
            .env("LISTEN_ADDR", listen_addr)
            .env("SECRETS_DIR", secrets_dir.path())
            .env("HEARTBEAT_TIMEOUT", "30")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to start {}: {err}", binary.display()))?;

        let stdout = child
            .stdout
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();
        let stderr = child
            .stderr
            .take()
            .map(CapturedOutput::capture)
            .unwrap_or_default();

        let mut server = Self {
            child,
            _secrets_dir: secrets_dir,
            ports,
            stdout,
            stderr,
        };
        server.wait_healthy(Duration::from_secs(20))?;
        server.wait_for_vm_agent(vm, Duration::from_secs(60))?;
        Ok(server)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.ports.ws)
    }

    fn wait_healthy(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                anyhow::bail!(
                    "VM management exited during health check with {status}; stderr: {}",
                    self.stderr.take()
                );
            }

            match probe_http_ok(self.ports.http, "/api/v1/health") {
                Ok(true) => return Ok(()),
                Ok(false) => last_error = "non-200 health response".to_string(),
                Err(err) => last_error = err.to_string(),
            }

            thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!("VM management did not become healthy: {last_error}")
    }

    fn wait_for_vm_agent(&self, vm: &VmTestTarget, timeout: Duration) -> anyhow::Result<()> {
        let service = vm.restart_agent_service()?;
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            match http_get_json(self.ports.http, "/api/v1/agents") {
                Ok(value) => {
                    let ids = agent_ids_from_response(&value)?;
                    if ids.iter().any(|id| id == &vm.vm_name) {
                        return Ok(());
                    }
                    last_error = format!("registry had {ids:?}");
                }
                Err(err) => last_error = err.to_string(),
            }
            thread::sleep(Duration::from_secs(1));
        }

        anyhow::bail!(
            "{service} did not register as {} within {:?}; {last_error}",
            vm.vm_name,
            timeout
        )
    }
}

impl Drop for VmManagementServer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = self.stdout.take();
        let _ = self.stderr.take();
    }
}

impl WsTestClient {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let (ws, _) = connect_async(url).await?;
        Ok(Self {
            ws,
            inbox: VecDeque::new(),
        })
    }

    pub async fn send(&mut self, payload: Value) -> anyhow::Result<()> {
        use futures_util::SinkExt;

        self.ws.send(Message::Text(payload.to_string())).await?;
        Ok(())
    }

    pub async fn subscribe(&mut self, agent_id: &str) -> anyhow::Result<Value> {
        self.send(serde_json::json!({
            "type": "subscribe",
            "agent_id": agent_id,
        }))
        .await?;
        self.wait_for_type("subscribed", Duration::from_secs(20))
            .await
    }

    pub async fn unsubscribe(&mut self, agent_id: &str) -> anyhow::Result<Value> {
        self.send(serde_json::json!({
            "type": "unsubscribe",
            "agent_id": agent_id,
        }))
        .await?;
        self.wait_for_type("unsubscribed", Duration::from_secs(20))
            .await
    }

    pub async fn send_command(
        &mut self,
        agent_id: &str,
        command: &str,
        args: Vec<String>,
    ) -> anyhow::Result<String> {
        self.send(serde_json::json!({
            "type": "send_command",
            "agent_id": agent_id,
            "command": command,
            "args": args,
        }))
        .await?;

        let frame = self
            .wait_for_type("command_started", Duration::from_secs(20))
            .await?;
        let command_id = frame
            .get("command_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("command_started missing command_id: {frame}"))?;

        Ok(command_id.to_string())
    }

    pub async fn send_input(
        &mut self,
        agent_id: &str,
        command_id: &str,
        data: &str,
    ) -> anyhow::Result<Value> {
        self.send(serde_json::json!({
            "type": "send_input",
            "agent_id": agent_id,
            "command_id": command_id,
            "data": data,
        }))
        .await?;
        self.wait_for_type("input_sent", Duration::from_secs(5))
            .await
    }

    pub async fn wait_for_type(
        &mut self,
        expected_type: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.wait_for(timeout, |frame| {
            frame.get("type").and_then(Value::as_str) == Some(expected_type)
        })
        .await
    }

    pub async fn collect_output(
        &mut self,
        command_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut quiet_deadline = None;
        let mut output = Vec::new();

        loop {
            if let Some(index) = self.inbox.iter().position(|frame| {
                frame.get("type").and_then(Value::as_str) == Some("output")
                    && frame.get("command_id").and_then(Value::as_str) == Some(command_id)
            }) {
                output.push(self.inbox.remove(index).expect("indexed inbox item"));
                quiet_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                continue;
            }

            let now = tokio::time::Instant::now();
            if now >= deadline || quiet_deadline.is_some_and(|quiet| now >= quiet) {
                return Ok(output);
            }

            let next_timeout = quiet_deadline
                .unwrap_or(deadline)
                .saturating_duration_since(now);
            match tokio::time::timeout(
                next_timeout.min(Duration::from_millis(250)),
                self.next_json(),
            )
            .await
            {
                Ok(Ok(frame)) => {
                    if frame.get("type").and_then(Value::as_str) == Some("output")
                        && frame.get("command_id").and_then(Value::as_str) == Some(command_id)
                    {
                        output.push(frame);
                        quiet_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                    } else {
                        self.inbox.push_back(frame);
                    }
                }
                Ok(Err(err)) => return Err(err),
                Err(_) => {}
            }
        }
    }

    pub async fn drain_for(&mut self, duration: Duration) -> anyhow::Result<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut frames = self.inbox.drain(..).collect::<Vec<_>>();

        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(frames);
            }

            match tokio::time::timeout(
                (deadline - now).min(Duration::from_millis(250)),
                self.next_json(),
            )
            .await
            {
                Ok(Ok(frame)) => frames.push(frame),
                Ok(Err(err)) => return Err(err),
                Err(_) => {}
            }
        }
    }

    async fn wait_for<F>(&mut self, timeout: Duration, mut matches: F) -> anyhow::Result<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            if let Some(index) = self.inbox.iter().position(&mut matches) {
                return Ok(self.inbox.remove(index).expect("indexed inbox item"));
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                let seen = self
                    .inbox
                    .iter()
                    .filter_map(|frame| frame.get("type").and_then(Value::as_str))
                    .collect::<Vec<_>>();
                anyhow::bail!("timed out waiting for websocket frame; inbox types: {seen:?}");
            }

            let frame = tokio::time::timeout(deadline - now, self.next_json()).await??;
            if matches(&frame) {
                return Ok(frame);
            }
            self.inbox.push_back(frame);
        }
    }

    async fn next_json(&mut self) -> anyhow::Result<Value> {
        use futures_util::StreamExt;

        while let Some(frame) = self.ws.next().await {
            if let Message::Text(text) = frame? {
                return Ok(serde_json::from_str(&text)?);
            }
        }

        anyhow::bail!("websocket closed before next text frame")
    }
}

pub fn rust_e2e_enabled() -> bool {
    env::var("AGENTIC_RUN_RUST_E2E").as_deref() == Ok("1")
}

pub fn rust_vm_e2e_enabled() -> bool {
    env::var("AGENTIC_RUN_RUST_VM_E2E").as_deref() == Ok("1")
}

pub fn require_rust_e2e() -> bool {
    if rust_e2e_enabled() {
        true
    } else {
        eprintln!("skipping Rust E2E test; set AGENTIC_RUN_RUST_E2E=1 to run");
        false
    }
}

pub fn require_rust_vm_e2e() -> bool {
    if rust_vm_e2e_enabled() {
        true
    } else {
        eprintln!("skipping VM-backed Rust E2E test; set AGENTIC_RUN_RUST_VM_E2E=1 to run");
        false
    }
}

pub struct SshOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub async fn websocket_round_trip(
    url: &str,
    payload: Value,
    expected_type: &str,
) -> anyhow::Result<Value> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (mut ws, _) = connect_async(url).await?;
    ws.send(Message::Text(payload.to_string())).await?;

    while let Some(frame) = ws.next().await {
        let frame = frame?;
        if let Message::Text(text) = frame {
            let value: Value = serde_json::from_str(&text)?;
            if value.get("type").and_then(Value::as_str) == Some(expected_type) {
                return Ok(value);
            }
        }
    }

    anyhow::bail!("websocket closed before returning a {expected_type:?} frame")
}

fn management_binary() -> PathBuf {
    env::var_os("AGENTIC_MGMT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_agentic-mgmt")))
}

fn agent_binary() -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("AGENTIC_AGENT_BIN") {
        return Ok(PathBuf::from(path));
    }

    let candidate = PathBuf::from("../agent-rs/target/release/agent-client");
    if candidate.is_file() {
        Ok(candidate)
    } else {
        anyhow::bail!(
            "agent-client binary not found; set AGENTIC_AGENT_BIN or build agent-rs release binary"
        )
    }
}

fn allocate_ports() -> anyhow::Result<Ports> {
    for grpc in 18120..18420 {
        if [grpc, grpc + 1, grpc + 2].into_iter().all(port_is_free) {
            return Ok(Ports {
                grpc,
                ws: grpc + 1,
                http: grpc + 2,
            });
        }
    }

    anyhow::bail!("could not allocate three adjacent loopback ports")
}

fn port_is_free(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpListener::bind(addr).is_ok()
}

fn probe_http_ok(port: u16, path: &str) -> io::Result<bool> {
    let response = http_get_raw(port, path)?;
    Ok(response.starts_with("HTTP/1.1 200"))
}

fn http_get_json(port: u16, path: &str) -> anyhow::Result<Value> {
    let response = http_get_raw(port, path)?;
    let (_, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("HTTP response missing body separator: {response:?}"))?;
    Ok(serde_json::from_str(body.trim())?)
}

fn agent_ids_from_response(value: &Value) -> anyhow::Result<Vec<String>> {
    let agents = value
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("missing agents array in {value}"))?;

    Ok(agents
        .iter()
        .filter_map(|agent| agent.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect())
}

fn http_get_raw(port: u16, path: &str) -> io::Result<String> {
    use std::io::{Read as _, Write as _};

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn vm_ip(vm_name: &str) -> anyhow::Result<String> {
    let info_path = PathBuf::from("/var/lib/agentic-sandbox/vms")
        .join(vm_name)
        .join("vm-info.json");
    let output = if info_path.exists() {
        match std::fs::read_to_string(&info_path) {
            Ok(output) => output,
            Err(direct_err) => {
                let cat = Command::new("sudo")
                    .arg("cat")
                    .arg(&info_path)
                    .output()
                    .map_err(|sudo_err| {
                        anyhow::anyhow!(
                            "failed to read {} directly ({direct_err}) or with sudo: {sudo_err}",
                            info_path.display()
                        )
                    })?;
                if !cat.status.success() {
                    return vm_ip_from_virsh(vm_name);
                }
                String::from_utf8_lossy(&cat.stdout).into_owned()
            }
        }
    } else {
        return vm_ip_from_virsh(vm_name);
    };

    let value: Value = serde_json::from_str(&output)?;
    value
        .get("ip")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("missing ip in {}", info_path.display()))
}

fn vm_ip_from_virsh(vm_name: &str) -> anyhow::Result<String> {
    let output = Command::new("virsh")
        .arg("domifaddr")
        .arg(vm_name)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for part in stdout.split_whitespace() {
        if part.contains('/') && part.matches('.').count() == 3 {
            return Ok(part.split('/').next().unwrap_or(part).to_string());
        }
    }

    anyhow::bail!("could not determine IP for VM {vm_name}")
}

fn vm_ssh_key(vm_name: &str) -> Option<PathBuf> {
    let key_path = PathBuf::from("/var/lib/agentic-sandbox/secrets/ssh-keys").join(vm_name);
    if sudo_test_file(&key_path) {
        Some(key_path)
    } else {
        None
    }
}

fn sudo_test_file(path: &Path) -> bool {
    Command::new("sudo")
        .arg("test")
        .arg("-f")
        .arg(path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn vm_e2e_ports() -> anyhow::Result<Ports> {
    let grpc = env::var("E2E_MGMT_GRPC_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8120);
    Ok(Ports {
        grpc,
        ws: grpc + 1,
        http: grpc + 2,
    })
}

fn ensure_port_free(port: u16) -> anyhow::Result<()> {
    if port_is_free(port) {
        Ok(())
    } else {
        anyhow::bail!("required VM E2E port {port} is already in use")
    }
}

fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> anyhow::Result<std::process::Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + timeout;

    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{description} timed out after {timeout:?}");
        }
        thread::sleep(Duration::from_millis(100));
    }
}
