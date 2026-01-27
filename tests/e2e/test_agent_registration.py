"""Tests for agent registration and deregistration."""

import asyncio

import pytest

from .helpers import WSTestClient


pytestmark = pytest.mark.asyncio


async def test_rust_agent_registers(ws_client: WSTestClient, rust_agent: str):
    """Start a Rust agent; list_agents should include it."""
    # Allow time for registration to propagate
    await asyncio.sleep(1)
    agents = await ws_client.list_agents()
    agent_ids = [a["id"] for a in agents]
    assert rust_agent in agent_ids, (
        f"Expected {rust_agent} in agent list, got {agent_ids}"
    )


async def test_python_agent_registers(ws_client: WSTestClient, python_agent: str):
    """Start a Python agent; list_agents should include it."""
    await asyncio.sleep(1)
    agents = await ws_client.list_agents()
    agent_ids = [a["id"] for a in agents]
    assert python_agent in agent_ids, (
        f"Expected {python_agent} in agent list, got {agent_ids}"
    )


async def test_agent_info_fields(ws_client: WSTestClient, rust_agent: str):
    """Verify agent info contains required fields."""
    await asyncio.sleep(1)
    agents = await ws_client.list_agents()
    agent = next((a for a in agents if a["id"] == rust_agent), None)
    assert agent is not None, f"Agent {rust_agent} not found"

    required_fields = ["id", "hostname", "status", "connected_at"]
    for field in required_fields:
        assert field in agent, f"Missing field '{field}' in agent info: {agent}"


async def test_agent_deregisters_on_disconnect(
    ws_client: WSTestClient, rust_agent_process
):
    """Stop an agent; list_agents should no longer show it."""
    agent_id, proc = rust_agent_process

    # Verify it's registered
    await asyncio.sleep(1)
    agents = await ws_client.list_agents()
    assert agent_id in [a["id"] for a in agents]

    # Stop the agent
    await proc.stop(timeout=5)

    # Wait for the server to notice the disconnection
    await asyncio.sleep(3)

    agents = await ws_client.list_agents()
    agent_ids = [a["id"] for a in agents]
    assert agent_id not in agent_ids, (
        f"Agent {agent_id} still in list after disconnect: {agent_ids}"
    )
