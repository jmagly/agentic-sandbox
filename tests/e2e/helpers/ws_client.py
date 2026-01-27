"""Async WebSocket test client for the management server."""

from __future__ import annotations

import asyncio
import json
import time
from typing import Any, Optional

import websockets
from websockets.asyncio.client import ClientConnection


class WSTestClient:
    """WebSocket client for E2E tests against the management server."""

    def __init__(self):
        self._ws: Optional[ClientConnection] = None
        self._inbox: list[dict[str, Any]] = []
        self._recv_task: Optional[asyncio.Task] = None

    async def connect(self, url: str, timeout: float = 5.0) -> None:
        """Open a WebSocket connection."""
        self._ws = await asyncio.wait_for(
            websockets.connect(url),
            timeout=timeout,
        )
        # Start background receiver
        self._recv_task = asyncio.create_task(self._receiver())

    async def _receiver(self) -> None:
        """Background task that reads messages into the inbox."""
        try:
            async for raw in self._ws:
                msg = json.loads(raw)
                self._inbox.append(msg)
        except websockets.exceptions.ConnectionClosed:
            pass
        except Exception:
            pass

    async def close(self) -> None:
        """Close the connection."""
        if self._recv_task:
            self._recv_task.cancel()
            try:
                await self._recv_task
            except asyncio.CancelledError:
                pass
        if self._ws:
            await self._ws.close()

    async def send(self, msg: dict[str, Any]) -> None:
        """Send a JSON message."""
        await self._ws.send(json.dumps(msg))

    async def subscribe(self, agent_id: str = "*") -> dict:
        """Subscribe to agent output."""
        await self.send({"type": "subscribe", "agent_id": agent_id})
        return await self.wait_for_message("subscribed", timeout=5)

    async def list_agents(self) -> list[dict]:
        """Request and return the list of connected agents."""
        await self.send({"type": "list_agents"})
        msg = await self.wait_for_message("agent_list", timeout=5)
        return msg["agents"]

    async def send_command(
        self, agent_id: str, command: str, args: list[str] | None = None
    ) -> str:
        """Send a command to an agent, return command_id."""
        await self.send({
            "type": "send_command",
            "agent_id": agent_id,
            "command": command,
            "args": args or [],
        })
        msg = await self.wait_for_message("command_started", timeout=10)
        return msg["command_id"]

    async def send_input(
        self, agent_id: str, command_id: str, data: str
    ) -> dict:
        """Send stdin data to a running command."""
        await self.send({
            "type": "send_input",
            "agent_id": agent_id,
            "command_id": command_id,
            "data": data,
        })
        return await self.wait_for_message("input_sent", timeout=5)

    async def wait_for_message(
        self, msg_type: str, timeout: float = 5.0, **match_fields
    ) -> dict:
        """Wait for a specific message type to appear in the inbox."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for i, msg in enumerate(self._inbox):
                if msg.get("type") == msg_type:
                    if all(msg.get(k) == v for k, v in match_fields.items()):
                        self._inbox.pop(i)
                        return msg
            await asyncio.sleep(0.05)
        raise TimeoutError(
            f"Timed out waiting for message type={msg_type} "
            f"fields={match_fields}. Inbox has: "
            f"{[m.get('type') for m in self._inbox]}"
        )

    async def collect_output(
        self, command_id: str, timeout: float = 10.0
    ) -> list[dict]:
        """Collect all output messages for a command until no more arrive."""
        collected = []
        deadline = time.monotonic() + timeout
        quiet_deadline = time.monotonic() + 2.0  # 2s of silence = done

        while time.monotonic() < deadline:
            found = False
            for i, msg in enumerate(self._inbox):
                if (
                    msg.get("type") == "output"
                    and msg.get("command_id") == command_id
                ):
                    collected.append(self._inbox.pop(i))
                    found = True
                    quiet_deadline = time.monotonic() + 2.0
                    break

            if not found:
                if time.monotonic() > quiet_deadline:
                    break
                await asyncio.sleep(0.05)

        return collected

    def drain_inbox(self) -> list[dict]:
        """Return and clear all queued messages."""
        msgs = list(self._inbox)
        self._inbox.clear()
        return msgs

    async def ping(self) -> dict:
        """Send a ping, return the pong."""
        ts = int(time.time() * 1000)
        await self.send({"type": "ping", "timestamp": ts})
        return await self.wait_for_message("pong", timeout=5)
