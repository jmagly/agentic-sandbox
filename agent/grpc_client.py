#!/usr/bin/env python3
"""
Agentic Sandbox - Agent gRPC Client

Runs inside the agent VM, connects to management server on boot.
Establishes bidirectional stream for commands and output streaming.

Authentication: Uses AGENT_SECRET from /etc/agentic-sandbox/agent.env
The secret is passed as metadata on every request.

Usage:
    python3 grpc_client.py --server HOST:PORT --agent-id agent-01

Or via environment (loaded from agent.env):
    source /etc/agentic-sandbox/agent.env
    python3 grpc_client.py
"""

import argparse
import grpc
import json
import logging
import os
import platform
import psutil
import queue
import shutil
import signal
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Iterator, Optional, Generator, TextIO

# Add proto directory to path
sys.path.insert(0, os.path.dirname(__file__))
from proto import agent_pb2, agent_pb2_grpc

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    datefmt='%H:%M:%S'
)
log = logging.getLogger(__name__)


# =============================================================================
# Agentshare File Logger
# =============================================================================

class AgentshareLogger:
    """Writes agent output to agentshare inbox filesystem alongside gRPC."""

    INBOX_PATH = Path('/mnt/inbox')

    def __init__(self, agent_id: str):
        self.agent_id = agent_id
        self.run_id = f"run-{datetime.now().strftime('%Y%m%d-%H%M%S')}"
        self.run_dir: Optional[Path] = None
        self.stdout_file: Optional[TextIO] = None
        self.stderr_file: Optional[TextIO] = None
        self.commands_file: Optional[TextIO] = None
        self.enabled = False
        self._lock = threading.Lock()

    def initialize(self) -> bool:
        """Initialize run directory and log files. Returns True if agentshare available."""
        if not self.INBOX_PATH.exists():
            log.info("Agentshare inbox not mounted - file logging disabled")
            return False

        try:
            # Create run directory
            self.run_dir = self.INBOX_PATH / 'runs' / self.run_id
            self.run_dir.mkdir(parents=True, exist_ok=True)
            (self.run_dir / 'outputs').mkdir(exist_ok=True)
            (self.run_dir / 'trace').mkdir(exist_ok=True)

            # Update current symlink
            current_link = self.INBOX_PATH / 'current'
            if current_link.is_symlink():
                current_link.unlink()
            current_link.symlink_to(self.run_dir)

            # Open log files
            self.stdout_file = open(self.run_dir / 'stdout.log', 'a', buffering=1)
            self.stderr_file = open(self.run_dir / 'stderr.log', 'a', buffering=1)
            self.commands_file = open(self.run_dir / 'commands.log', 'a', buffering=1)

            self.enabled = True
            log.info(f"Agentshare logging initialized: {self.run_dir}")

            # Write initial run metadata
            self._write_metadata()

            return True

        except Exception as e:
            log.error(f"Failed to initialize agentshare logging: {e}")
            return False

    def _write_metadata(self):
        """Write run metadata file."""
        if not self.run_dir:
            return
        metadata = {
            'run_id': self.run_id,
            'agent_id': self.agent_id,
            'started_at': datetime.now().isoformat(),
            'hostname': platform.node(),
            'platform': platform.platform(),
        }
        with open(self.run_dir / 'metadata.json', 'w') as f:
            json.dump(metadata, f, indent=2)

    def write_stdout(self, data: bytes):
        """Write stdout data to file."""
        if not self.enabled or not self.stdout_file:
            return
        with self._lock:
            try:
                text = data.decode('utf-8', errors='replace')
                self.stdout_file.write(text)
                self.stdout_file.flush()
            except Exception as e:
                log.debug(f"Error writing stdout: {e}")

    def write_stderr(self, data: bytes):
        """Write stderr data to file."""
        if not self.enabled or not self.stderr_file:
            return
        with self._lock:
            try:
                text = data.decode('utf-8', errors='replace')
                self.stderr_file.write(text)
                self.stderr_file.flush()
            except Exception as e:
                log.debug(f"Error writing stderr: {e}")

    def write_command(self, command_id: str, command: str, args: list):
        """Log command execution."""
        if not self.enabled or not self.commands_file:
            return
        with self._lock:
            try:
                timestamp = datetime.now().isoformat()
                entry = f"[{timestamp}] [{command_id}] {command} {' '.join(args)}\n"
                self.commands_file.write(entry)
                self.commands_file.flush()
            except Exception as e:
                log.debug(f"Error writing command: {e}")

    def write_command_result(self, command_id: str, exit_code: int, duration_ms: int):
        """Log command completion."""
        if not self.enabled or not self.commands_file:
            return
        with self._lock:
            try:
                timestamp = datetime.now().isoformat()
                entry = f"[{timestamp}] [{command_id}] EXIT {exit_code} ({duration_ms}ms)\n"
                self.commands_file.write(entry)
                self.commands_file.flush()
            except Exception as e:
                log.debug(f"Error writing command result: {e}")

    def write_metrics(self):
        """Write current metrics snapshot."""
        if not self.enabled or not self.run_dir:
            return
        try:
            mem = psutil.virtual_memory()
            disk = shutil.disk_usage('/')
            load = os.getloadavg()

            metrics = {
                'timestamp': datetime.now().isoformat(),
                'cpu_percent': psutil.cpu_percent(interval=0.1),
                'memory': {
                    'used_bytes': mem.used,
                    'total_bytes': mem.total,
                    'percent': mem.percent,
                },
                'disk': {
                    'used_bytes': disk.used,
                    'total_bytes': disk.total,
                    'percent': (disk.used / disk.total) * 100,
                },
                'load_avg': list(load),
            }

            with open(self.run_dir / 'metrics.json', 'w') as f:
                json.dump(metrics, f, indent=2)

        except Exception as e:
            log.debug(f"Error writing metrics: {e}")

    def close(self):
        """Close log files and finalize run."""
        if not self.enabled:
            return

        # Write final metrics
        self.write_metrics()

        # Update metadata with end time
        if self.run_dir:
            try:
                metadata_path = self.run_dir / 'metadata.json'
                if metadata_path.exists():
                    with open(metadata_path) as f:
                        metadata = json.load(f)
                    metadata['ended_at'] = datetime.now().isoformat()
                    with open(metadata_path, 'w') as f:
                        json.dump(metadata, f, indent=2)
            except Exception as e:
                log.debug(f"Error finalizing metadata: {e}")

        # Close files
        for f in [self.stdout_file, self.stderr_file, self.commands_file]:
            if f:
                try:
                    f.close()
                except Exception:
                    pass

        self.enabled = False
        log.info("Agentshare logging closed")

