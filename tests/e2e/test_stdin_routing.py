"""Tests for stdin routing to running commands."""

import asyncio
import os

import pytest

from .helpers import WSTestClient

pytestmark = pytest.mark.asyncio

SCRIPTS_DIR = os.path.join(os.path.dirname(__file__), "scripts")


async def test_stdin_to_rust_agent(
    ws_client_subscribed: WSTestClient, rust_agent: str
):
    """Send stdin data to Rust agent, verify echo back via stdout."""
    script = os.path.join(SCRIPTS_DIR, "long_running.sh")
    command_id = await ws_client_subscribed.send_command(
        rust_agent, "bash", [script]
    )

    # Give the process a moment to start
    await asyncio.sleep(0.5)

    # Send input
    await ws_client_subscribed.send_input(
        rust_agent, command_id, "hello-from-test\n"
    )

    # Collect output and look for the echoed line
    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stdout_data = "".join(
        m["data"] for m in output if m.get("stream") == "stdout"
    )
    assert "GOT: hello-from-test" in stdout_data, (
        f"Expected echoed input in stdout, got: {stdout_data!r}"
    )


async def test_stdin_to_python_agent(
    ws_client_subscribed: WSTestClient, python_agent: str
):
    """Send stdin data to Python agent, verify echo back via stdout."""
    script = os.path.join(SCRIPTS_DIR, "long_running.sh")
    command_id = await ws_client_subscribed.send_command(
        python_agent, "bash", [script]
    )

    await asyncio.sleep(0.5)

    await ws_client_subscribed.send_input(
        python_agent, command_id, "hello-from-test\n"
    )

    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stdout_data = "".join(
        m["data"] for m in output if m.get("stream") == "stdout"
    )
    assert "GOT: hello-from-test" in stdout_data, (
        f"Expected echoed input in stdout, got: {stdout_data!r}"
    )
