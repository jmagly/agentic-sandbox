#![allow(dead_code)]

use std::{
    collections::VecDeque,
    env,
    io::{self, Read},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
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

pub fn require_rust_e2e() -> bool {
    if rust_e2e_enabled() {
        true
    } else {
        eprintln!("skipping Rust E2E test; set AGENTIC_RUN_RUST_E2E=1 to run");
        false
    }
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
