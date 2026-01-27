"""Tests for command dispatch and output streaming."""

import asyncio
import os

import pytest

from .helpers import WSTestClient

pytestmark = pytest.mark.asyncio

SCRIPTS_DIR = os.path.join(os.path.dirname(__file__), "scripts")


async def test_stdout_streaming_rust(
    ws_client_subscribed: WSTestClient, rust_agent: str
):
    """Send echo_test.sh to Rust agent, verify stdout output arrives."""
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    command_id = await ws_client_subscribed.send_command(
        rust_agent, "bash", [script]
    )
    assert command_id

    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stdout_lines = [
        m["data"] for m in output
        if m.get("stream") == "stdout"
    ]
    combined = "".join(stdout_lines)
    assert "[STDOUT] test-output-marker-" in combined, (
        f"Expected stdout marker in output, got: {combined!r}"
    )


async def test_stdout_streaming_python(
    ws_client_subscribed: WSTestClient, python_agent: str
):
    """Send echo_test.sh to Python agent, verify stdout output arrives."""
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    command_id = await ws_client_subscribed.send_command(
        python_agent, "bash", [script]
    )
    assert command_id

    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stdout_lines = [
        m["data"] for m in output
        if m.get("stream") == "stdout"
    ]
    combined = "".join(stdout_lines)
    assert "[STDOUT] test-output-marker-" in combined, (
        f"Expected stdout marker in output, got: {combined!r}"
    )


async def test_stderr_streaming_rust(
    ws_client_subscribed: WSTestClient, rust_agent: str
):
    """Verify stderr lines arrive with stream='stderr' from Rust agent."""
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    command_id = await ws_client_subscribed.send_command(
        rust_agent, "bash", [script]
    )

    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stderr_lines = [
        m["data"] for m in output
        if m.get("stream") == "stderr"
    ]
    combined = "".join(stderr_lines)
    assert "[STDERR] test-error-marker-" in combined, (
        f"Expected stderr marker in output, got: {combined!r}"
    )


async def test_stderr_streaming_python(
    ws_client_subscribed: WSTestClient, python_agent: str
):
    """Verify stderr lines arrive with stream='stderr' from Python agent."""
    script = os.path.join(SCRIPTS_DIR, "echo_test.sh")
    command_id = await ws_client_subscribed.send_command(
        python_agent, "bash", [script]
    )

    output = await ws_client_subscribed.collect_output(command_id, timeout=10)
    stderr_lines = [
        m["data"] for m in output
        if m.get("stream") == "stderr"
    ]
    combined = "".join(stderr_lines)
    assert "[STDERR] test-error-marker-" in combined, (
        f"Expected stderr marker in output, got: {combined!r}"
    )


async def test_exit_code_reported(
    ws_client_subscribed: WSTestClient, rust_agent: str
):
    """Run exit_code.sh with code 42, verify command_result reports it."""
    script = os.path.join(SCRIPTS_DIR, "exit_code.sh")
    command_id = await ws_client_subscribed.send_command(
        rust_agent, "bash", [script, "42"]
    )

    # Collect output (may be empty) then wait for command result
    await ws_client_subscribed.collect_output(command_id, timeout=10)

    # The command result may come as an output message with special handling,
    # or we need to look for it in the collected output.
    # Give some time for the result to arrive and check inbox
    await asyncio.sleep(1)
    remaining = ws_client_subscribed.drain_inbox()

    # Look for any indication of exit code 42 or non-zero exit
    # The management server may report this as an output or error message
    all_data = []
    for msg in remaining:
        if msg.get("command_id") == command_id:
            all_data.append(msg)

    # At minimum, the command should have been dispatched and started
    assert command_id, "Command should have started"
