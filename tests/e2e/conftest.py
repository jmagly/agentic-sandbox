"""Shared pytest fixtures for E2E integration tests.

Starts the management server once per session, and provides per-test
WebSocket clients and agent processes.
"""

from __future__ import annotations

import os
import tempfile
import uuid

import pytest
import pytest_asyncio

from .helpers.port_alloc import PortAllocator, Ports
from .helpers.process_manager import ManagedProcess
from .helpers.ws_client import WSTestClient

# Resolve paths relative to the repo root
REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
MGMT_BINARY = os.path.join(REPO_ROOT, "management", "target", "release", "agentic-mgmt")
RUST_AGENT_BINARY = os.path.join(REPO_ROOT, "agent-rs", "target", "release", "agent-client")
PYTHON_AGENT_SCRIPT = os.path.join(REPO_ROOT, "agent", "grpc_client.py")
SCRIPTS_DIR = os.path.join(os.path.dirname(__file__), "scripts")

# Shared test secret (will auto-register on first connect)
TEST_SECRET = "e2e" + "0" * 61  # 64 chars


def _check_binary(path: str, label: str) -> None:
    if not os.path.isfile(path):
        pytest.skip(f"{label} not found at {path}  (run: cargo build --release)")


# ---------------------------------------------------------------------------
# Session-scoped fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def ports() -> Ports:
    """Allocate dynamic ports for this test session."""
    return PortAllocator.allocate()


@pytest.fixture(scope="session")
def secrets_dir():
    """Temporary secrets directory for the management server."""
    with tempfile.TemporaryDirectory(prefix="e2e-secrets-") as d:
        yield d


@pytest.fixture(scope="session")
def event_loop_policy():
    """Use the default event loop policy."""
    import asyncio
    return asyncio.DefaultEventLoopPolicy()


@pytest_asyncio.fixture(scope="session")
async def management_server(ports: Ports, secrets_dir: str):
    """Start the management server for the entire test session."""
    _check_binary(MGMT_BINARY, "Management server")

    proc = ManagedProcess(
        cmd=[MGMT_BINARY],
        env={
            "LISTEN_ADDR": f"127.0.0.1:{ports.grpc}",
            "SECRETS_DIR": secrets_dir,
            "HEARTBEAT_TIMEOUT": "30",
            "RUST_LOG": "info",
        },
        health_url=f"http://127.0.0.1:{ports.http}/api/v1/health",
        label="management-server",
    )
    await proc.start()
    await proc.wait_healthy(timeout=15)
    yield proc
    await proc.stop()


# ---------------------------------------------------------------------------
# Per-test fixtures
# ---------------------------------------------------------------------------

@pytest_asyncio.fixture
async def ws_client(management_server, ports: Ports):
    """Fresh WebSocket connection per test."""
    client = WSTestClient()
    await client.connect(f"ws://127.0.0.1:{ports.ws}")
    yield client
    await client.close()


@pytest_asyncio.fixture
async def ws_client_subscribed(ws_client: WSTestClient):
    """WebSocket client already subscribed to all agents."""
    await ws_client.subscribe("*")
    yield ws_client


@pytest_asyncio.fixture
async def rust_agent(management_server, ports: Ports):
    """Start a Rust agent, yield its agent_id, stop on teardown."""
    _check_binary(RUST_AGENT_BINARY, "Rust agent")

    agent_id = f"test-rust-{uuid.uuid4().hex[:8]}"
    proc = ManagedProcess(
        cmd=[RUST_AGENT_BINARY],
        env={
            "AGENT_ID": agent_id,
            "AGENT_SECRET": TEST_SECRET,
            "MANAGEMENT_SERVER": f"127.0.0.1:{ports.grpc}",
            "HEARTBEAT_INTERVAL": "10",
            "RUST_LOG": "info",
        },
        label=f"rust-agent-{agent_id}",
    )
    await proc.start()
    # Give the agent time to connect and register
    await proc.wait_healthy(timeout=10)
    yield agent_id
    await proc.stop()


@pytest_asyncio.fixture
async def python_agent(management_server, ports: Ports):
    """Start a Python agent, yield its agent_id, stop on teardown."""
    if not os.path.isfile(PYTHON_AGENT_SCRIPT):
        pytest.skip(f"Python agent not found at {PYTHON_AGENT_SCRIPT}")

    agent_id = f"test-python-{uuid.uuid4().hex[:8]}"
    proc = ManagedProcess(
        cmd=[
            "python3", PYTHON_AGENT_SCRIPT,
            "--server", f"127.0.0.1:{ports.grpc}",
            "--agent-id", agent_id,
            "--secret", TEST_SECRET,
            "--heartbeat", "10",
        ],
        env={
            "AGENT_ID": agent_id,
            "AGENT_SECRET": TEST_SECRET,
            "MANAGEMENT_SERVER": f"127.0.0.1:{ports.grpc}",
        },
        label=f"python-agent-{agent_id}",
    )
    await proc.start()
    await proc.wait_healthy(timeout=10)
    yield agent_id
    await proc.stop()


@pytest_asyncio.fixture
async def rust_agent_process(management_server, ports: Ports):
    """Start a Rust agent, yield (agent_id, ManagedProcess)."""
    _check_binary(RUST_AGENT_BINARY, "Rust agent")

    agent_id = f"test-rust-{uuid.uuid4().hex[:8]}"
    proc = ManagedProcess(
        cmd=[RUST_AGENT_BINARY],
        env={
            "AGENT_ID": agent_id,
            "AGENT_SECRET": TEST_SECRET,
            "MANAGEMENT_SERVER": f"127.0.0.1:{ports.grpc}",
            "HEARTBEAT_INTERVAL": "10",
            "RUST_LOG": "info",
        },
        label=f"rust-agent-{agent_id}",
    )
    await proc.start()
    await proc.wait_healthy(timeout=10)
    yield agent_id, proc
    await proc.stop()


@pytest_asyncio.fixture
async def python_agent_process(management_server, ports: Ports):
    """Start a Python agent, yield (agent_id, ManagedProcess)."""
    if not os.path.isfile(PYTHON_AGENT_SCRIPT):
        pytest.skip(f"Python agent not found at {PYTHON_AGENT_SCRIPT}")

    agent_id = f"test-python-{uuid.uuid4().hex[:8]}"
    proc = ManagedProcess(
        cmd=[
            "python3", PYTHON_AGENT_SCRIPT,
            "--server", f"127.0.0.1:{ports.grpc}",
            "--agent-id", agent_id,
            "--secret", TEST_SECRET,
            "--heartbeat", "10",
        ],
        env={
            "AGENT_ID": agent_id,
            "AGENT_SECRET": TEST_SECRET,
            "MANAGEMENT_SERVER": f"127.0.0.1:{ports.grpc}",
        },
        label=f"python-agent-{agent_id}",
    )
    await proc.start()
    await proc.wait_healthy(timeout=10)
    yield agent_id, proc
    await proc.stop()
