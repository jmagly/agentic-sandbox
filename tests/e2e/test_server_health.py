"""Tests for management server health and WebSocket connectivity."""

import asyncio

import aiohttp
import pytest
import pytest_asyncio

from .helpers import WSTestClient


pytestmark = pytest.mark.asyncio


async def test_http_health_endpoint(management_server, ports):
    """GET /api/v1/health returns 200 with expected body."""
    url = f"http://127.0.0.1:{ports.http}/api/v1/health"
    async with aiohttp.ClientSession() as session:
        async with session.get(url) as resp:
            assert resp.status == 200
            body = await resp.json()
            assert body["status"] == "ok"
            assert body["service"] == "agentic-management"


async def test_websocket_connects(management_server, ports):
    """WebSocket handshake succeeds and connection stays open."""
    client = WSTestClient()
    await client.connect(f"ws://127.0.0.1:{ports.ws}")
    # Connection should be alive
    assert client._ws is not None
    await client.close()


async def test_ws_ping_pong(ws_client: WSTestClient):
    """Send a ping message, receive a pong with matching timestamp."""
    pong = await ws_client.ping()
    assert pong["type"] == "pong"
    assert "timestamp" in pong
