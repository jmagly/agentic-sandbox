use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SANDBOXCTL: &str = env!("CARGO_BIN_EXE_sandboxctl");

#[derive(Debug, Deserialize)]
struct GatewayPrelude {
    actor: String,
    instance_id: String,
    access_mode: String,
}

#[test]
fn sandboxctl_ssh_and_generated_config_route_through_gateway_fixture() {
    let Some(sshd) = command_path("sshd") else {
        eprintln!("sshd unavailable; skipping routed SSH fixture");
        return;
    };
    if command_path("ssh-keygen").is_none() {
        eprintln!("ssh-keygen unavailable; skipping routed SSH fixture");
        return;
    }

    let fixture = SshdFixture::start(&sshd);
    let gateway = GatewayFixture::start(fixture.addr(), 2);

    let direct = run_with_timeout(
        sandboxctl_command(fixture.home_dir())
            .arg("ssh")
            .arg("--no-lease")
            .arg("--gateway")
            .arg(gateway.addr().to_string())
            .arg("--actor")
            .arg("operator@example.test")
            .arg("--ssh-option")
            .arg(format!(
                "UserKnownHostsFile={}",
                fixture.known_hosts().display()
            ))
            .arg("-i")
            .arg(fixture.client_key())
            .arg("-l")
            .arg(fixture.user())
            .arg("agent-01")
            .arg("echo")
            .arg("routed-ok"),
        Duration::from_secs(15),
    );
    assert_success("sandboxctl ssh", &direct);
    assert!(
        String::from_utf8_lossy(&direct.stdout).contains("routed-ok"),
        "stdout did not contain routed marker: {}",
        String::from_utf8_lossy(&direct.stdout)
    );

    let config_path = fixture.temp_dir().join("agent-01.ssh_config");
    let config = run_with_timeout(
        sandboxctl_command(fixture.home_dir())
            .arg("ssh-config")
            .arg("--gateway")
            .arg(gateway.addr().to_string())
            .arg("--actor")
            .arg("operator@example.test")
            .arg("-i")
            .arg(fixture.client_key())
            .arg("-l")
            .arg(fixture.user())
            .arg("agent-01"),
        Duration::from_secs(10),
    );
    assert_success("sandboxctl ssh-config", &config);
    std::fs::write(&config_path, &config.stdout).unwrap();

    let via_config = run_with_timeout(
        ssh_base_command()
            .arg("-F")
            .arg(&config_path)
            .arg("agent-01")
            .arg("echo")
            .arg("config-ok"),
        Duration::from_secs(15),
    );
    assert_success("ssh -F sandboxctl config", &via_config);
    assert!(
        String::from_utf8_lossy(&via_config.stdout).contains("config-ok"),
        "stdout did not contain config marker: {}",
        String::from_utf8_lossy(&via_config.stdout)
    );

    gateway.join();
}

struct SshdFixture {
    temp_dir: tempfile::TempDir,
    user: String,
    client_key: PathBuf,
    addr: SocketAddr,
    child: Child,
}

impl SshdFixture {
    fn start(sshd: &Path) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let user = current_user();
        let client_key = temp_dir.path().join("client_ed25519");
        let host_key = temp_dir.path().join("ssh_host_ed25519_key");
        run_command(
            Command::new("ssh-keygen")
                .arg("-q")
                .arg("-t")
                .arg("ed25519")
                .arg("-N")
                .arg("")
                .arg("-f")
                .arg(&client_key),
        );
        run_command(
            Command::new("ssh-keygen")
                .arg("-q")
                .arg("-t")
                .arg("ed25519")
                .arg("-N")
                .arg("")
                .arg("-f")
                .arg(&host_key),
        );
        let authorized_keys = temp_dir.path().join("authorized_keys");
        std::fs::copy(client_key.with_extension("pub"), &authorized_keys).unwrap();
        let home_dir = temp_dir.path().join("home");
        let ssh_dir = home_dir.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        let known_hosts = ssh_dir.join("known_hosts");
        let host_public_key = std::fs::read_to_string(host_key.with_extension("pub")).unwrap();
        let host_key_fields: Vec<_> = host_public_key.split_whitespace().collect();
        assert!(host_key_fields.len() >= 2);
        std::fs::write(
            known_hosts,
            format!("agent-01 {} {}\n", host_key_fields[0], host_key_fields[1]),
        )
        .unwrap();
        let port = free_port();
        let config_path = temp_dir.path().join("sshd_config");
        std::fs::write(
            &config_path,
            format!(
                "\
Port {port}
ListenAddress 127.0.0.1
HostKey {host_key}
PidFile {pid_file}
AuthorizedKeysFile {authorized_keys}
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
UsePAM no
StrictModes no
LogLevel VERBOSE
AllowUsers {user}
Subsystem sftp internal-sftp
",
                host_key = host_key.display(),
                pid_file = temp_dir.path().join("sshd.pid").display(),
                authorized_keys = authorized_keys.display(),
            ),
        )
        .unwrap();

