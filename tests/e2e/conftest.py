"""Shared pytest fixtures for E2E integration tests.

Starts the management server once per session, and provides per-test
WebSocket clients and agent processes.
"""

from __future__ import annotations

import asyncio
import os
import tempfile
import uuid

import aiohttp
import pytest
import pytest_asyncio

from .helpers.port_alloc import PortAllocator, Ports
from .helpers.process_manager import ManagedProcess
from .helpers.ws_client import WSTestClient

# Resolve paths relative to the repo root
REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
MGMT_BINARY = os.path.join(REPO_ROOT, "management", "target", "release", "agentic-mgmt")
RUST_AGENT_BINARY = os.path.join(REPO_ROOT, "agent-rs", "target", "release", "agent-client")
SCRIPTS_DIR = os.path.join(os.path.dirname(__file__), "scripts")

# Use venv python if available, otherwise fall back to system python3
import sys
PYTHON_BIN = sys.executable

# Shared test secret (will auto-register on first connect)
TEST_SECRET = "e2e" + "0" * 61  # 64 chars


def _check_binary(path: str, label: str) -> None:
    if not os.path.isfile(path):
        pytest.skip(f"{label} not found at {path}  (run: cargo build --release)")


async def _wait_for_agent_registration(
    ports: Ports,
    agent_id: str,
    proc: ManagedProcess,
    management_proc: ManagedProcess,
    timeout: float = 30,
) -> None:
    """Wait until management shows the fixture agent as connected."""
    url = f"http://127.0.0.1:{ports.http}/api/v1/agents"
    deadline = asyncio.get_event_loop().time() + timeout
    last_seen: list[str] = []
    last_error = None

    async with aiohttp.ClientSession() as session:
        while asyncio.get_event_loop().time() < deadline:
            if not proc.is_running:
                stderr = await proc.read_stderr()
                mgmt_stderr = await management_proc.read_stderr()
                raise RuntimeError(
                    f"{proc.label} exited before registration; "
                    f"stderr: {stderr}; management stderr: {mgmt_stderr}"
                )

            try:
                async with session.get(url, timeout=aiohttp.ClientTimeout(total=3)) as resp:
                    resp.raise_for_status()
                    payload = await resp.json()
                last_seen = [agent["id"] for agent in payload.get("agents", [])]
                if agent_id in last_seen:
                    return
            except (aiohttp.ClientError, asyncio.TimeoutError) as exc:
                last_error = str(exc)

            await asyncio.sleep(0.2)

    stderr = await proc.read_stderr()
    mgmt_stdout = await management_proc.read_stdout()
    mgmt_stderr = await management_proc.read_stderr()
    raise TimeoutError(
        f"{proc.label} did not register within {timeout}s; "
        f"last registry snapshot had {last_seen}; "
        f"last error: {last_error}; stderr: {stderr}; "
        f"management stdout: {mgmt_stdout}; management stderr: {mgmt_stderr}"
    )


# ---------------------------------------------------------------------------
# Shared fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def ports() -> Ports:
    """Allocate dynamic ports for this test."""
    return PortAllocator.allocate()


@pytest.fixture
def secrets_dir():
    """Temporary secrets directory for the management server."""
    with tempfile.TemporaryDirectory(prefix="e2e-secrets-") as d:
        yield d


@pytest.fixture(scope="session")
def event_loop_policy():
    """Use the default event loop policy."""
    import asyncio
    return asyncio.DefaultEventLoopPolicy()


@pytest_asyncio.fixture
async def management_server(ports: Ports, secrets_dir: str):
    """Start an isolated management server for each test."""
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


async def _spawn_rust_agent(management_server, ports: Ports, label_suffix: str = ""):
    """Helper: spawn a Rust agent process and wait for registration."""
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
        label=f"rust-agent-{agent_id}{label_suffix}",
    )
    await proc.start()
    await proc.wait_healthy(timeout=10)
    await _wait_for_agent_registration(ports, agent_id, proc, management_server)
    return agent_id, proc


@pytest_asyncio.fixture
async def rust_agent(management_server, ports: Ports):
    """Start a Rust agent, yield its agent_id, stop on teardown."""
    agent_id, proc = await _spawn_rust_agent(management_server, ports)
    yield agent_id
    await proc.stop()


@pytest_asyncio.fixture
async def rust_agent_2(management_server, ports: Ports):
    """Second independent Rust agent for concurrent-agent routing tests."""
    agent_id, proc = await _spawn_rust_agent(management_server, ports, "-2")
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
    await _wait_for_agent_registration(ports, agent_id, proc, management_server)
    yield agent_id, proc
    await proc.stop()