# =============================================================================
# Configuration
# =============================================================================

@dataclass
class AgentConfig:
    agent_id: str
    agent_secret: str
    server_address: str
    heartbeat_interval: int = 30
    reconnect_delay: int = 5
    max_reconnect_delay: int = 60

    @classmethod
    def from_env(cls, env_file: str = '/etc/agentic-sandbox/agent.env') -> 'AgentConfig':
        """Load config from environment file and environment variables."""
        # Load from env file if exists
        if os.path.exists(env_file):
            with open(env_file) as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        key, value = line.split('=', 1)
                        os.environ.setdefault(key.strip(), value.strip())

        return cls(
            agent_id=os.environ.get('AGENT_ID', platform.node()),
            agent_secret=os.environ.get('AGENT_SECRET', ''),
            server_address=os.environ.get('MANAGEMENT_SERVER', 'host.internal:8120'),
            heartbeat_interval=int(os.environ.get('HEARTBEAT_INTERVAL', '30')),
            reconnect_delay=int(os.environ.get('RECONNECT_DELAY', '5')),
            max_reconnect_delay=int(os.environ.get('MAX_RECONNECT_DELAY', '60')),
        )


class _ClientCallDetails(
    grpc.ClientCallDetails,
):
    """Writable ClientCallDetails for interceptor use."""

    def __init__(self, method, timeout, metadata, credentials, wait_for_ready, compression):
        self.method = method
        self.timeout = timeout
        self.metadata = metadata
        self.credentials = credentials
        self.wait_for_ready = wait_for_ready
        self.compression = compression


