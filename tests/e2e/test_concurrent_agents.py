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


async def test_subscribe_filters_by_agent(
    management_server,
    ports,
    rust_agent: str,
    python_agent: str,
):
    """Subscribe to specific agent_id only delivers that agent's output.

    Verifies server-side filtering: a client subscribed to only one agent
    should NOT receive output from other agents.
    """
    # Client A: subscribes ONLY to rust_agent
    client_a = WSTestClient()
    await client_a.connect(f"ws://127.0.0.1:{ports.ws}")
    await client_a.subscribe(rust_agent)

    # Client B: subscribes to wildcard (dispatcher — can send commands)
    client_b = WSTestClient()
    await client_b.connect(f"ws://127.0.0.1:{ports.ws}")
    await client_b.subscribe("*")

    await asyncio.sleep(0.5)

    # Send commands to BOTH agents via client_b
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    rust_cmd_id = await client_b.send_command(rust_agent, "bash", [script])
    python_cmd_id = await client_b.send_command(python_agent, "bash", [script])

    # Collect output on both clients
    rust_output_b = await client_b.collect_output(rust_cmd_id, timeout=10)
    python_output_b = await client_b.collect_output(python_cmd_id, timeout=10)

    # Client B (wildcard) should have output from BOTH agents
    assert len(rust_output_b) > 0, "Client B missing Rust agent output"
    assert len(python_output_b) > 0, "Client B missing Python agent output"

    # Wait a bit for any straggler messages to arrive at client A
    await asyncio.sleep(2)

    # Client A should have output ONLY from rust_agent
    # Drain client A's inbox and check
    all_a_msgs = client_a.drain_inbox()
    output_msgs_a = [m for m in all_a_msgs if m.get("type") == "output"]

    # All output messages on client A should be from rust_agent only
    for m in output_msgs_a:
        assert m.get("agent_id") == rust_agent, (
            f"Client subscribed to {rust_agent} received output from "
            f"{m.get('agent_id')} — filtering is broken"
        )

    # Client A should have received the rust command output
    rust_output_a = [m for m in output_msgs_a if m.get("command_id") == rust_cmd_id]
    assert len(rust_output_a) > 0, "Client A missing subscribed agent's output"

    # Client A should NOT have any python agent output
    python_output_a = [m for m in output_msgs_a if m.get("agent_id") == python_agent]
    assert len(python_output_a) == 0, (
        f"Client A received {len(python_output_a)} messages from unsubscribed agent"
    )

    await client_a.close()
    await client_b.close()


async def test_unsubscribe_stops_output(
    management_server,
    ports,
    rust_agent: str,
):
    """Unsubscribe stops output delivery for that agent."""
    client = WSTestClient()
    await client.connect(f"ws://127.0.0.1:{ports.ws}")

    # Subscribe then unsubscribe
    await client.subscribe(rust_agent)
    ack = await client.unsubscribe(rust_agent)
    assert ack["type"] == "unsubscribed"
    assert ack["agent_id"] == rust_agent

    await asyncio.sleep(0.3)

    # Send a command — need a dispatcher client
    dispatcher = WSTestClient()
    await dispatcher.connect(f"ws://127.0.0.1:{ports.ws}")
    await dispatcher.subscribe("*")

    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    cmd_id = await dispatcher.send_command(rust_agent, "bash", [script])
    dispatcher_output = await dispatcher.collect_output(cmd_id, timeout=10)
    assert len(dispatcher_output) > 0, "Dispatcher should see output"

    # Wait for any messages to arrive at unsubscribed client
    await asyncio.sleep(2)
    remaining = client.drain_inbox()
    output_msgs = [m for m in remaining if m.get("type") == "output"]
    assert len(output_msgs) == 0, (
        f"Unsubscribed client received {len(output_msgs)} output messages"
    )

    await client.close()
    await dispatcher.close()
