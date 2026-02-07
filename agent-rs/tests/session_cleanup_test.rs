use agent_client::*;
use nix::sys::signal;
use nix::unistd::Pid;
use std::process::Command as StdCommand;
use std::time::Duration;
use tokio::sync::mpsc;

/// Helper to create a test AgentConfig
fn create_test_config() -> AgentConfig {
    AgentConfig {
        agent_id: "test-agent".to_string(),
        agent_secret: "test-secret".to_string(),
        server_address: "localhost:8120".to_string(),
        heartbeat_interval: Duration::from_secs(5),
        reconnect_delay: Duration::from_secs(1),
        max_reconnect_delay: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn test_cleanup_sessions_empty() {
    // Test that cleanup_sessions works when there are no running sessions
    let config = create_test_config();
    let client = AgentClient::new(config);

    // Should not panic with empty running_commands
    client.cleanup_sessions().await;

    // Verify running_commands is still empty
    let running = client.running_commands.lock().await;
    assert_eq!(running.len(), 0);
}

#[tokio::test]
async fn test_cleanup_sessions_with_tracked_pids() {
    // Test that cleanup_sessions sends SIGTERM to tracked PIDs
    let config = create_test_config();
    let client = AgentClient::new(config);

    // Spawn a long-running sleep process that we can track
    let child = StdCommand::new("sleep")
        .arg("300")
        .spawn()
        .expect("Failed to spawn test process");

    let child_pid = Pid::from_raw(child.id() as i32);

    // Add it to running_commands
    let (stdin_tx, _stdin_rx) = mpsc::channel::<StdinData>(1);
    let (pty_tx, _pty_rx) = mpsc::channel::<PtyControlMsg>(1);

    {
        let mut running = client.running_commands.lock().await;
        running.insert("test-cmd-1".to_string(), RunningCommand {
            stdin_tx,
            pty_control_tx: Some(pty_tx),
            pid: Some(child_pid),
        });
    }

    // Verify it's in the map
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 1);
    }

    // Call cleanup
    client.cleanup_sessions().await;

    // Verify running_commands is cleared
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 0);
    }

    // Wait a bit for signal to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify process was killed (sending signal 0 checks if process exists)
    let result = signal::kill(child_pid, None);
    assert!(result.is_err(), "Process should have been killed");
}

#[tokio::test]
async fn test_cleanup_sessions_multiple_pids() {
    // Test cleanup with multiple tracked PIDs
    let config = create_test_config();
    let client = AgentClient::new(config);

    // Spawn multiple test processes
    let mut pids = Vec::new();
    for _ in 0..3 {
        let child = StdCommand::new("sleep")
            .arg("300")
            .spawn()
            .expect("Failed to spawn test process");
        pids.push(Pid::from_raw(child.id() as i32));
    }

    // Add them all to running_commands
    {
        let mut running = client.running_commands.lock().await;
        for (i, pid) in pids.iter().enumerate() {
            let (stdin_tx, _) = mpsc::channel::<StdinData>(1);
            let (pty_tx, _) = mpsc::channel::<PtyControlMsg>(1);
            running.insert(format!("test-cmd-{}", i), RunningCommand {
                stdin_tx,
                pty_control_tx: Some(pty_tx),
                pid: Some(*pid),
            });
        }
    }

    // Verify all are tracked
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 3);
    }

    // Call cleanup
    client.cleanup_sessions().await;

    // Verify all cleared
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 0);
    }

    // Wait for signals to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify all processes were killed
    for pid in pids {
        let result = signal::kill(pid, None);
        assert!(result.is_err(), "Process {} should have been killed", pid);
    }
}

#[tokio::test]
async fn test_cleanup_sessions_without_pids() {
    // Test cleanup with commands that don't have PIDs (non-PTY commands)
    let config = create_test_config();
    let client = AgentClient::new(config);

    // Add commands without PIDs
    {
        let mut running = client.running_commands.lock().await;
        for i in 0..3 {
            let (stdin_tx, _) = mpsc::channel::<StdinData>(1);
            running.insert(format!("test-cmd-{}", i), RunningCommand {
                stdin_tx,
                pty_control_tx: None,
                pid: None, // No PID
            });
        }
    }

    // Verify commands are tracked
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 3);
    }

    // Call cleanup (should not panic even without PIDs)
    client.cleanup_sessions().await;

    // Verify all cleared
    {
        let running = client.running_commands.lock().await;
        assert_eq!(running.len(), 0);
    }
}

#[tokio::test]
async fn test_cleanup_sessions_idempotent() {
    // Test that cleanup can be called multiple times safely
    let config = create_test_config();
    let client = AgentClient::new(config);

    // Call cleanup multiple times
    client.cleanup_sessions().await;
    client.cleanup_sessions().await;
    client.cleanup_sessions().await;

    // Should still be empty
    let running = client.running_commands.lock().await;
    assert_eq!(running.len(), 0);
}
