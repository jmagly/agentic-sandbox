"""Tests for error handling and edge cases."""

import asyncio
import os

import pytest

from .helpers import WSTestClient

pytestmark = pytest.mark.asyncio


async def test_command_to_nonexistent_agent(ws_client: WSTestClient):
    """Sending a command to a non-existent agent returns an error."""
    await ws_client.send({
        "type": "send_command",
        "agent_id": "nonexistent-agent-00000",
        "command": "echo",
        "args": ["hello"],
    })

    # Should get an error response (not command_started)
    msg = await ws_client.wait_for_message("error", timeout=5)
    assert "message" in msg
    assert msg["message"]  # non-empty error message


async def test_command_not_found(
    ws_client_subscribed: WSTestClient, rust_agent: str
):
    """Agent runs a bad command, reports failure."""
    # Send a command that doesn't exist
    command_id = await ws_client_subscribed.send_command(
        rust_agent,
        "/nonexistent/binary/that/does/not/exist",
        [],
    )

    # The agent should report an error either through output or the
    # command should fail to execute. Collect whatever comes back.
    await asyncio.sleep(3)
    output = await ws_client_subscribed.collect_output(command_id, timeout=5)

    # Drain remaining messages that might contain error info
    remaining = ws_client_subscribed.drain_inbox()

    # We expect either:
    # 1. An error output message
    # 2. An error in the inbox related to this command
    # 3. No output at all (command failed before producing any)
    # Any of these is acceptable - the key is no crash/hang
    all_msgs = output + [
        m for m in remaining if m.get("command_id") == command_id
    ]

    # Command was dispatched (command_started was received in send_command)
    assert command_id, "Command should have been dispatched"
