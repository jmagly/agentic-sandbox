mod e2e_support;

use std::{thread, time::Duration};

use e2e_support::{require_rust_e2e, ManagementServer, WsTestClient};

#[test]
fn rust_e2e_agent_registers_and_deregisters() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let mut agent = server.start_agent("registration")?;
    let agent_id = agent.agent_id().to_string();

    let agent_ids = server.agent_ids()?;
    assert!(
        agent_ids.iter().any(|seen| seen == &agent_id),
        "expected {agent_id} in registry, got {agent_ids:?}"
    );

    agent.stop()?;
    thread::sleep(Duration::from_millis(500));
    if let Err(err) = server.wait_for_agent_absent(&agent_id, Duration::from_secs(30)) {
        let (stdout, stderr) = agent.take_output();
        anyhow::bail!("{err}; agent stdout: {stdout:?}; agent stderr: {stderr:?}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_e2e_websocket_agent_list_includes_required_fields() -> anyhow::Result<()> {
    if !require_rust_e2e() {
        return Ok(());
    }

    let server = ManagementServer::start()?;
    let agent = server.start_agent("agent-info-fields")?;
    let mut client = WsTestClient::connect(&server.ws_url()).await?;

    let agents = client.list_agents().await?;
    let agent_info = agents
        .iter()
        .find(|candidate| {
            candidate.get("id").and_then(serde_json::Value::as_str) == Some(agent.agent_id())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "agent {} missing from websocket agent list: {agents:?}",
                agent.agent_id()
            )
        })?;

    for field in [
        "id",
        "hostname",
        "ip_address",
        "status",
        "connected_at",
        "last_heartbeat",
    ] {
        assert!(
            agent_info.get(field).is_some(),
            "missing {field:?} in websocket agent info: {agent_info}"
        );
    }

    assert_eq!(
        agent_info.get("id").and_then(serde_json::Value::as_str),
        Some(agent.agent_id())
    );
    assert!(
        agent_info
            .get("hostname")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "hostname should be a non-empty string: {agent_info}"
    );
    assert!(
        agent_info
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "status should be a non-empty string: {agent_info}"
    );

    Ok(())
}