class AuthInterceptor(grpc.UnaryUnaryClientInterceptor,
                      grpc.UnaryStreamClientInterceptor,
                      grpc.StreamUnaryClientInterceptor,
                      grpc.StreamStreamClientInterceptor):
    """Adds authentication token to all gRPC calls."""

    def __init__(self, agent_id: str, agent_secret: str):
        self.agent_id = agent_id
        self.agent_secret = agent_secret

    def _add_auth_metadata(self, client_call_details):
        """Add authentication headers to request metadata."""
        metadata = list(client_call_details.metadata or [])
        metadata.append(('x-agent-id', self.agent_id))
        metadata.append(('x-agent-secret', self.agent_secret))
        return _ClientCallDetails(
            client_call_details.method,
            client_call_details.timeout,
            metadata,
            client_call_details.credentials,
            client_call_details.wait_for_ready,
            client_call_details.compression,
        )

    def intercept_unary_unary(self, continuation, client_call_details, request):
        return continuation(self._add_auth_metadata(client_call_details), request)

    def intercept_unary_stream(self, continuation, client_call_details, request):
        return continuation(self._add_auth_metadata(client_call_details), request)

    def intercept_stream_unary(self, continuation, client_call_details, request_iterator):
        return continuation(self._add_auth_metadata(client_call_details), request_iterator)

    def intercept_stream_stream(self, continuation, client_call_details, request_iterator):
        return continuation(self._add_auth_metadata(client_call_details), request_iterator)


# =============================================================================
# System Information
# =============================================================================

def get_system_info() -> agent_pb2.SystemInfo:
    """Collect system information for registration."""
    try:
        with open('/etc/os-release') as f:
            os_info = dict(line.strip().split('=', 1) for line in f if '=' in line)
        os_name = os_info.get('PRETTY_NAME', 'Unknown').strip('"')
    except Exception:
        os_name = platform.platform()

    return agent_pb2.SystemInfo(
        os=os_name,
        kernel=platform.release(),
        cpu_cores=psutil.cpu_count(),
        memory_bytes=psutil.virtual_memory().total,
        disk_bytes=shutil.disk_usage('/').total,
    )


def get_metrics(agent_id: str) -> agent_pb2.Metrics:
    """Collect current system metrics."""
    mem = psutil.virtual_memory()
    disk = shutil.disk_usage('/')
    load = os.getloadavg()

    return agent_pb2.Metrics(
        agent_id=agent_id,
        timestamp_ms=int(time.time() * 1000),
        cpu_percent=psutil.cpu_percent(interval=0.1),
        memory_used_bytes=mem.used,
        memory_total_bytes=mem.total,
        disk_used_bytes=disk.used,
        disk_total_bytes=disk.total,
        load_avg=list(load),
    )


def get_ip_address() -> str:
    """Get primary IP address."""
    try:
        import socket
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(('8.8.8.8', 80))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except Exception:
        return '0.0.0.0'


# =============================================================================
# Command Executor
# =============================================================================

