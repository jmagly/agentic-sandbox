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

fn output_agent_ids(frames: &[serde_json::Value]) -> Vec<&str> {
    frames
        .iter()
        .filter(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some("output"))
        .filter_map(|frame| frame.get("agent_id").and_then(serde_json::Value::as_str))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_two_agents_route_commands_independently() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent_a = server.start_agent("concurrent-a")?;
    let agent_b = server.start_agent("concurrent-b")?;
    let mut client = WsTestClient::connect(&server.ws_url()).await?;
    client.subscribe("*").await?;

    let marker_a = format!("rust-e2e-agent-a-{}", std::process::id());
    let marker_b = format!("rust-e2e-agent-b-{}", std::process::id());
    let command_a = client
        .send_command(
            agent_a.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '[STDOUT] {marker_a}'")],
        )
        .await?;
    let command_b = client
        .send_command(
            agent_b.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '[STDOUT] {marker_b}'")],
        )
        .await?;

    let output_a = client
        .collect_output(&command_a, Duration::from_secs(10))
        .await?;
    let output_b = client
        .collect_output(&command_b, Duration::from_secs(10))
        .await?;

    assert!(
        output_for_stream(&output_a, "stdout").contains(&format!("[STDOUT] {marker_a}")),
        "agent A marker missing from output frames: {output_a:?}"
    );
    assert!(
        output_for_stream(&output_b, "stdout").contains(&format!("[STDOUT] {marker_b}")),
        "agent B marker missing from output frames: {output_b:?}"
    );
    assert!(
        output_agent_ids(&output_a)
            .iter()
            .all(|seen| *seen == agent_a.agent_id()),
        "agent A command output had unexpected agent IDs: {output_a:?}"
    );
    assert!(
        output_agent_ids(&output_b)
            .iter()
            .all(|seen| *seen == agent_b.agent_id()),
        "agent B command output had unexpected agent IDs: {output_b:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_subscribe_filters_by_agent() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent_a = server.start_agent("filter-a")?;
    let agent_b = server.start_agent("filter-b")?;
    let mut filtered = WsTestClient::connect(&server.ws_url()).await?;
    let mut dispatcher = WsTestClient::connect(&server.ws_url()).await?;
    filtered.subscribe(agent_a.agent_id()).await?;
    dispatcher.subscribe("*").await?;

    let marker_a = format!("rust-e2e-filter-a-{}", std::process::id());
    let marker_b = format!("rust-e2e-filter-b-{}", std::process::id());
    let command_a = dispatcher
        .send_command(
            agent_a.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '[STDOUT] {marker_a}'")],
        )
        .await?;
    let command_b = dispatcher
        .send_command(
            agent_b.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '[STDOUT] {marker_b}'")],
        )
        .await?;

    let dispatcher_a = dispatcher
        .collect_output(&command_a, Duration::from_secs(10))
        .await?;
    let dispatcher_b = dispatcher
        .collect_output(&command_b, Duration::from_secs(10))
        .await?;
    assert!(
        !dispatcher_a.is_empty() && !dispatcher_b.is_empty(),
        "dispatcher should see both agents; A={dispatcher_a:?}, B={dispatcher_b:?}"
    );

    let filtered_frames = filtered.drain_for(Duration::from_secs(2)).await?;
    let output_frames = filtered_frames
        .iter()
        .filter(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some("output"))
        .collect::<Vec<_>>();
    assert!(
        output_frames.iter().all(
            |frame| frame.get("agent_id").and_then(serde_json::Value::as_str)
                == Some(agent_a.agent_id())
        ),
        "filtered client received output for another agent: {output_frames:?}"
    );
    assert!(
        output_frames.iter().any(|frame| {
            frame.get("command_id").and_then(serde_json::Value::as_str) == Some(command_a.as_str())
                && frame
                    .get("data")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|data| data.contains(&marker_a))
        }),
        "filtered client missed subscribed agent output: {output_frames:?}"
    );
    assert!(
        output_frames.iter().all(|frame| {
            frame.get("command_id").and_then(serde_json::Value::as_str) != Some(command_b.as_str())
        }),
        "filtered client received unsubscribed command output: {output_frames:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_unsubscribe_stops_agent_output() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("unsubscribe")?;
    let mut unsubscribed = WsTestClient::connect(&server.ws_url()).await?;
    let mut dispatcher = WsTestClient::connect(&server.ws_url()).await?;
    unsubscribed.subscribe(agent.agent_id()).await?;
    let ack = unsubscribed.unsubscribe(agent.agent_id()).await?;
    assert_eq!(
        ack.get("agent_id").and_then(serde_json::Value::as_str),
        Some(agent.agent_id()),
        "unexpected unsubscribe ack: {ack}"
    );
    dispatcher.subscribe("*").await?;

    let marker = format!("rust-e2e-unsubscribed-{}", std::process::id());
    let command_id = dispatcher
        .send_command(
            agent.agent_id(),
            "bash",
            vec!["-c".to_string(), format!("echo '[STDOUT] {marker}'")],
        )
        .await?;
    let dispatcher_output = dispatcher
        .collect_output(&command_id, Duration::from_secs(10))
        .await?;
    assert!(
        output_for_stream(&dispatcher_output, "stdout").contains(&format!("[STDOUT] {marker}")),
        "dispatcher missed command output: {dispatcher_output:?}"
    );

    let unsubscribed_frames = unsubscribed.drain_for(Duration::from_secs(2)).await?;
    let output_frames = unsubscribed_frames
        .iter()
        .filter(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some("output"))
        .collect::<Vec<_>>();
    assert!(
        output_frames.is_empty(),
        "unsubscribed client received output frames: {output_frames:?}"
    );

    Ok(())
}
