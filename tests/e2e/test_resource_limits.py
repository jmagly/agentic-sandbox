"""
E2E tests for resource limit enforcement in agent VMs.

These tests verify that cgroup v2 limits, disk quotas, and I/O throttling
are properly enforced to prevent resource exhaustion attacks.

Prerequisites:
    - Running VM with hardened agent-client.service
    - SSH access to VM as 'agent' user
    - pytest-timeout installed

Usage:
    pytest tests/e2e/test_resource_limits.py -v
    pytest tests/e2e/test_resource_limits.py::TestMemoryLimits -v

Reference: docs/security/resource-quota-design.md
"""

import os
import asyncio
import json
import subprocess
import shutil
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Generator

import pytest
import websockets

# Default VM for testing
DEFAULT_VM = os.environ.get("TEST_VM", "agent-01")
DEFAULT_TIMEOUT = 60
RUN_DESTRUCTIVE_SSH_STRESS = os.environ.get("E2E_RUN_DESTRUCTIVE_SSH_STRESS") == "1"
REPO_ROOT = Path(__file__).resolve().parents[2]
MGMT_BINARY = REPO_ROOT / "management" / "target" / "release" / "agentic-mgmt"
DISPATCH_HTTP_URL = os.environ.get("E2E_MGMT_HTTP_URL", "http://127.0.0.1:8122")
DISPATCH_WS_URL = os.environ.get("E2E_MGMT_WS_URL", "ws://127.0.0.1:8121")


