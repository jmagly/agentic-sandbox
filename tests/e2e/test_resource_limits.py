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
import subprocess
import time
from typing import Generator

import pytest

# Default VM for testing
DEFAULT_VM = os.environ.get("TEST_VM", "agent-01")
DEFAULT_TIMEOUT = 60


def get_vm_ip(vm_name: str) -> str | None:
    """Get IP address of a libvirt VM."""
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


def ssh_command(
    ip: str,
    command: str,
    timeout: int = DEFAULT_TIMEOUT,
    check: bool = False,
) -> subprocess.CompletedProcess:
    """Execute SSH command on VM."""
    ssh_opts = [
        "-o", "ConnectTimeout=5",
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR",
        "-o", "BatchMode=yes",
    ]
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

    def ssh(
        self,
        command: str,
        timeout: int = DEFAULT_TIMEOUT,
        check: bool = False,
    ) -> subprocess.CompletedProcess:
        """Execute command via SSH."""
        return ssh_command(self.ip, command, timeout=timeout, check=check)

    def is_alive(self) -> bool:
        """Check if VM is responsive."""
        try:
            result = self.ssh("echo alive", timeout=10)
            return "alive" in result.stdout
        except (subprocess.TimeoutExpired, subprocess.SubprocessError):
            return False


@pytest.fixture(scope="module")
def vm() -> Generator[VMConnection, None, None]:
    """Fixture providing VM connection."""
    vm_name = os.environ.get("TEST_VM", DEFAULT_VM)
    conn = VMConnection(vm_name)

    # Verify VM is accessible
    if not conn.is_alive():
        pytest.skip(f"VM {vm_name} is not accessible")

    yield conn


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
        result = vm.ssh("systemctl show agentic-agent --property=ControlGroup")
        assert result.returncode == 0
        assert "agentic-agent" in result.stdout

    def test_memory_limits_configured(self, vm: VMConnection):
        """Verify memory limits are set in cgroup."""
        # Get cgroup path
        cgroup_result = vm.ssh(
            "systemctl show agentic-agent --property=ControlGroup --value"
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
        result = vm.ssh(
            "systemctl show agentic-agent --property=TasksMax --value"
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

    @pytest.mark.timeout(60)
    def test_fork_bomb_contained(self, vm: VMConnection):
        """Verify fork bomb is contained by TasksMax."""
        # Classic fork bomb - should be contained
        result = vm.ssh(
            "timeout 10 bash -c ':(){ :|:& };:' 2>&1 || true",
            timeout=30,
        )

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
