"""Pydantic models for agentic-sandbox API."""
from datetime import datetime
from typing import Optional, Dict, List, Literal
from pydantic import BaseModel, Field, field_validator


class ResourceLimits(BaseModel):
    """Resource limits for a sandbox."""

    cpu: int = Field(default=4, ge=1, le=128, description="Number of CPU cores")
    memory: str = Field(
        default="8G", pattern=r"^[0-9]+[KMG]$", description="Memory limit (e.g., 8G, 512M)"
    )
    disk: Optional[str] = Field(
        default=None, pattern=r"^[0-9]+[KMG]$", description="Disk size (QEMU only)"
    )
    pids_limit: int = Field(
        default=1024, ge=64, le=4096, description="Maximum number of processes"
    )


class NetworkConfig(BaseModel):
    """Network configuration for a sandbox."""

    mode: Literal["isolated", "gateway", "host"] = Field(
        default="isolated", description="Network isolation mode"
    )
    gateway_url: Optional[str] = Field(
        default=None, description="Auth gateway URL (required if mode=gateway)"
    )


class MountSpec(BaseModel):
    """Volume mount specification."""

    source: str = Field(description="Host path")
    target: str = Field(description="Container/VM path")
    mode: Literal["ro", "rw"] = Field(default="ro", description="Mount mode")


class SecurityConfig(BaseModel):
    """Security configuration for a sandbox."""

    read_only_root: bool = Field(
        default=True, description="Mount root filesystem as read-only"
    )
    seccomp_profile: Optional[str] = Field(
        default=None, description="Seccomp profile name"
    )
    capabilities_drop: List[str] = Field(
        default_factory=lambda: ["ALL"], description="Linux capabilities to drop"
    )


class SandboxSpec(BaseModel):
    """Specification for creating a sandbox."""

    name: str = Field(
        pattern=r"^[a-z0-9][a-z0-9-]{0,62}$",
        description="Unique sandbox name (DNS-compatible)",
    )
    runtime: Literal["docker", "qemu"] = Field(description="Runtime type")
    image: str = Field(description="Container image or VM template name")
    resources: ResourceLimits = Field(
        default_factory=ResourceLimits, description="Resource limits"
    )
    network: NetworkConfig = Field(
        default_factory=NetworkConfig, description="Network configuration"
    )
    mounts: List[MountSpec] = Field(
        default_factory=list, description="Volume mounts"
    )
    environment: Dict[str, str] = Field(
        default_factory=dict, description="Environment variables"
    )
    security: SecurityConfig = Field(
        default_factory=SecurityConfig, description="Security configuration"
    )
    auto_start: bool = Field(
        default=True, description="Start sandbox immediately after creation"
    )

    model_config = {"extra": "forbid"}


class ResourceUsage(BaseModel):
    """Current resource usage statistics."""

    cpu_percent: float = Field(description="CPU usage percentage")
    memory_bytes: int = Field(description="Memory usage in bytes")
    memory_percent: float = Field(description="Memory usage percentage")
    pids_current: int = Field(description="Current number of processes")


class Sandbox(BaseModel):
    """Sandbox status and details."""

    id: str = Field(pattern=r"^sb-[a-f0-9]{8}$", description="Unique sandbox ID")
    name: str = Field(description="Sandbox name")
    state: Literal["creating", "running", "stopped", "error"] = Field(
        description="Current sandbox state"
    )
    runtime: Literal["docker", "qemu"] = Field(description="Runtime type")
    image: str = Field(description="Container image or VM template")
    created: datetime = Field(description="Creation timestamp")
    started: Optional[datetime] = Field(default=None, description="Last start timestamp")
    stopped: Optional[datetime] = Field(default=None, description="Last stop timestamp")
    uptime: Optional[int] = Field(default=None, description="Uptime in seconds")
    resources: ResourceLimits = Field(description="Configured resource limits")
    resource_usage: Optional[ResourceUsage] = Field(
        default=None, description="Current resource usage"
    )
    network: Literal["isolated", "gateway", "host"] = Field(
        description="Network mode"
    )
    gateway_url: Optional[str] = Field(
        default=None, description="Auth gateway URL (if network=gateway)"
    )
    mounts: List[MountSpec] = Field(default_factory=list, description="Volume mounts")
    environment: Dict[str, str] = Field(
        default_factory=dict, description="Environment variables"
    )
    error: Optional[str] = Field(
        default=None, description="Error message (if state=error)"
    )

    model_config = {"extra": "allow"}


class ExecResult(BaseModel):
    """Result of command execution in a sandbox."""

    exit_code: int = Field(description="Command exit code (124 = timeout)")
    stdout: str = Field(description="Standard output")
    stderr: str = Field(description="Standard error")
    duration_ms: Optional[int] = Field(
        default=None, description="Execution duration in milliseconds"
    )
    timed_out: bool = Field(default=False, description="Whether command timed out")