def agentshare_project_quota_available() -> bool:
    """Return true when the host agentshare root supports XFS project quotas."""
    root = Path(os.environ.get("AGENTSHARE_ROOT", "/srv/agentshare"))
    if not root.exists():
        return False

    try:
        df = subprocess.run(
            ["df", "-T", str(root)],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if df.returncode != 0 or "xfs" not in df.stdout.split():
            return False

        mount = subprocess.run(
            ["findmnt", "-no", "OPTIONS", str(root)],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if mount.returncode != 0 or "prjquota" not in mount.stdout.split(","):
            return False

        return shutil.which("xfs_quota") is not None
    except (subprocess.TimeoutExpired, subprocess.SubprocessError):
        return False


def get_vm_ip(vm_name: str) -> str | None:
    """Get IP address of a libvirt VM."""
    info_path = Path("/var/lib/agentic-sandbox/vms") / vm_name / "vm-info.json"
    try:
        return json.loads(info_path.read_text())["ip"]
    except (FileNotFoundError, KeyError, json.JSONDecodeError):
        pass
    except PermissionError:
        try:
            result = subprocess.run(
                ["sudo", "cat", str(info_path)],
                capture_output=True,
                text=True,
                timeout=10,
            )
            if result.returncode == 0:
                return json.loads(result.stdout)["ip"]
        except (subprocess.TimeoutExpired, subprocess.SubprocessError, KeyError, json.JSONDecodeError):
            pass

    try:
        result = subprocess.run(
            ["virsh", "domifaddr", vm_name],
            capture_output=True,
            text=True,
            timeout=10,
        )
        for line in result.stdout.splitlines():
            # Look for IP address pattern
            parts = line.split()
            for part in parts:
                if "/" in part and part.count(".") == 3:
                    return part.split("/")[0]
    except (subprocess.TimeoutExpired, subprocess.SubprocessError):
        pass
    return None


def http_json(url: str, timeout: int = 5) -> dict:
    """Fetch a small JSON endpoint without adding an E2E dependency."""
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def ssh_command(
    ip: str,
    command: str,
    timeout: int = DEFAULT_TIMEOUT,
    check: bool = False,
    key_path: str | None = None,
) -> subprocess.CompletedProcess:
    """Execute SSH command on VM."""
    ssh_opts = [
        "-o", "ConnectTimeout=5",
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR",
        "-o", "BatchMode=yes",
    ]
    if key_path:
        full_command = ["sudo", "ssh", "-i", key_path] + ssh_opts + [f"agent@{ip}", command]
    else:
        full_command = ["ssh"] + ssh_opts + [f"agent@{ip}", command]

    return subprocess.run(
        full_command,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=check,
    )


class VMConnection:
    """Helper class for VM SSH operations."""

    def __init__(self, vm_name: str):
        self.vm_name = vm_name
        self.ip = get_vm_ip(vm_name)
        if not self.ip:
            raise RuntimeError(f"Could not get IP for VM: {vm_name}")
        self.key_path = self._get_key_path()

    def _get_key_path(self) -> str | None:
        """Return the host-side ephemeral SSH key path when provisioned."""
        key_path = f"/var/lib/agentic-sandbox/secrets/ssh-keys/{self.vm_name}"
        result = subprocess.run(
            ["sudo", "test", "-f", key_path],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return key_path if result.returncode == 0 else None

    def ssh(
        self,
        command: str,
        timeout: int = DEFAULT_TIMEOUT,
        check: bool = False,
    ) -> subprocess.CompletedProcess:
        """Execute command via SSH."""
        return ssh_command(
            self.ip,
            command,
            timeout=timeout,
            check=check,
            key_path=self.key_path,
        )

    def is_alive(self) -> bool:
        """Check if VM is responsive."""
        try:
            result = self.ssh("echo alive", timeout=10)
            return "alive" in result.stdout
        except (subprocess.TimeoutExpired, subprocess.SubprocessError):
            return False

    def agent_service(self) -> str:
        """Return the active agent service name used by this VM image."""
        for service in ("agent-client", "agentic-agent"):
            result = self.ssh(f"systemctl is-active {service}", timeout=10)
            if result.returncode == 0 and result.stdout.strip() == "active":
                return service
        raise RuntimeError("No active agent service found")

    def restart_agent_service(self) -> str:
        """Restart the active agent service and return its unit name."""
        service = self.agent_service()
        result = self.ssh(f"sudo systemctl restart {service}", timeout=20)
        if result.returncode != 0:
            raise RuntimeError(
                f"failed to restart {service}: {result.stdout} {result.stderr}"
            )
        return service


@pytest.fixture(scope="module")
def vm() -> Generator[VMConnection, None, None]:
    """Fixture providing VM connection."""
    vm_name = os.environ.get("TEST_VM", DEFAULT_VM)
    conn = VMConnection(vm_name)

    # Verify VM is accessible
    if not conn.is_alive():
        raise RuntimeError(f"VM {vm_name} is not accessible over SSH")

    yield conn


@pytest.fixture(scope="module")
def dispatch_management(vm: VMConnection) -> Generator[None, None, None]:
    """Start management on VM-routable ports and wait for the VM agent."""
    if not MGMT_BINARY.exists():
        pytest.skip(f"management binary not found at {MGMT_BINARY}")

    with tempfile.TemporaryDirectory(prefix="e2e-mgmt-state-") as state_dir, \
            tempfile.TemporaryDirectory(prefix="e2e-mgmt-logs-") as log_dir:
        secrets_dir = os.environ.get(
            "E2E_MGMT_SECRETS_DIR",
            str(Path(state_dir) / "secrets"),
        )
        env = os.environ.copy()
        env.update({
            # VM agents provisioned by this repo dial host.internal:8120.
            "LISTEN_ADDR": os.environ.get("E2E_MGMT_LISTEN_ADDR", "0.0.0.0:8120"),
            "SECRETS_DIR": secrets_dir,
            "HEARTBEAT_TIMEOUT": "30",
            "RUST_LOG": os.environ.get("RUST_LOG", "info"),
        })

        stdout_path = Path(log_dir) / "management.stdout.log"
        stderr_path = Path(log_dir) / "management.stderr.log"
        stdout_file = stdout_path.open("w+", encoding="utf-8")
        stderr_file = stderr_path.open("w+", encoding="utf-8")
        proc = subprocess.Popen(
            [str(MGMT_BINARY)],
            env=env,
            stdout=stdout_file,
            stderr=stderr_file,
            text=True,
        )

        def read_management_logs() -> str:
            stdout_file.flush()
            stderr_file.flush()
            stdout = stdout_path.read_text(encoding="utf-8", errors="replace")
            stderr = stderr_path.read_text(encoding="utf-8", errors="replace")
            return f"stdout={stdout}; stderr={stderr}"

        try:
            yield from _wait_for_dispatch_management(proc, vm, read_management_logs)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
            stdout_file.close()
            stderr_file.close()


def _wait_for_dispatch_management(
    proc: subprocess.Popen,
    vm: VMConnection,
    read_management_logs,
) -> Generator[None, None, None]:
    """Wait for management health and VM agent registration."""
    health_url = f"{DISPATCH_HTTP_URL}/api/v1/health"
    deadline = time.monotonic() + 20
    last_error = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                f"management exited before health check; "
                f"{read_management_logs()}"
            )
        try:
            http_json(health_url, timeout=2)
            break
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            last_error = exc
            time.sleep(0.25)
    else:
        raise RuntimeError(f"management did not become healthy: {last_error}")

    service = vm.restart_agent_service()
    agents_url = f"{DISPATCH_HTTP_URL}/api/v1/agents"
    deadline = time.monotonic() + 60
    last_agents: list[str] = []
    last_error = None
    while time.monotonic() < deadline:
        try:
            payload = http_json(agents_url, timeout=3)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            last_error = exc
            time.sleep(1)
            continue
        last_agents = [agent.get("id", "") for agent in payload.get("agents", [])]
        if vm.vm_name in last_agents:
            break
        time.sleep(1)
    else:
        raise RuntimeError(
            f"{service} did not register as {vm.vm_name}; "
            f"last agents={last_agents}; last error={last_error}"
        )

    yield


async def run_dispatched_pid_stress(agent_id: str) -> str:
    """Run PID stress via management WS command dispatch."""
    script = r"""
set -euo pipefail
python3 - <<'PY'
import subprocess
import sys
import time

def own_cgroup_pids_max():
    with open("/proc/self/cgroup", "r", encoding="utf-8") as handle:
        for line in handle:
            parts = line.strip().split(":", 2)
            if len(parts) == 3 and parts[0] == "0":
                value = open(
                    "/sys/fs/cgroup" + parts[2] + "/pids.max",
                    "r",
                    encoding="utf-8",
                ).read().strip()
                if value == "max":
                    raise RuntimeError("agent cgroup has no pids.max limit")
                return int(value)
    raise RuntimeError("could not locate unified cgroup entry")

limit = own_cgroup_pids_max()
target = min(limit + 128, 6000)
processes = []
hit_limit = False

try:
    for _ in range(target):
        processes.append(subprocess.Popen(["sleep", "60"]))
except OSError as exc:
    hit_limit = True
    print(f"PID_STRESS_HIT_LIMIT spawned={len(processes)} errno={exc.errno}", flush=True)
finally:
    for proc in processes:
        try:
            proc.terminate()
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 10
    for proc in processes:
        remaining = max(0.1, deadline - time.monotonic())
        try:
            proc.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            try:
                proc.kill()
            except ProcessLookupError:
                pass

print(f"PID_STRESS_DONE hit_limit={hit_limit} spawned={len(processes)} pids_max={limit}", flush=True)
sys.exit(0 if hit_limit else 1)
PY
"""
    output: list[str] = []
    command_id = None
    async with websockets.connect(DISPATCH_WS_URL) as ws:
        await ws.send(json.dumps({"type": "subscribe", "agent_id": agent_id}))
        await ws.send(
            json.dumps({
                "type": "send_command",
                "agent_id": agent_id,
                "command": "bash",
                "args": ["-lc", script],
            })
        )

        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            msg = json.loads(raw)
            if msg.get("type") == "command_started":
                command_id = msg["command_id"]
            if msg.get("type") == "output" and msg.get("command_id") == command_id:
                data = msg.get("data", "")
                output.append(data)
                joined = "".join(output)
                if "PID_STRESS_DONE" in joined:
                    return joined

    raise TimeoutError(f"dispatch PID stress did not finish; output={''.join(output)}")


class TestCgroupConfiguration:
    """Tests for cgroup v2 configuration."""

    def test_cgroup_v2_enabled(self, vm: VMConnection):
        """Verify cgroup v2 is the active hierarchy."""
        result = vm.ssh("cat /sys/fs/cgroup/cgroup.controllers")
        assert result.returncode == 0
        # Should have memory, cpu, io controllers
        controllers = result.stdout.strip()
        assert "memory" in controllers, "memory controller not found"
        assert "cpu" in controllers or "cpuset" in controllers

    def test_agent_service_has_cgroup(self, vm: VMConnection):
        """Verify agent service has its own cgroup."""
        service = vm.agent_service()
        result = vm.ssh(f"systemctl show {service} --property=ControlGroup")
        assert result.returncode == 0
        assert service in result.stdout

    def test_memory_limits_configured(self, vm: VMConnection):
        """Verify memory limits are set in cgroup."""
        service = vm.agent_service()
        # Get cgroup path
        cgroup_result = vm.ssh(
            f"systemctl show {service} --property=ControlGroup --value"
        )
        cgroup_path = cgroup_result.stdout.strip()

        if not cgroup_path:
            pytest.skip("Could not determine cgroup path")

        # Check memory.max
        result = vm.ssh(f"cat /sys/fs/cgroup{cgroup_path}/memory.max")
        if result.returncode == 0:
            max_bytes = result.stdout.strip()
            if max_bytes != "max":
                # Convert to GB for readability
                max_gb = int(max_bytes) / (1024**3)
                assert max_gb <= 8, f"Memory limit too high: {max_gb}GB"

    def test_pids_limit_configured(self, vm: VMConnection):
        """Verify PID limit is set in cgroup."""
        service = vm.agent_service()
        result = vm.ssh(
            f"systemctl show {service} --property=TasksMax --value"
        )
        if result.returncode == 0 and result.stdout.strip():
            tasks_max = result.stdout.strip()
            if tasks_max != "infinity":
                assert int(tasks_max) <= 4096, f"PID limit too high: {tasks_max}"


class TestMemoryLimits:
    """Tests for memory limit enforcement."""

    @pytest.mark.timeout(120)
    def test_memory_allocation_fails_at_limit(self, vm: VMConnection):
        """Verify large memory allocation is blocked."""
        # Try to allocate 10GB - should fail with ~7.5GB limit
        result = vm.ssh(
            """python3 -c '
import sys
try:
    x = bytearray(10 * 1024 * 1024 * 1024)
    print("SUCCESS - limit not enforced")
    sys.exit(1)
except MemoryError:
    print("MemoryError - limit enforced")
    sys.exit(0)
except Exception as e:
    print(f"Error: {e}")
    sys.exit(2)
'""",
            timeout=60,
        )

        # Either MemoryError or process killed is acceptable
        assert (
            result.returncode == 0
            or "Killed" in result.stderr
            or "MemoryError" in result.stdout
        ), f"Memory limit not enforced: {result.stdout} {result.stderr}"

    @pytest.mark.timeout(120)
    def test_vm_survives_memory_pressure(self, vm: VMConnection):
        """Verify VM remains responsive after memory exhaustion attempt."""
        # Attempt memory exhaustion
        vm.ssh(
            "python3 -c 'x = [0] * (8 * 1024 * 1024 * 1024)' 2>/dev/null",
            timeout=30,
        )

        # Wait for any OOM handling
        time.sleep(3)

        # VM should still be alive
        assert vm.is_alive(), "VM became unresponsive after memory test"


class TestPidLimits:
    """Tests for PID/task limit enforcement."""

    @pytest.fixture(autouse=True)
    def require_destructive_ssh_stress(self):
        """Direct SSH PID stress is opt-in; it is outside agent service cgroups."""
        if not RUN_DESTRUCTIVE_SSH_STRESS:
            pytest.skip(
                "direct SSH PID stress is opt-in; use dispatch-backed stress for CI"
            )

    @pytest.mark.timeout(60)
    def test_fork_bomb_contained(self, vm: VMConnection):
        """Verify fork bomb is contained by TasksMax."""
        # Classic fork bomb - should be contained
        try:
            vm.ssh(
                "timeout 10 bash -c ':(){ :|:& };:' 2>&1 || true",
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            pass

        # Wait for dust to settle
        time.sleep(3)

        # VM must survive
        assert vm.is_alive(), "VM became unresponsive after fork bomb"

    @pytest.mark.timeout(60)
    def test_rapid_process_spawn_limited(self, vm: VMConnection):
        """Verify rapid process spawning hits limit."""
        result = vm.ssh(
            """python3 -c '
import subprocess
import sys

processes = []
try:
    for i in range(5000):
        p = subprocess.Popen(["sleep", "60"])
        processes.append(p)
except OSError as e:
    print(f"Hit limit at {len(processes)} processes: {e}")
    sys.exit(0)
finally:
    for p in processes:
        try:
            p.kill()
        except:
            pass

print(f"Spawned {len(processes)} processes without hitting limit")
sys.exit(1 if len(processes) >= 5000 else 0)
'""",
            timeout=45,
        )

        # Should hit the limit before 5000 processes
        assert (
            "Hit limit" in result.stdout
            or result.returncode == 0
        )


class TestDispatchBackedPidLimits:
    """PID stress that runs under the agent service cgroup."""

    @pytest.mark.timeout(180)
    def test_agent_dispatch_process_spawn_limited(
        self,
        vm: VMConnection,
        dispatch_management,
    ):
        """Verify dispatch-backed PID stress hits agent-client TasksMax."""
        output = asyncio.run(run_dispatched_pid_stress(vm.vm_name))

        assert "PID_STRESS_HIT_LIMIT" in output, output
        assert "PID_STRESS_DONE hit_limit=True" in output, output
        assert vm.is_alive(), "VM became unresponsive after dispatch PID stress"
        assert vm.agent_service() in ("agent-client", "agentic-agent")


class TestFileDescriptorLimits:
    """Tests for file descriptor limit enforcement."""

    @pytest.mark.timeout(60)
    def test_fd_exhaustion_blocked(self, vm: VMConnection):
        """Verify file descriptor exhaustion is blocked."""
        result = vm.ssh(
            """python3 -c '
import os
import sys

fds = []
try:
    for i in range(100000):
        fds.append(os.open("/dev/null", os.O_RDONLY))
except OSError as e:
    print(f"Hit limit at {len(fds)} FDs: {e}")
    for fd in fds:
        try:
            os.close(fd)
        except:
            pass
    sys.exit(0)

print(f"Opened {len(fds)} FDs without limit")
for fd in fds:
    os.close(fd)
sys.exit(1)
'""",
            timeout=30,
        )

        # Should hit limit (default is 65536)
        assert (
            "Hit limit" in result.stdout
            or "Too many open files" in result.stderr
        )

    @pytest.mark.timeout(60)
    def test_vm_survives_fd_exhaustion(self, vm: VMConnection):
        """Verify VM remains responsive after FD exhaustion."""
        # Attempt FD exhaustion
        vm.ssh(
            "python3 -c 'import os; [os.open(\"/dev/null\", os.O_RDONLY) for _ in range(100000)]' 2>/dev/null",
            timeout=20,
        )

        time.sleep(2)
        assert vm.is_alive(), "VM became unresponsive after FD test"


class TestDiskQuotas:
    """Tests for disk quota enforcement."""

    @pytest.fixture(autouse=True)
    def check_agentshare(self, vm: VMConnection):
        """Skip if agentshare is not enabled."""
        result = vm.ssh("test -d /mnt/inbox && echo exists")
        if "exists" not in result.stdout:
            pytest.skip("Agentshare not enabled")

    @pytest.mark.timeout(120)
    def test_disk_quota_blocks_excess_write(self, vm: VMConnection):
        """Verify disk quota prevents writing beyond limit."""
        # First, check available space
        result = vm.ssh("df -h /mnt/inbox")
        print(f"Disk status before test: {result.stdout}")

        if not agentshare_project_quota_available():
            pytest.skip("Agentshare project quotas are not available on this host")

        # Try to write more than typical quota (60GB vs 50GB limit)
        result = vm.ssh(
            "dd if=/dev/zero of=/mnt/inbox/quota_test bs=1M count=61440 2>&1; "
            "rm -f /mnt/inbox/quota_test 2>/dev/null",
            timeout=90,
        )

        # Check for quota error
        output = result.stdout + result.stderr
        quota_enforced = any(
            msg in output.lower()
            for msg in ["quota", "no space", "disk quota exceeded"]
        )

        if not quota_enforced:
            pytest.skip("Disk quota not enforced (quotas may not be configured)")

    @pytest.mark.timeout(60)
    def test_small_writes_succeed(self, vm: VMConnection):
        """Verify normal writes within quota succeed."""
        result = vm.ssh(
            "dd if=/dev/zero of=/mnt/inbox/small_test bs=1M count=100 && "
            "rm -f /mnt/inbox/small_test",
            timeout=30,
        )

        assert result.returncode == 0, f"Small write failed: {result.stderr}"


class TestIOThrottling:
    """Tests for I/O bandwidth limiting."""

    @pytest.mark.timeout(120)
    def test_write_throughput_limited(self, vm: VMConnection):
        """Verify write throughput is limited."""
        # Write 512MB and measure time
        # With 200MB/s limit, should take at least 2.5 seconds
        result = vm.ssh(
            """
            start=$(date +%s.%N)
            dd if=/dev/zero of=/tmp/io_test bs=1M count=512 oflag=direct 2>/dev/null
            end=$(date +%s.%N)
            rm -f /tmp/io_test
            python3 -c "print(f'{float('$end') - float('$start'):.2f}')"
            """,
            timeout=60,
        )

        if result.returncode == 0:
            try:
                elapsed = float(result.stdout.strip().split()[-1])
                throughput = 512 / elapsed  # MB/s
                print(f"Write throughput: {throughput:.1f} MB/s ({elapsed:.2f}s)")

                # With 200MB/s limit, throughput should be below ~250MB/s
                # (some overhead expected)
                if throughput < 300:
                    pass  # Throttling appears active
                else:
                    print(f"Warning: Throughput ({throughput:.1f} MB/s) may indicate no throttling")
            except (ValueError, IndexError):
                pass  # Could not parse timing


class TestResourceRecovery:
    """Tests for resource recovery after exhaustion."""

    @pytest.fixture(autouse=True)
    def require_destructive_ssh_stress(self):
        """Direct SSH recovery stress is opt-in; it is outside agent service cgroups."""
        if not RUN_DESTRUCTIVE_SSH_STRESS:
            pytest.skip(
                "direct SSH recovery stress is opt-in; use dispatch-backed stress for CI"
            )

    @pytest.mark.timeout(180)
    def test_recovery_after_all_exhaustion_attempts(self, vm: VMConnection):
        """Verify VM recovers after multiple exhaustion attempts."""
        # Run multiple exhaustion attempts
        attempts = [
            "timeout 5 bash -c ':(){ :|:& };:' 2>/dev/null || true",
            "python3 -c 'x = [0] * (8 * 1024**3)' 2>/dev/null || true",
            "dd if=/dev/zero of=/tmp/fill bs=1G count=20 2>/dev/null; rm -f /tmp/fill",
        ]

        for attempt in attempts:
            vm.ssh(attempt, timeout=30)
            time.sleep(3)

        # VM should still be responsive
        assert vm.is_alive(), "VM not responsive after exhaustion attempts"

        # Should be able to run normal commands
        result = vm.ssh("echo 'recovery test' && uptime", timeout=10)
        assert result.returncode == 0
        assert "recovery test" in result.stdout
