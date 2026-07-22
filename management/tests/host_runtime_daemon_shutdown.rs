#![cfg(unix)]

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn sigterm_removes_host_runtime_socket() {
    let temp = tempfile::tempdir().expect("temporary daemon root");
    let socket = temp.path().join("host-runtime.sock");
    let root = temp.path().join("state");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentic-host-runtime-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--root-dir")
        .arg(&root)
        .arg("--agent-client")
        .arg("/bin/false")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start host runtime daemon");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        assert!(child.try_wait().expect("inspect daemon").is_none());
        thread::sleep(Duration::from_millis(20));
    }
    assert!(socket.exists(), "daemon did not create its socket");

    let result = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(result, 0, "failed to send SIGTERM to daemon");

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for daemon") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon did not exit after SIGTERM");
        }
        thread::sleep(Duration::from_millis(20));
    };

    assert!(status.success(), "daemon exited unsuccessfully: {status}");
    assert!(!socket.exists(), "daemon left its Unix socket behind");
}