class CommandExecutor:
    """Executes commands and streams output."""

    def __init__(self, output_callback, agentshare_logger: Optional[AgentshareLogger] = None):
        """
        Args:
            output_callback: Callable that receives AgentMessage objects
            agentshare_logger: Optional file logger for agentshare
        """
        self.output_callback = output_callback
        self.agentshare = agentshare_logger
        self.active_processes = {}  # command_id -> subprocess.Popen
        self._lock = threading.Lock()
        self.status = agent_pb2.AGENT_STATUS_READY

    def write_stdin(self, command_id: str, data: bytes, eof: bool = False) -> bool:
        """Write data to stdin of a running command.

        Args:
            command_id: The command to write to
            data: Bytes to write to stdin
            eof: If True, close stdin after writing

        Returns:
            True if successful, False if command not found or write failed
        """
        with self._lock:
            proc = self.active_processes.get(command_id)
            if not proc or not proc.stdin:
                log.warning(f"Cannot write stdin: command {command_id} not found or has no stdin")
                return False

            try:
                if data:
                    proc.stdin.write(data)
                    proc.stdin.flush()
                    log.debug(f"[{command_id}] Wrote {len(data)} bytes to stdin")

                if eof:
                    proc.stdin.close()
                    log.debug(f"[{command_id}] Closed stdin")

                return True
            except Exception as e:
                log.error(f"[{command_id}] Failed to write stdin: {e}")
                return False

    def execute(self, cmd: agent_pb2.CommandRequest) -> None:
        """Execute command in background, stream output via callback."""
        self.status = agent_pb2.AGENT_STATUS_BUSY

        # Log command to agentshare
        if self.agentshare:
            self.agentshare.write_command(cmd.command_id, cmd.command, list(cmd.args))

        thread = threading.Thread(
            target=self._run_command,
            args=(cmd,),
            daemon=True
        )
        thread.start()

    def _run_command(self, cmd: agent_pb2.CommandRequest) -> None:
        """Run command and stream output."""
        start_time = time.time()
        full_cmd = [cmd.command] + list(cmd.args)

        # Prepend sudo -u if run_as specified
        if cmd.run_as and cmd.run_as != os.getenv('USER'):
            full_cmd = ['sudo', '-u', cmd.run_as] + full_cmd

        # Merge environment
        proc_env = os.environ.copy()
        if cmd.env:
            proc_env.update(cmd.env)

        log.info(f"[{cmd.command_id}] Executing: {' '.join(full_cmd)}")

        try:
            proc = subprocess.Popen(
                full_cmd,
                stdin=subprocess.PIPE,  # Enable stdin for interactive commands
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                cwd=cmd.working_dir or None,
                env=proc_env,
                bufsize=0
            )

            with self._lock:
                self.active_processes[cmd.command_id] = proc

            # Stream stdout and stderr in parallel
            def stream_pipe(pipe, is_stdout: bool):
                try:
                    while True:
                        chunk = pipe.read(4096)
                        if not chunk:
                            break
                        output = agent_pb2.OutputChunk(
                            stream_id=cmd.command_id,
                            data=chunk,
                            timestamp_ms=int(time.time() * 1000),
                        )
                        if is_stdout:
                            msg = agent_pb2.AgentMessage(stdout=output)
                            # Also write to agentshare
                            if self.agentshare:
                                self.agentshare.write_stdout(chunk)
                        else:
                            msg = agent_pb2.AgentMessage(stderr=output)
                            # Also write to agentshare
                            if self.agentshare:
                                self.agentshare.write_stderr(chunk)
                        self.output_callback(msg)
                except Exception as e:
                    log.error(f"Error reading pipe: {e}")

            stdout_thread = threading.Thread(target=stream_pipe, args=(proc.stdout, True))
            stderr_thread = threading.Thread(target=stream_pipe, args=(proc.stderr, False))
            stdout_thread.start()
            stderr_thread.start()

            # Wait for completion with optional timeout
            timeout = cmd.timeout_seconds if cmd.timeout_seconds > 0 else None
            try:
                exit_code = proc.wait(timeout=timeout)
                error_msg = ""
            except subprocess.TimeoutExpired:
                proc.kill()
                exit_code = -1
                error_msg = f"Command timed out after {cmd.timeout_seconds}s"
                log.warning(f"[{cmd.command_id}] {error_msg}")

            stdout_thread.join()
            stderr_thread.join()

            duration_ms = int((time.time() - start_time) * 1000)

            # Send completion result
            result = agent_pb2.CommandResult(
                command_id=cmd.command_id,
                exit_code=exit_code,
                error=error_msg,
                duration_ms=duration_ms,
                success=(exit_code == 0),
            )
            self.output_callback(agent_pb2.AgentMessage(command_result=result))

            # Log to agentshare
            if self.agentshare:
                self.agentshare.write_command_result(cmd.command_id, exit_code, duration_ms)

            log.info(f"[{cmd.command_id}] Completed: exit={exit_code}, duration={duration_ms}ms")

        except Exception as e:
            log.error(f"[{cmd.command_id}] Execution failed: {e}")
            result = agent_pb2.CommandResult(
                command_id=cmd.command_id,
                exit_code=-1,
                error=str(e),
                success=False,
            )
            self.output_callback(agent_pb2.AgentMessage(command_result=result))

        finally:
            with self._lock:
                self.active_processes.pop(cmd.command_id, None)
                if not self.active_processes:
                    self.status = agent_pb2.AGENT_STATUS_READY

    def cancel(self, command_id: str) -> bool:
        """Cancel a running command."""
        with self._lock:
            proc = self.active_processes.get(command_id)
            if proc:
                proc.terminate()
                return True
        return False


