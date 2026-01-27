"""Python SDK for agentic-sandbox API.

This package provides a client library for interacting with the agentic-sandbox
runtime isolation service. It supports creating and managing Docker containers
and QEMU VMs for running AI agents in secure, isolated environments.

Example:
    >>> from agentic_sandbox import SandboxClient, SandboxSpec
    >>> client = SandboxClient("http://localhost:8080")
    >>> spec = SandboxSpec(name="my-agent", runtime="docker", image="alpine:3.18")
    >>> sandbox = client.create_sandbox(spec)
    >>> result = client.exec_command(sandbox.id, ["echo", "hello"])
    >>> print(result.stdout)
    hello

"""

from .client import SandboxClient
from .models import (
    SandboxSpec,
    Sandbox,
    ExecResult,
    ResourceLimits,
    NetworkConfig,
    MountSpec,
    SecurityConfig,
    ResourceUsage,
)
from .exceptions import (
    SandboxError,
    NotFoundError,
    APIError,
)

__version__ = "0.1.0"

__all__ = [
    "SandboxClient",
    "SandboxSpec",
    "Sandbox",
    "ExecResult",
    "ResourceLimits",
    "NetworkConfig",
    "MountSpec",
    "SecurityConfig",
    "ResourceUsage",
    "SandboxError",
    "NotFoundError",
    "APIError",
]