        let child = Command::new(sshd)
            .arg("-D")
            .arg("-e")
            .arg("-f")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let fixture = Self {
            temp_dir,
            user,
            client_key,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            child,
        };
        fixture.wait_ready();
        fixture
    }

    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let output = run_with_timeout(
                ssh_base_command()
                    .arg("-i")
                    .arg(&self.client_key)
                    .arg("-p")
                    .arg(self.addr.port().to_string())
                    .arg(format!("{}@127.0.0.1", self.user))
                    .arg("true"),
                Duration::from_secs(3),
            );
            if output.status.success() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("sshd fixture did not become ready on {}", self.addr);
    }

    fn temp_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    fn home_dir(&self) -> PathBuf {
        self.temp_dir.path().join("home")
    }

    fn user(&self) -> &str {
        &self.user
    }

    fn client_key(&self) -> &Path {
        &self.client_key
    }

    fn known_hosts(&self) -> PathBuf {
        self.home_dir().join(".ssh").join("known_hosts")
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for SshdFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct GatewayFixture {
    addr: SocketAddr,
    handle: thread::JoinHandle<()>,
}

impl GatewayFixture {
    fn start(target_addr: SocketAddr, expected_connections: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut accepted = 0usize;
            while accepted < expected_connections && Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => {
                        accepted += 1;
                        handle_gateway_connection(stream, target_addr);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => panic!("gateway accept failed: {error}"),
                }
            }
            assert_eq!(accepted, expected_connections);
        });
        Self { addr, handle }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn join(self) {
        self.handle.join().unwrap();
    }
}

fn handle_gateway_connection(client: TcpStream, target_addr: SocketAddr) {
    let mut client_reader = BufReader::new(client.try_clone().unwrap());
    let mut prelude_line = String::new();
    client_reader.read_line(&mut prelude_line).unwrap();
    let prelude: GatewayPrelude = serde_json::from_str(&prelude_line).unwrap();
    assert_eq!(prelude.actor, "operator@example.test");
    assert_eq!(prelude.instance_id, "agent-01");
    assert_eq!(prelude.access_mode, "ssh");

    let mut upstream = TcpStream::connect(target_addr).unwrap();
    let mut buffered = client_reader.buffer().to_vec();
    if !buffered.is_empty() {
        upstream.write_all(&buffered).unwrap();
        buffered.clear();
    }

    let mut client_to_upstream = client_reader.into_inner();
    let mut upstream_to_client = upstream.try_clone().unwrap();
    let mut client_writer = client_to_upstream.try_clone().unwrap();
    let send = thread::spawn(move || {
        let result = std::io::copy(&mut client_to_upstream, &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
        result
    });
    let recv = thread::spawn(move || {
        let result = std::io::copy(&mut upstream_to_client, &mut client_writer);
        let _ = client_writer.shutdown(Shutdown::Write);
        result
    });
    let _ = send.join().unwrap();
    let _ = recv.join().unwrap();
}

fn ssh_base_command() -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("LogLevel=ERROR");
    command
}

fn sandboxctl_command(home: PathBuf) -> Command {
    let mut command = Command::new(SANDBOXCTL);
    command.env("HOME", home);
    command
}

fn run_command(command: &mut Command) {
    let output = command.output().unwrap();
    assert_success("command", &output);
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "command timed out after {:?}\nstdout:\n{}\nstderr:\n{}",
                timeout,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|user| !user.trim().is_empty())
        .unwrap_or_else(|| {
            let output = Command::new("id").arg("-un").output().unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        })
}

fn command_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
