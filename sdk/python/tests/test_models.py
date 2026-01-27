"""Test suite for Pydantic models."""
import pytest
from datetime import datetime
from pydantic import ValidationError
from agentic_sandbox.models import (
    SandboxSpec,
    Sandbox,
    ExecResult,
    ResourceLimits,
    NetworkConfig,
    MountSpec,
    SecurityConfig,
    ResourceUsage,
)


class TestResourceLimits:
    """Test ResourceLimits model."""

    def test_resource_limits_defaults(self):
        """Test ResourceLimits with default values."""
        limits = ResourceLimits()
        assert limits.cpu == 4
        assert limits.memory == "8G"
        assert limits.pids_limit == 1024

    def test_resource_limits_custom(self):
        """Test ResourceLimits with custom values."""
        limits = ResourceLimits(cpu=8, memory="16G", disk="50G", pids_limit=2048)
        assert limits.cpu == 8
        assert limits.memory == "16G"
        assert limits.disk == "50G"
        assert limits.pids_limit == 2048

    def test_resource_limits_validation_cpu_min(self):
        """Test CPU validation minimum."""
        with pytest.raises(ValidationError):
            ResourceLimits(cpu=0)

    def test_resource_limits_validation_cpu_max(self):
        """Test CPU validation maximum."""
        with pytest.raises(ValidationError):
            ResourceLimits(cpu=129)

    def test_resource_limits_validation_memory_pattern(self):
        """Test memory pattern validation."""
        # Valid patterns
        ResourceLimits(memory="512M")
        ResourceLimits(memory="8G")
        ResourceLimits(memory="1024K")

        # Invalid pattern
        with pytest.raises(ValidationError):
            ResourceLimits(memory="8GB")  # Wrong suffix

    def test_resource_limits_serialization(self):
        """Test ResourceLimits serializes to dict."""
        limits = ResourceLimits(cpu=8, memory="16G")
        data = limits.model_dump()
        assert data["cpu"] == 8
        assert data["memory"] == "16G"
        assert data["pids_limit"] == 1024


class TestNetworkConfig:
    """Test NetworkConfig model."""

    def test_network_config_isolated(self):
        """Test isolated network mode (default)."""
        config = NetworkConfig()
        assert config.mode == "isolated"
        assert config.gateway_url is None

    def test_network_config_gateway(self):
        """Test gateway network mode."""
        config = NetworkConfig(mode="gateway", gateway_url="http://gateway:8080")
        assert config.mode == "gateway"
        assert config.gateway_url == "http://gateway:8080"

    def test_network_config_host(self):
        """Test host network mode."""
        config = NetworkConfig(mode="host")
        assert config.mode == "host"

    def test_network_config_invalid_mode(self):
        """Test invalid network mode."""
        with pytest.raises(ValidationError):
            NetworkConfig(mode="invalid")


class TestMountSpec:
    """Test MountSpec model."""

    def test_mount_spec_readonly(self):
        """Test read-only mount."""
        mount = MountSpec(source="/host/path", target="/container/path", mode="ro")
        assert mount.source == "/host/path"
        assert mount.target == "/container/path"
        assert mount.mode == "ro"

    def test_mount_spec_readwrite(self):
        """Test read-write mount."""
        mount = MountSpec(source="/workspace", target="/workspace", mode="rw")
        assert mount.mode == "rw"

    def test_mount_spec_default_mode(self):
        """Test default mount mode is read-only."""
        mount = MountSpec(source="/src", target="/dst")
        assert mount.mode == "ro"


class TestSecurityConfig:
    """Test SecurityConfig model."""

    def test_security_config_defaults(self):
        """Test SecurityConfig defaults."""
        config = SecurityConfig()
        assert config.read_only_root is True
        assert config.capabilities_drop == ["ALL"]

    def test_security_config_custom(self):
        """Test SecurityConfig with custom values."""
        config = SecurityConfig(
            read_only_root=False,
            seccomp_profile="agent-strict",
            capabilities_drop=["NET_RAW", "SYS_ADMIN"],
        )
        assert config.read_only_root is False
        assert config.seccomp_profile == "agent-strict"
        assert "NET_RAW" in config.capabilities_drop