# =============================================================================
# Agent Client
# =============================================================================

class AgentClient:
    """gRPC client for agent-to-management communication."""

    def __init__(self, config: AgentConfig):
        self.config = config
        self.running = False
        self.connected = False
        self.channel = None
        self.stub = None
        self.outbound_queue = queue.Queue()

        # Initialize agentshare logger
        self.agentshare = AgentshareLogger(config.agent_id)
        self.agentshare.initialize()

        self.executor = CommandExecutor(self._queue_message, self.agentshare)

    def _queue_message(self, msg: agent_pb2.AgentMessage):
        """Queue outbound message for sending."""
        self.outbound_queue.put(msg)

    def connect(self) -> bool:
        """Establish connection to management server."""
        try:
            log.info(f"Connecting to {self.config.server_address}...")

            # Create channel with authentication interceptor
            channel = grpc.insecure_channel(self.config.server_address)
            interceptor = AuthInterceptor(self.config.agent_id, self.config.agent_secret)
            self.channel = grpc.intercept_channel(channel, interceptor)
            self.stub = agent_pb2_grpc.AgentServiceStub(self.channel)

            self.connected = True
            log.info("Connected to management server")
            return True

        except Exception as e:
            log.error(f"Connection failed: {e}")
            return False

    def _create_registration(self) -> agent_pb2.AgentMessage:
        """Create registration message."""
        reg = agent_pb2.AgentRegistration(
            agent_id=self.config.agent_id,
            ip_address=get_ip_address(),
            hostname=platform.node(),
            profile=os.environ.get('AGENT_PROFILE', 'basic'),
            system=get_system_info(),
        )
        return agent_pb2.AgentMessage(registration=reg)

    def _create_heartbeat(self) -> agent_pb2.AgentMessage:
        """Create heartbeat message."""
        hb = agent_pb2.Heartbeat(
            agent_id=self.config.agent_id,
            timestamp_ms=int(time.time() * 1000),
            status=self.executor.status,
            cpu_percent=psutil.cpu_percent(interval=0),
            memory_used_bytes=psutil.virtual_memory().used,
            uptime_seconds=int(time.time() - psutil.boot_time()),
        )
        return agent_pb2.AgentMessage(heartbeat=hb)

    def _outbound_generator(self) -> Generator[agent_pb2.AgentMessage, None, None]:
        """Generate outbound messages for the stream."""
        # First message: registration
        yield self._create_registration()
        log.info(f"Sent registration for {self.config.agent_id}")

        last_heartbeat = time.time()

        while self.running and self.connected:
            # Send heartbeat if interval elapsed
            now = time.time()
            if now - last_heartbeat >= self.config.heartbeat_interval:
                yield self._create_heartbeat()
                last_heartbeat = now

            # Send queued output messages
            try:
                while True:
                    msg = self.outbound_queue.get_nowait()
                    yield msg
            except queue.Empty:
                pass

            time.sleep(0.1)

    def _handle_inbound(self, msg: agent_pb2.ManagementMessage) -> None:
        """Handle inbound message from management."""
        payload = msg.WhichOneof('payload')

        if payload == 'registration_ack':
            ack = msg.registration_ack
            if ack.accepted:
                log.info(f"Registration accepted: {ack.message}")
                if ack.heartbeat_interval_seconds > 0:
                    self.config.heartbeat_interval = ack.heartbeat_interval_seconds
            else:
                log.error(f"Registration rejected: {ack.message}")
                self.running = False

        elif payload == 'command':
            cmd = msg.command
            log.info(f"Received command: {cmd.command_id} - {cmd.command}")
            self.executor.execute(cmd)

        elif payload == 'config':
            cfg = msg.config
            log.info(f"Config update received: {cfg.config}")
            # Apply config changes
            for key, value in cfg.config.items():
                os.environ[key] = value

        elif payload == 'shutdown':
            sig = msg.shutdown
            log.info(f"Shutdown signal received: {sig.reason}")
            log.info(f"Grace period: {sig.grace_period_seconds}s")
            # Start graceful shutdown
            threading.Timer(sig.grace_period_seconds, self.stop).start()

        elif payload == 'ping':
            ping = msg.ping
            log.debug(f"Ping received: {ping.timestamp_ms}")
            # Could send pong heartbeat

        elif payload == 'stdin':
            stdin_chunk = msg.stdin
            log.debug(f"Received stdin for command {stdin_chunk.command_id}: {len(stdin_chunk.data)} bytes")
            self.executor.write_stdin(
                stdin_chunk.command_id,
                stdin_chunk.data,
                stdin_chunk.eof
            )

    def run(self) -> None:
        """Main run loop with reconnection logic."""
        self.running = True
        reconnect_delay = self.config.reconnect_delay

        # Start metrics logging thread if agentshare enabled
        if self.agentshare and self.agentshare.enabled:
            metrics_thread = threading.Thread(
                target=self._metrics_loop,
                daemon=True
            )
            metrics_thread.start()

        while self.running:
            if not self.connect():
                log.info(f"Retrying in {reconnect_delay}s...")
                time.sleep(reconnect_delay)
                reconnect_delay = min(reconnect_delay * 2, self.config.max_reconnect_delay)
                continue

            # Reset delay on successful connection
            reconnect_delay = self.config.reconnect_delay

            try:
                self._stream_loop()
            except grpc.RpcError as e:
                log.error(f"gRPC error: {e.code()} - {e.details()}")
                self.connected = False
            except Exception as e:
                log.error(f"Stream error: {e}")
                self.connected = False

            if self.channel:
                self.channel.close()
                self.channel = None

        log.info("Agent client stopped")

    def _stream_loop(self) -> None:
        """Bidirectional streaming loop."""
        log.info("Starting bidirectional stream...")

        # Open bidirectional stream
        responses = self.stub.Connect(self._outbound_generator())

        # Process inbound messages
        for msg in responses:
            if not self.running:
                break
            self._handle_inbound(msg)

    def _metrics_loop(self) -> None:
        """Periodically write metrics to agentshare."""
        metrics_interval = 60  # Write metrics every 60 seconds
        while self.running:
            time.sleep(metrics_interval)
            if self.agentshare:
                self.agentshare.write_metrics()

    def stop(self) -> None:
        """Stop the client gracefully."""
        log.info("Stopping agent client...")
        self.running = False
        self.connected = False
        # Close agentshare logger
        if self.agentshare:
            self.agentshare.close()


