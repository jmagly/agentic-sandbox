# agentic-sandbox Python SDK

Python client library for the agentic-sandbox runtime isolation API. Create and manage isolated Docker containers and QEMU VMs for running AI agents with security hardening and resource limits.

## Installation

```bash
pip install agentic-sandbox
```

For development:

```bash
pip install -e ".[dev]"
```

## Quick Start

### Synchronous Client

```python
from agentic_sandbox import SandboxClient, SandboxSpec

# Create client
client = SandboxClient("http://localhost:8080")

# Create a sandbox
spec = SandboxSpec(
    name="my-agent",
    runtime="docker",
    image="alpine:3.18"
)
sandbox = client.create_sandbox(spec)
print(f"Created sandbox: {sandbox.id}")

# Execute commands
result = client.exec_command(sandbox.id, ["echo", "hello world"])
print(result.stdout)  # "hello world\n"

# List sandboxes
response = client.list_sandboxes(runtime="docker", state="running")
for sb in response["sandboxes"]:
    print(f"{sb.name}: {sb.state}")

# Stop and delete
client.stop_sandbox(sandbox.id)
client.delete_sandbox(sandbox.id)
```

### Asynchronous Client

```python
import asyncio
from agentic_sandbox import SandboxClient, SandboxSpec

async def main():
    # Create async client
    client = SandboxClient("http://localhost:8080", async_mode=True)

    # Create sandbox
    spec = SandboxSpec(name="async-agent", runtime="docker", image="alpine:3.18")
    sandbox = await client.create_sandbox(spec)

    # Execute command
    result = await client.exec_command(sandbox.id, ["ls", "-la"])
    print(result.stdout)

    # Clean up
    await client.delete_sandbox(sandbox.id, force=True)

asyncio.run(main())
```

## Advanced Usage

### Full Sandbox Specification

```python
from agentic_sandbox import (
    SandboxClient,
    SandboxSpec,
    ResourceLimits,
    NetworkConfig,
    MountSpec,
    SecurityConfig,
)

client = SandboxClient("http://localhost:8080")

spec = SandboxSpec(
    name="claude-agent",
    runtime="docker",
    image="agent-claude",
    resources=ResourceLimits(
        cpu=8,
        memory="16G",
        pids_limit=2048
    ),
    network=NetworkConfig(
        mode="gateway",
        gateway_url="http://gateway:8080"
    ),
    mounts=[
        MountSpec(source="/workspace", target="/workspace", mode="rw")
    ],
    environment={
        "AGENT_MODE": "autonomous",
        "AGENT_TASK": "Implement new feature"
    },
    security=SecurityConfig(
        read_only_root=True,
        seccomp_profile="agent-default",
        capabilities_drop=["ALL"]
    ),
    auto_start=True
)

sandbox = client.create_sandbox(spec)
```

### Execute Commands with Options

```python
# Execute with working directory and environment
result = client.exec_command(
    sandbox.id,
    ["python3", "script.py"],
    working_dir="/workspace",
    environment={"DEBUG": "1"},
    timeout=60
)

# Execute with stdin
result = client.exec_command(
    sandbox.id,
    ["python3", "-"],
    stdin="print('Hello from sandbox')\nprint(2 + 2)\n",
    timeout=30
)

# Check execution result
if result.exit_code == 0:
    print(f"Success: {result.stdout}")
else:
    print(f"Failed: {result.stderr}")

if result.timed_out:
    print(f"Command timed out after {result.duration_ms}ms")
```

### Error Handling

```python
from agentic_sandbox import SandboxClient, NotFoundError, APIError, SandboxError

client = SandboxClient("http://localhost:8080")

try:
    sandbox = client.get_sandbox("sb-missing")
except NotFoundError as e:
    print(f"Sandbox not found: {e.message}")
except APIError as e:
    print(f"API error ({e.status_code}): {e.message}")
except SandboxError as e:
    print(f"Sandbox error: {e.message}")
```

### List and Filter Sandboxes

```python
# List all running Docker sandboxes
response = client.list_sandboxes(
    runtime="docker",
    state="running",
    limit=50,
    offset=0
)

print(f"Total: {response['total']}")
for sandbox in response["sandboxes"]:
    print(f"{sandbox.name} ({sandbox.id}): {sandbox.state}")
    if sandbox.resource_usage:
        print(f"  CPU: {sandbox.resource_usage.cpu_percent}%")
        print(f"  Memory: {sandbox.resource_usage.memory_percent}%")
```

### Lifecycle Management

```python
# Create sandbox without auto-start
spec = SandboxSpec(
    name="manual-start",
    runtime="docker",
    image="alpine",
    auto_start=False
)
sandbox = client.create_sandbox(spec)
print(f"State: {sandbox.state}")  # "stopped"

# Start sandbox
sandbox = client.start_sandbox(sandbox.id)
print(f"State: {sandbox.state}")  # "running"

# Stop with custom timeout
sandbox = client.stop_sandbox(sandbox.id, timeout=30)
print(f"State: {sandbox.state}")  # "stopped"

# Delete (force if running)
client.delete_sandbox(sandbox.id, force=True)
```

## Models

### SandboxSpec

Specification for creating a sandbox:

- `name` (str): Unique DNS-compatible name
- `runtime` (str): "docker" or "qemu"
- `image` (str): Container image or VM template
- `resources` (ResourceLimits): CPU, memory, disk limits
- `network` (NetworkConfig): Network isolation mode
- `mounts` (List[MountSpec]): Volume mounts
- `environment` (Dict[str, str]): Environment variables
- `security` (SecurityConfig): Security hardening
- `auto_start` (bool): Start immediately (default: True)

### Sandbox

Sandbox status and details:

- `id` (str): Unique sandbox ID (sb-xxxxxxxx)
- `name` (str): Sandbox name
- `state` (str): "creating", "running", "stopped", "error"
- `runtime` (str): Runtime type
- `image` (str): Image/template name
- `created` (datetime): Creation timestamp
- `started` (datetime): Last start timestamp
- `stopped` (datetime): Last stop timestamp
- `resources` (ResourceLimits): Configured limits
- `resource_usage` (ResourceUsage): Current usage stats
- `network` (str): Network mode
- `error` (str): Error message if state=error

### ExecResult

Command execution result:

- `exit_code` (int): Exit code (124 = timeout)
- `stdout` (str): Standard output
- `stderr` (str): Standard error
- `duration_ms` (int): Execution duration
- `timed_out` (bool): Whether command timed out

## Development

### Running Tests

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run tests
pytest

# Run tests with coverage
pytest --cov=agentic_sandbox --cov-report=html

# Run specific test file
pytest tests/test_client.py

# Run specific test
pytest tests/test_client.py::TestSandboxClientSync::test_create_sandbox_minimal_spec
```

### Code Quality

```bash
# Format code
black agentic_sandbox tests

# Lint
ruff check agentic_sandbox tests

# Type check
mypy agentic_sandbox
```

### Project Structure

```
sdk/python/
├── agentic_sandbox/       # Package source
│   ├── __init__.py        # Package exports
│   ├── client.py          # SandboxClient implementation
│   ├── models.py          # Pydantic models
│   └── exceptions.py      # Custom exceptions
├── tests/                 # Test suite
│   ├── conftest.py        # Pytest configuration
│   ├── test_client.py     # Client tests
│   └── test_models.py     # Model tests
├── pyproject.toml         # Package configuration
└── README.md              # This file
```

## Requirements

- Python 3.8+
- httpx >= 0.25.0
- pydantic >= 2.0.0

## License

MIT