class TestSandboxSpec:
    """Test SandboxSpec model."""

    def test_sandbox_spec_minimal(self):
        """Test minimal SandboxSpec."""
        spec = SandboxSpec(name="test-agent", runtime="docker", image="alpine:3.18")
        assert spec.name == "test-agent"
        assert spec.runtime == "docker"
        assert spec.image == "alpine:3.18"
        assert spec.auto_start is True
        assert spec.resources.cpu == 4

    def test_sandbox_spec_full(self):
        """Test full SandboxSpec with all fields."""
        spec = SandboxSpec(
            name="full-agent",
            runtime="docker",
            image="agent-claude",
            resources=ResourceLimits(cpu=8, memory="16G"),
            network=NetworkConfig(mode="gateway", gateway_url="http://gateway:8080"),
            mounts=[MountSpec(source="/workspace", target="/workspace", mode="rw")],
            environment={"AGENT_MODE": "autonomous", "DEBUG": "1"},
            security=SecurityConfig(
                read_only_root=True, seccomp_profile="agent-default"
            ),
            auto_start=True,
        )

        assert spec.name == "full-agent"
        assert spec.resources.cpu == 8
        assert spec.network.mode == "gateway"
        assert len(spec.mounts) == 1
        assert spec.environment["AGENT_MODE"] == "autonomous"
        assert spec.security.read_only_root is True

    def test_sandbox_spec_name_validation(self):
        """Test sandbox name validation (DNS-compatible)."""
        # Valid names
        SandboxSpec(name="test-agent", runtime="docker", image="alpine")
        SandboxSpec(name="a", runtime="docker", image="alpine")
        SandboxSpec(name="test-123", runtime="docker", image="alpine")

        # Invalid names
        with pytest.raises(ValidationError):
            SandboxSpec(name="Test-Agent", runtime="docker", image="alpine")  # Uppercase

        with pytest.raises(ValidationError):
            SandboxSpec(name="-test", runtime="docker", image="alpine")  # Starts with -

        with pytest.raises(ValidationError):
            SandboxSpec(name="test_agent", runtime="docker", image="alpine")  # Underscore

    def test_sandbox_spec_runtime_validation(self):
        """Test runtime validation."""
        # Valid runtimes
        SandboxSpec(name="test", runtime="docker", image="alpine")
        SandboxSpec(name="test", runtime="qemu", image="ubuntu")

        # Invalid runtime
        with pytest.raises(ValidationError):
            SandboxSpec(name="test", runtime="podman", image="alpine")

    def test_sandbox_spec_serialization(self):
        """Test SandboxSpec serializes correctly."""
        spec = SandboxSpec(
            name="test-agent",
            runtime="docker",
            image="alpine",
            environment={"KEY": "value"},
        )
        data = spec.model_dump(exclude_none=True)
        assert data["name"] == "test-agent"
        assert data["runtime"] == "docker"
        assert data["image"] == "alpine"
        assert data["environment"] == {"KEY": "value"}


class TestResourceUsage:
    """Test ResourceUsage model."""

    def test_resource_usage_creation(self):
        """Test ResourceUsage model."""
        usage = ResourceUsage(
            cpu_percent=45.2,
            memory_bytes=2147483648,
            memory_percent=25.0,
            pids_current=42,
        )
        assert usage.cpu_percent == 45.2
        assert usage.memory_bytes == 2147483648
        assert usage.memory_percent == 25.0
        assert usage.pids_current == 42


