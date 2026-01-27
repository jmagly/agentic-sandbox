"""Tests for concurrent agent operation."""

import asyncio
import os

import pytest

from .helpers import WSTestClient

pytestmark = pytest.mark.asyncio

SCRIPTS_DIR = os.path.join(os.path.dirname(__file__), "scripts")


async def test_two_agents_simultaneously(
    ws_client_subscribed: WSTestClient,
    rust_agent: str,
    python_agent: str,
):
    """Both Rust and Python agents connected; commands routed correctly."""
    await asyncio.sleep(1)

    # Verify both agents are registered
    agents = await ws_client_subscribed.list_agents()
    agent_ids = [a["id"] for a in agents]
    assert rust_agent in agent_ids, f"Rust agent not found: {agent_ids}"
    assert python_agent in agent_ids, f"Python agent not found: {agent_ids}"

    # Send command to each agent
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")

    rust_cmd_id = await ws_client_subscribed.send_command(
        rust_agent, "bash", [script]
    )
    python_cmd_id = await ws_client_subscribed.send_command(
        python_agent, "bash", [script]
    )

    # Collect output from both
    rust_output = await ws_client_subscribed.collect_output(rust_cmd_id, timeout=10)
    python_output = await ws_client_subscribed.collect_output(python_cmd_id, timeout=10)

    # Verify each got output
    rust_stdout = "".join(
        m["data"] for m in rust_output if m.get("stream") == "stdout"
    )
    python_stdout = "".join(
        m["data"] for m in python_output if m.get("stream") == "stdout"
    )

    assert "[STDOUT] test-output-marker-" in rust_stdout
    assert "[STDOUT] test-output-marker-" in python_stdout

    # Verify output was tagged with correct agent_id
    for m in rust_output:
        assert m.get("agent_id") == rust_agent
    for m in python_output:
        assert m.get("agent_id") == python_agent


async def test_subscribe_confirms_and_output_tagged(
    management_server,
    ports,
    rust_agent: str,
    python_agent: str,
):
    """Subscribe is acknowledged and output messages carry correct agent_id tags."""
    # Create two separate clients to dispatch and observe
    client = WSTestClient()
    await client.connect(f"ws://127.0.0.1:{ports.ws}")

    # Subscribe to a specific agent — server should acknowledge
    ack = await client.subscribe(rust_agent)
    assert ack["type"] == "subscribed"
    assert ack["agent_id"] == rust_agent

    # Also subscribe to wildcard
    ack_all = await client.subscribe("*")
    assert ack_all["type"] == "subscribed"
    assert ack_all["agent_id"] == "*"

    # Send commands to both agents and verify output is tagged correctly
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    rust_cmd_id = await client.send_command(rust_agent, "bash", [script])
    python_cmd_id = await client.send_command(python_agent, "bash", [script])

    rust_output = await client.collect_output(rust_cmd_id, timeout=10)
    python_output = await client.collect_output(python_cmd_id, timeout=10)

    # Each output message must be tagged with the correct agent_id
    for m in rust_output:
        assert m.get("agent_id") == rust_agent, (
            f"Rust command output tagged with wrong agent: {m.get('agent_id')}"
        )
    for m in python_output:
        assert m.get("agent_id") == python_agent, (
            f"Python command output tagged with wrong agent: {m.get('agent_id')}"
        )

    await client.close()