# =============================================================================
# Main
# =============================================================================

def main():
    parser = argparse.ArgumentParser(description='Agentic Sandbox Agent')
    parser.add_argument('--server',
                        default=os.environ.get('MANAGEMENT_SERVER'),
                        help='Management server address (host:port)')
    parser.add_argument('--agent-id',
                        default=os.environ.get('AGENT_ID'),
                        help='Agent identifier')
    parser.add_argument('--secret',
                        default=os.environ.get('AGENT_SECRET'),
                        help='Agent authentication secret')
    parser.add_argument('--env-file',
                        default='/etc/agentic-sandbox/agent.env',
                        help='Environment file path')
    parser.add_argument('--heartbeat', type=int, default=30,
                        help='Heartbeat interval in seconds')
    args = parser.parse_args()

    # Load config (will read env-file and merge with CLI args)
    config = AgentConfig.from_env(args.env_file)

    # Override with CLI args if provided
    if args.server:
        config.server_address = args.server
    if args.agent_id:
        config.agent_id = args.agent_id
    if args.secret:
        config.agent_secret = args.secret
    config.heartbeat_interval = args.heartbeat

    # Validate config
    if not config.agent_id:
        log.error("AGENT_ID required")
        sys.exit(1)
    if not config.agent_secret:
        log.warning("AGENT_SECRET not set - authentication may fail")
    if not config.server_address:
        log.error("MANAGEMENT_SERVER required")
        sys.exit(1)

    client = AgentClient(config)

    # Handle shutdown signals
    def shutdown(signum, frame):
        log.info(f"Received signal {signum}")
        client.stop()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    log.info(f"Starting agent: {config.agent_id}")
    log.info(f"Management server: {config.server_address}")

    try:
        client.run()
    except KeyboardInterrupt:
        client.stop()


if __name__ == '__main__':
    main()