class TestSandbox:
    """Test Sandbox model."""

    def test_sandbox_minimal(self):
        """Test minimal Sandbox model."""
        sandbox = Sandbox(
            id="sb-d8f3a9c4",
            name="test-agent",
            state="running",
            runtime="docker",
            image="alpine",
            created=datetime.fromisoformat("2026-01-24T10:30:00+00:00"),
            resources=ResourceLimits(cpu=4, memory="8G"),
            network="isolated",
        )

        assert sandbox.id == "sb-d8f3a9c4"
        assert sandbox.name == "test-agent"
        assert sandbox.state == "running"
        assert sandbox.runtime == "docker"

    def test_sandbox_with_timestamps(self):
        """Test Sandbox with all timestamps."""
        now = datetime.fromisoformat("2026-01-24T10:30:00+00:00")
        started = datetime.fromisoformat("2026-01-24T10:30:02+00:00")
        stopped = datetime.fromisoformat("2026-01-24T11:00:00+00:00")

        sandbox = Sandbox(
            id="sb-12345678",
            name="test",
            state="stopped",
            runtime="docker",
            image="alpine",
            created=now,
            started=started,
            stopped=stopped,
            resources=ResourceLimits(),
            network="isolated",
        )

        assert sandbox.created == now
        assert sandbox.started == started
        assert sandbox.stopped == stopped

    def test_sandbox_with_resource_usage(self):
        """Test Sandbox with resource usage."""
        sandbox = Sandbox(
            id="sb-abcdef12",
            name="test",
            state="running",
            runtime="docker",
            image="alpine",
            created=datetime.now(),
            resources=ResourceLimits(cpu=4, memory="8G"),
            resource_usage=ResourceUsage(
                cpu_percent=45.2,
                memory_bytes=2147483648,
                memory_percent=25.0,
                pids_current=42,
            ),
            network="isolated",
        )

        assert sandbox.resource_usage.cpu_percent == 45.2
        assert sandbox.resource_usage.memory_bytes == 2147483648

    def test_sandbox_state_validation(self):
        """Test sandbox state validation."""
        # Valid states
        for state in ["creating", "running", "stopped", "error"]:
            Sandbox(
                id="sb-a1b2c3d4",
                name="test",
                state=state,
                runtime="docker",
                image="alpine",
                created=datetime.now(),
                resources=ResourceLimits(),
                network="isolated",
            )

        # Invalid state
        with pytest.raises(ValidationError):
            Sandbox(
                id="sb-a1b2c3d4",
                name="test",
                state="invalid",
                runtime="docker",
                image="alpine",
                created=datetime.now(),
                resources=ResourceLimits(),
                network="isolated",
            )

    def test_sandbox_with_error(self):
        """Test Sandbox in error state."""
        sandbox = Sandbox(
            id="sb-deadbeef",
            name="test",
            state="error",
            runtime="docker",
            image="alpine",
            created=datetime.now(),
            resources=ResourceLimits(),
            network="isolated",
            error="Failed to start container",
        )

        assert sandbox.state == "error"
        assert sandbox.error == "Failed to start container"


class TestExecResult:
    """Test ExecResult model."""

    def test_exec_result_success(self):
        """Test successful command execution result."""
        result = ExecResult(
            exit_code=0,
            stdout="hello world\n",
            stderr="",
            duration_ms=45,
            timed_out=False,
        )

        assert result.exit_code == 0
        assert result.stdout == "hello world\n"
        assert result.stderr == ""
        assert result.duration_ms == 45
        assert result.timed_out is False

    def test_exec_result_failure(self):
        """Test failed command execution result."""
        result = ExecResult(
            exit_code=1,
            stdout="",
            stderr="command not found\n",
            duration_ms=10,
            timed_out=False,
        )

        assert result.exit_code == 1
        assert "command not found" in result.stderr

    def test_exec_result_timeout(self):
        """Test timed out command result."""
        result = ExecResult(
            exit_code=124,
            stdout="partial output\n",
            stderr="Command timed out\n",
            duration_ms=30000,
            timed_out=True,
        )

        assert result.exit_code == 124
        assert result.timed_out is True
        assert result.duration_ms == 30000

    def test_exec_result_default_timeout_flag(self):
        """Test timed_out defaults to False."""
        result = ExecResult(
            exit_code=0, stdout="", stderr="", duration_ms=10
        )
        assert result.timed_out is False
