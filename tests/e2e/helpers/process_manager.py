"""Subprocess lifecycle management for E2E tests."""

from __future__ import annotations

import asyncio
import os
import signal
from typing import Optional

import aiohttp


class ManagedProcess:
    """Manages a subprocess with health checking and clean teardown."""

    def __init__(
        self,
        cmd: list[str],
        env: Optional[dict[str, str]] = None,
        health_url: Optional[str] = None,
        label: str = "process",
    ):
        self.cmd = cmd
        self.env = {**os.environ, **(env or {})}
        self.health_url = health_url
        self.label = label
        self._proc: Optional[asyncio.subprocess.Process] = None

    @property
    def is_running(self) -> bool:
        return self._proc is not None and self._proc.returncode is None

    @property
    def pid(self) -> Optional[int]:
        return self._proc.pid if self._proc else None

    async def start(self) -> None:
        """Start the subprocess."""
        self._proc = await asyncio.create_subprocess_exec(
            *self.cmd,
            env=self.env,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

    async def stop(self, timeout: float = 5.0) -> None:
        """Gracefully stop the subprocess."""
        if not self.is_running:
            return

        # Send SIGTERM first
        try:
            self._proc.send_signal(signal.SIGTERM)
        except ProcessLookupError:
            return

        try:
            await asyncio.wait_for(self._proc.wait(), timeout=timeout)
        except asyncio.TimeoutError:
            # Force kill
            try:
                self._proc.kill()
                await self._proc.wait()
            except ProcessLookupError:
                pass

    async def wait_healthy(self, timeout: float = 15.0, interval: float = 0.3) -> None:
        """Poll health endpoint until it responds 200, or raise on timeout."""
        if not self.health_url:
            # No health URL; just wait a bit for the process to start
            await asyncio.sleep(1.0)
            if not self.is_running:
                stdout = await self._proc.stdout.read() if self._proc.stdout else b""
                stderr = await self._proc.stderr.read() if self._proc.stderr else b""
                raise RuntimeError(
                    f"{self.label} exited immediately (code={self._proc.returncode})\n"
                    f"stdout: {stdout.decode(errors='replace')}\n"
                    f"stderr: {stderr.decode(errors='replace')}"
                )
            return

        deadline = asyncio.get_event_loop().time() + timeout
        last_error = None

        while asyncio.get_event_loop().time() < deadline:
            if not self.is_running:
                stdout = await self._proc.stdout.read() if self._proc.stdout else b""
                stderr = await self._proc.stderr.read() if self._proc.stderr else b""
                raise RuntimeError(
                    f"{self.label} exited during health check (code={self._proc.returncode})\n"
                    f"stdout: {stdout.decode(errors='replace')}\n"
                    f"stderr: {stderr.decode(errors='replace')}"
                )
            try:
                async with aiohttp.ClientSession() as session:
                    async with session.get(self.health_url, timeout=aiohttp.ClientTimeout(total=2)) as resp:
                        if resp.status == 200:
                            return
                        last_error = f"HTTP {resp.status}"
            except Exception as e:
                last_error = str(e)

            await asyncio.sleep(interval)

        raise TimeoutError(
            f"{self.label} did not become healthy within {timeout}s. "
            f"Last error: {last_error}"
        )

    async def read_stderr(self, n: int = 4096) -> str:
        """Read available stderr (non-blocking best-effort)."""
        if self._proc and self._proc.stderr:
            try:
                data = await asyncio.wait_for(self._proc.stderr.read(n), timeout=0.5)
                return data.decode(errors="replace")
            except asyncio.TimeoutError:
                return ""
        return ""
