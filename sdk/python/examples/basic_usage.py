#!/usr/bin/env python3
"""Basic usage examples for agentic-sandbox Python SDK."""

from agentic_sandbox import (
    SandboxClient,
    SandboxSpec,
    ResourceLimits,
    NetworkConfig,
    MountSpec,
)


def example_basic_sandbox():
    """Create a basic sandbox and execute commands."""
    print("=== Basic Sandbox Example ===")

    # Create client
    client = SandboxClient("http://localhost:8080")

    # Create minimal sandbox
    spec = SandboxSpec(
        name="example-agent",
        runtime="docker",
        image="alpine:3.18"
    )

    print(f"Creating sandbox '{spec.name}'...")
    sandbox = client.create_sandbox(spec)
    print(f"Created sandbox: {sandbox.id} (state: {sandbox.state})")

    # Execute a command
    print("\nExecuting command: echo 'Hello from sandbox'")
    result = client.exec_command(sandbox.id, ["echo", "Hello from sandbox"])
    print(f"Exit code: {result.exit_code}")
    print(f"Output: {result.stdout.strip()}")

    # List files
    print("\nListing root directory:")
    result = client.exec_command(sandbox.id, ["ls", "-la", "/"])
    print(result.stdout)

    # Clean up
    print(f"\nDeleting sandbox {sandbox.id}...")
    client.delete_sandbox(sandbox.id, force=True)
    print("Done!")


def example_advanced_sandbox():
    """Create an advanced sandbox with custom configuration."""
    print("\n=== Advanced Sandbox Example ===")

    client = SandboxClient("http://localhost:8080")

    # Create advanced spec
    spec = SandboxSpec(
        name="advanced-agent",
        runtime="docker",
        image="python:3.11-slim",
        resources=ResourceLimits(
            cpu=4,
            memory="4G",
            pids_limit=512
        ),
        network=NetworkConfig(
            mode="gateway",
            gateway_url="http://gateway:8080"
        ),
        mounts=[
            MountSpec(
                source="/tmp/workspace",
                target="/workspace",
                mode="rw"
            )
        ],
        environment={
            "PYTHONUNBUFFERED": "1",
            "DEBUG": "1"
        },
        auto_start=True
    )

    print(f"Creating advanced sandbox '{spec.name}'...")
    sandbox = client.create_sandbox(spec)
    print(f"Created: {sandbox.id}")
    print(f"Resources: {sandbox.resources.cpu} CPUs, {sandbox.resources.memory} memory")
    print(f"Network: {sandbox.network}")

    # Run Python code
    python_code = """
import sys
print(f"Python version: {sys.version}")
print(f"Running in sandbox!")
"""

    print("\nExecuting Python code...")
    result = client.exec_command(
        sandbox.id,
        ["python3", "-c", python_code],
        timeout=10
    )
    print(result.stdout)

    # Clean up
    client.delete_sandbox(sandbox.id, force=True)
    print("Done!")


def example_error_handling():
    """Demonstrate error handling."""
    print("\n=== Error Handling Example ===")

    from agentic_sandbox import NotFoundError, APIError

    client = SandboxClient("http://localhost:8080")

    # Try to get non-existent sandbox
    try:
        sandbox = client.get_sandbox("sb-notfound")
    except NotFoundError as e:
        print(f"Caught NotFoundError: {e.message}")

    # Try to execute in non-existent sandbox
    try:
        result = client.exec_command("sb-notfound", ["echo", "test"])
    except NotFoundError as e:
        print(f"Caught NotFoundError: {e.message}")

    print("Error handling works!")


async def example_async_usage():
    """Demonstrate async usage."""
    print("\n=== Async Usage Example ===")

    client = SandboxClient("http://localhost:8080")

    # Create sandbox
    spec = SandboxSpec(
        name="async-agent",
        runtime="docker",
        image="alpine:3.18"
    )

    print("Creating sandbox asynchronously...")
    sandbox = await client.async_create_sandbox(spec)
    print(f"Created: {sandbox.id}")

    # Execute command
    print("Executing command asynchronously...")
    result = await client.async_exec_command(
        sandbox.id,
        ["uname", "-a"]
    )
    print(f"Output: {result.stdout.strip()}")

    # Clean up
    await client.async_delete_sandbox(sandbox.id, force=True)
    print("Done!")


def example_lifecycle_management():
    """Demonstrate sandbox lifecycle management."""
    print("\n=== Lifecycle Management Example ===")

    client = SandboxClient("http://localhost:8080")

    # Create without auto-start
    spec = SandboxSpec(
        name="lifecycle-test",
        runtime="docker",
        image="alpine:3.18",
        auto_start=False
    )

    sandbox = client.create_sandbox(spec)
    print(f"Created sandbox in state: {sandbox.state}")

    # Start it
    print("Starting sandbox...")
    sandbox = client.start_sandbox(sandbox.id)
    print(f"State after start: {sandbox.state}")

    # Execute command
    result = client.exec_command(sandbox.id, ["hostname"])
    print(f"Hostname: {result.stdout.strip()}")

    # Stop it
    print("Stopping sandbox...")
    sandbox = client.stop_sandbox(sandbox.id, timeout=10)
    print(f"State after stop: {sandbox.state}")

    # Delete it
    print("Deleting sandbox...")
    client.delete_sandbox(sandbox.id)
    print("Done!")


def example_list_sandboxes():
    """List and filter sandboxes."""
    print("\n=== List Sandboxes Example ===")

    client = SandboxClient("http://localhost:8080")

    # List all sandboxes
    response = client.list_sandboxes()
    print(f"Total sandboxes: {response['total']}")

    for sandbox in response['sandboxes']:
        print(f"  {sandbox.name} ({sandbox.id}): {sandbox.state}")
        if sandbox.resource_usage:
            print(f"    CPU: {sandbox.resource_usage.cpu_percent:.1f}%")
            print(f"    Memory: {sandbox.resource_usage.memory_percent:.1f}%")

    # Filter by runtime and state
    print("\nRunning Docker sandboxes:")
    response = client.list_sandboxes(runtime="docker", state="running")
    print(f"Found {response['total']} running Docker sandboxes")


if __name__ == "__main__":
    import sys

    # Note: These examples require a running agentic-sandbox manager API
    print("agentic-sandbox Python SDK Examples")
    print("=====================================\n")
    print("NOTE: These examples require the agentic-sandbox API to be running")
    print("at http://localhost:8080. Start the API server first.\n")

    try:
        # Sync examples
        example_basic_sandbox()
        example_advanced_sandbox()
        example_lifecycle_management()
        example_list_sandboxes()
        example_error_handling()

        # Async example (if Python 3.7+)
        if sys.version_info >= (3, 7):
            import asyncio
            asyncio.run(example_async_usage())

    except Exception as e:
        print(f"\nError: {e}")
        print("\nMake sure the agentic-sandbox API is running at http://localhost:8080")
        sys.exit(1)
