mod e2e_support;

use std::{thread, time::Duration};

use e2e_support::{require_rust_e2e, ManagementServer};

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
    server.wait_for_agent_absent(&agent_id, Duration::from_secs(5))?;

    Ok(())
}
