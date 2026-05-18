"""Subprocess lifecycle management for E2E tests."""

from __future__ import annotations

import asyncio
import os
import signal
from typing import Optional

import aiohttp


class ManagedProcess:
    """Manages a subprocess with health checking and clean teardown."""

    _MAX_CAPTURE_BYTES = 65536

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
        self._stdout = bytearray()
        self._stderr = bytearray()
        self._drain_tasks: list[asyncio.Task] = []

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
        self._drain_tasks = [
            asyncio.create_task(self._drain_stream(self._proc.stdout, self._stdout)),
            asyncio.create_task(self._drain_stream(self._proc.stderr, self._stderr)),
        ]

    async def _drain_stream(
        self,
        stream: Optional[asyncio.StreamReader],
        buffer: bytearray,
    ) -> None:
        """Continuously drain subprocess output into a bounded buffer."""
        if stream is None:
            return

        try:
            while chunk := await stream.read(4096):
                buffer.extend(chunk)
                overflow = len(buffer) - self._MAX_CAPTURE_BYTES
                if overflow > 0:
                    del buffer[:overflow]
        except asyncio.CancelledError:
            raise
        except Exception:
            # Output capture is diagnostic only; process lifecycle stays primary.
            return

    async def stop(self, timeout: float = 5.0) -> None:
        """Gracefully stop the subprocess."""
        if not self.is_running:
            await self._stop_drain_tasks()
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
        finally:
            await self._stop_drain_tasks()

    async def _stop_drain_tasks(self) -> None:
        """Stop background output-drain tasks after process termination."""
        for task in self._drain_tasks:
            if not task.done():
                task.cancel()
        for task in self._drain_tasks:
            try:
                await task
            except asyncio.CancelledError:
                pass
        self._drain_tasks = []

    async def wait_healthy(self, timeout: float = 15.0, interval: float = 0.3) -> None:
        """Poll health endpoint until it responds 200, or raise on timeout."""
        if not self.health_url:
            # No health URL; just wait a bit for the process to start
            await asyncio.sleep(1.0)
            if not self.is_running:
                raise RuntimeError(
                    f"{self.label} exited immediately (code={self._proc.returncode})\n"
                    f"stdout: {await self.read_stdout()}\n"
                    f"stderr: {await self.read_stderr()}"
                )
            return

        deadline = asyncio.get_event_loop().time() + timeout
        last_error = None

        while asyncio.get_event_loop().time() < deadline:
            if not self.is_running:
                raise RuntimeError(
                    f"{self.label} exited during health check (code={self._proc.returncode})\n"
                    f"stdout: {await self.read_stdout()}\n"
                    f"stderr: {await self.read_stderr()}"
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
        """Return recently captured stderr."""
        return bytes(self._stderr[-n:]).decode(errors="replace")

    async def read_stdout(self, n: int = 4096) -> str:
        """Return recently captured stdout."""
        return bytes(self._stdout[-n:]).decode(errors="replace")
