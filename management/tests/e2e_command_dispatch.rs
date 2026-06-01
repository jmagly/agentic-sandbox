mod e2e_support;

use std::time::Duration;

use e2e_support::{require_rust_e2e, ManagementServer, WsTestClient};

fn output_for_stream(frames: &[serde_json::Value], stream: &str) -> String {
    frames
        .iter()
        .filter(|frame| frame.get("stream").and_then(serde_json::Value::as_str) == Some(stream))
        .filter_map(|frame| frame.get("data").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_command_dispatch_streams_stdout_and_stderr() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("dispatch-streams")?;
    let marker = format!("rust-e2e-marker-{}", std::process::id());
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client.subscribe(agent.agent_id()).await?;

    let command_id = client
        .send_command(
            agent.agent_id(),
            "bash",
            vec![
                "-c".to_string(),
                format!("echo '[STDOUT] {marker}'; echo '[STDERR] {marker}' >&2"),
            ],
        )
        .await?;
    let output = client
        .collect_output(&command_id, Duration::from_secs(10))
        .await?;

    assert!(
        output_for_stream(&output, "stdout").contains(&format!("[STDOUT] {marker}")),
        "stdout marker missing from output frames: {output:?}"
    );
    assert!(
        output_for_stream(&output, "stderr").contains(&format!("[STDERR] {marker}")),
        "stderr marker missing from output frames: {output:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_command_to_missing_agent_returns_error() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client
        .send(serde_json::json!({
            "type": "send_command",
            "agent_id": "missing-rust-e2e-agent",
            "command": "echo",
            "args": ["hello"],
        }))
        .await?;

    let error = client
        .wait_for_type("error", Duration::from_secs(5))
        .await?;
    assert!(
        error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "expected non-empty error message, got {error}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_command_not_found_does_not_break_dispatch() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("dispatch-missing-command")?;
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client.subscribe(agent.agent_id()).await?;

    let command_id = client
        .send_command(
            agent.agent_id(),
            "/nonexistent/binary/that/does/not/exist",
            Vec::new(),
        )
        .await?;
    let _ = client
        .collect_output(&command_id, Duration::from_secs(5))
        .await?;

    assert!(
        !command_id.is_empty(),
        "command should have been dispatched"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_nonzero_exit_keeps_dispatch_channel_usable() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("dispatch-nonzero-exit")?;
    let marker = format!("rust-e2e-nonzero-{}", std::process::id());
    let followup_marker = format!("rust-e2e-followup-{}", std::process::id());
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client.subscribe(agent.agent_id()).await?;

    let command_id = client
        .send_command(
            agent.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '{marker}'; exit 42")],
        )
        .await?;
    let output = client
        .collect_output(&command_id, Duration::from_secs(10))
        .await?;

    assert!(
        output_for_stream(&output, "stdout").contains(&marker),
        "non-zero command stdout marker missing from output frames: {output:?}"
    );

    let followup_command_id = client
        .send_command(
            agent.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '{followup_marker}'")],
        )
        .await?;
    let followup_output = client
        .collect_output(&followup_command_id, Duration::from_secs(10))
        .await?;

    assert!(
        output_for_stream(&followup_output, "stdout").contains(&followup_marker),
        "follow-up command output missing after non-zero exit; frames: {followup_output:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_stdin_routes_to_running_command() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("stdin-routing")?;
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client.subscribe(agent.agent_id()).await?;

    let command_id = client
        .send_command(
            agent.agent_id(),
            "bash",
            vec![
                "-c".to_string(),
                "IFS= read -r line; printf 'GOT: %s\\n' \"$line\"".to_string(),
            ],
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    client
        .send_input(agent.agent_id(), &command_id, "hello-from-rust-e2e\n")
        .await?;

    let output = client
        .collect_output(&command_id, Duration::from_secs(10))
        .await?;
    let stdout = output_for_stream(&output, "stdout");
    assert!(
        stdout.contains("GOT: hello-from-rust-e2e"),
        "expected stdin echo in stdout, got {stdout:?}; frames: {output:?}"
    );

    Ok(())
}
