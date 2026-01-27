"""Test suite for SandboxClient."""
import pytest
from datetime import datetime
from agentic_sandbox import (
    SandboxClient,
    SandboxSpec,
    Sandbox,
    ExecResult,
    ResourceLimits,
    NetworkConfig,
    NotFoundError,
    APIError,
    SandboxError,
)


class TestSandboxClientSync:
    """Test synchronous client operations."""

    @pytest.fixture
    def client(self):
        """Create test client."""
        return SandboxClient("http://localhost:8080")

    @pytest.fixture
    def mock_httpx(self, mocker):
        """Mock httpx client."""
        return mocker.patch("agentic_sandbox.client.httpx.Client")

    def test_create_client_with_base_url(self):
        """Test client creation with base URL."""
        client = SandboxClient("http://localhost:8080")
        assert client.base_url == "http://localhost:8080"

    def test_create_client_with_api_prefix(self):
        """Test client automatically adds /api prefix."""
        client = SandboxClient("http://localhost:8080")
        assert client._api_base == "http://localhost:8080/api/v1"

    def test_create_sandbox_minimal_spec(self, client, mock_httpx):
        """Test create_sandbox with minimal specification."""
        # Setup mock response
        mock_response = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "running",
            "runtime": "docker",
            "image": "alpine:3.18",
            "created": "2026-01-24T10:30:00Z",
            "started": "2026-01-24T10:30:02Z",
            "resources": {"cpu": 4, "memory": "8G", "pids_limit": 1024},
            "network": "isolated",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 201

        # Create sandbox
        spec = SandboxSpec(name="test-agent", runtime="docker", image="alpine:3.18")
        sandbox = client.create_sandbox(spec)

        # Verify request
        mock_client.post.assert_called_once()
        call_args = mock_client.post.call_args
        assert call_args[0][0] == "sandboxes"
        assert call_args[1]["json"]["name"] == "test-agent"
        assert call_args[1]["json"]["runtime"] == "docker"
        assert call_args[1]["json"]["image"] == "alpine:3.18"

        # Verify response
        assert isinstance(sandbox, Sandbox)
        assert sandbox.id == "sb-d8f3a9c4"
        assert sandbox.name == "test-agent"
        assert sandbox.state == "running"
        assert sandbox.runtime == "docker"

    def test_create_sandbox_full_spec(self, client, mock_httpx):
        """Test create_sandbox with full specification."""
        mock_response = {
            "id": "sb-12345678",
            "name": "full-agent",
            "state": "running",
            "runtime": "docker",
            "image": "agent-claude",
            "created": "2026-01-24T10:30:00Z",
            "started": "2026-01-24T10:30:02Z",
            "resources": {"cpu": 8, "memory": "16G", "pids_limit": 2048},
            "network": "gateway",
            "gateway_url": "http://gateway:8080",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 201

        # Create full spec
        spec = SandboxSpec(
            name="full-agent",
            runtime="docker",
            image="agent-claude",
            resources=ResourceLimits(cpu=8, memory="16G", pids_limit=2048),
            network=NetworkConfig(mode="gateway", gateway_url="http://gateway:8080"),
            environment={"AGENT_MODE": "autonomous"},
            auto_start=True,
        )
        sandbox = client.create_sandbox(spec)

        # Verify
        assert sandbox.id == "sb-12345678"
        assert sandbox.resources.cpu == 8
        assert sandbox.resources.memory == "16G"
        assert sandbox.network == "gateway"

    def test_get_sandbox_by_id(self, client, mock_httpx):
        """Test get_sandbox retrieves sandbox by ID."""
        mock_response = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "running",
            "runtime": "docker",
            "image": "alpine:3.18",
            "created": "2026-01-24T10:30:00Z",
            "resources": {"cpu": 4, "memory": "8G"},
            "network": "isolated",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.get.return_value.json.return_value = mock_response
        mock_client.get.return_value.status_code = 200

        sandbox = client.get_sandbox("sb-d8f3a9c4")

        mock_client.get.assert_called_once_with("sandboxes/sb-d8f3a9c4")
        assert sandbox.id == "sb-d8f3a9c4"
        assert sandbox.name == "test-agent"

    def test_get_sandbox_not_found(self, client, mock_httpx):
        """Test get_sandbox raises NotFoundError for missing sandbox."""
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.get.return_value.status_code = 404
        mock_client.get.return_value.json.return_value = {
            "error": "Not Found",
            "message": "Sandbox 'sb-missing' not found",
            "code": "SANDBOX_NOT_FOUND",
        }

        with pytest.raises(NotFoundError) as exc_info:
            client.get_sandbox("sb-missing")

        assert "sb-missing" in str(exc_info.value)

    def test_list_sandboxes_no_filters(self, client, mock_httpx):
        """Test list_sandboxes without filters."""
        mock_response = {
            "sandboxes": [
                {
                    "id": "sb-11111111",
                    "name": "agent-1",
                    "state": "running",
                    "runtime": "docker",
                    "image": "alpine",
                    "created": "2026-01-24T10:00:00Z",
                    "resources": {"cpu": 4, "memory": "8G"},
                    "network": "isolated",
                },
                {
                    "id": "sb-22222222",
                    "name": "agent-2",
                    "state": "stopped",
                    "runtime": "qemu",
                    "image": "ubuntu",
                    "created": "2026-01-24T09:00:00Z",
                    "resources": {"cpu": 8, "memory": "16G"},
                    "network": "gateway",
                },
            ],
            "total": 2,
            "limit": 100,
            "offset": 0,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.get.return_value.json.return_value = mock_response
        mock_client.get.return_value.status_code = 200

        result = client.list_sandboxes()

        mock_client.get.assert_called_once_with("sandboxes", params={})
        assert len(result["sandboxes"]) == 2
        assert result["total"] == 2
        assert isinstance(result["sandboxes"][0], Sandbox)
        assert result["sandboxes"][0].id == "sb-11111111"
        assert result["sandboxes"][1].id == "sb-22222222"

    def test_list_sandboxes_with_filters(self, client, mock_httpx):
        """Test list_sandboxes with runtime and state filters."""
        mock_response = {
            "sandboxes": [],
            "total": 0,
            "limit": 50,
            "offset": 0,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.get.return_value.json.return_value = mock_response
        mock_client.get.return_value.status_code = 200

        result = client.list_sandboxes(runtime="docker", state="running", limit=50)

        call_params = mock_client.get.call_args[1]["params"]
        assert call_params["runtime"] == "docker"
        assert call_params["state"] == "running"
        assert call_params["limit"] == 50

    def test_start_sandbox(self, client, mock_httpx):
        """Test start_sandbox starts a stopped sandbox."""
        mock_response = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "running",
            "runtime": "docker",
            "image": "alpine",
            "created": "2026-01-24T10:30:00Z",
            "started": "2026-01-24T11:00:00Z",
            "resources": {"cpu": 4, "memory": "8G"},
            "network": "isolated",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        sandbox = client.start_sandbox("sb-d8f3a9c4")

        mock_client.post.assert_called_once_with("sandboxes/sb-d8f3a9c4/start")
        assert sandbox.state == "running"
        assert sandbox.started is not None

    def test_stop_sandbox(self, client, mock_httpx):
        """Test stop_sandbox stops a running sandbox."""
        mock_response = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "stopped",
            "runtime": "docker",
            "image": "alpine",
            "created": "2026-01-24T10:30:00Z",
            "stopped": "2026-01-24T11:30:00Z",
            "resources": {"cpu": 4, "memory": "8G"},
            "network": "isolated",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        sandbox = client.stop_sandbox("sb-d8f3a9c4")

        mock_client.post.assert_called_once_with(
            "sandboxes/sb-d8f3a9c4/stop", params={"timeout": 10}
        )
        assert sandbox.state == "stopped"

    def test_stop_sandbox_with_timeout(self, client, mock_httpx):
        """Test stop_sandbox with custom timeout."""
        mock_response = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "stopped",
            "runtime": "docker",
            "image": "alpine",
            "created": "2026-01-24T10:30:00Z",
            "resources": {"cpu": 4, "memory": "8G"},
            "network": "isolated",
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        client.stop_sandbox("sb-d8f3a9c4", timeout=30)

        call_params = mock_client.post.call_args[1]["params"]
        assert call_params["timeout"] == 30

    def test_delete_sandbox(self, client, mock_httpx):
        """Test delete_sandbox removes a sandbox."""
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.delete.return_value.status_code = 204

        client.delete_sandbox("sb-d8f3a9c4")

        mock_client.delete.assert_called_once_with(
            "sandboxes/sb-d8f3a9c4", params={"force": False}
        )

    def test_delete_sandbox_force(self, client, mock_httpx):
        """Test delete_sandbox with force flag."""
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.delete.return_value.status_code = 204

        client.delete_sandbox("sb-d8f3a9c4", force=True)

        call_params = mock_client.delete.call_args[1]["params"]
        assert call_params["force"] is True

    def test_exec_command_simple(self, client, mock_httpx):
        """Test exec_command with simple command."""
        mock_response = {
            "exit_code": 0,
            "stdout": "hello world\n",
            "stderr": "",
            "duration_ms": 45,
            "timed_out": False,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        result = client.exec_command("sb-d8f3a9c4", ["echo", "hello world"])

        # Verify request
        call_args = mock_client.post.call_args
        assert call_args[0][0] == "sandboxes/sb-d8f3a9c4/exec"
        request_body = call_args[1]["json"]
        assert request_body["command"] == ["echo", "hello world"]

        # Verify response
        assert isinstance(result, ExecResult)
        assert result.exit_code == 0
        assert result.stdout == "hello world\n"
        assert result.stderr == ""
        assert result.duration_ms == 45
        assert result.timed_out is False

    def test_exec_command_with_options(self, client, mock_httpx):
        """Test exec_command with full options."""
        mock_response = {
            "exit_code": 0,
            "stdout": "test output\n",
            "stderr": "",
            "duration_ms": 100,
            "timed_out": False,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        result = client.exec_command(
            "sb-d8f3a9c4",
            ["python3", "test.py"],
            working_dir="/workspace",
            environment={"DEBUG": "1"},
            timeout=60,
            stdin="test input",
        )

        # Verify request body
        request_body = mock_client.post.call_args[1]["json"]
        assert request_body["command"] == ["python3", "test.py"]
        assert request_body["working_dir"] == "/workspace"
        assert request_body["environment"] == {"DEBUG": "1"}
        assert request_body["timeout"] == 60
        assert request_body["stdin"] == "test input"

    def test_exec_command_failure(self, client, mock_httpx):
        """Test exec_command handles non-zero exit codes."""
        mock_response = {
            "exit_code": 1,
            "stdout": "",
            "stderr": "command not found\n",
            "duration_ms": 10,
            "timed_out": False,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        result = client.exec_command("sb-d8f3a9c4", ["nonexistent"])

        assert result.exit_code == 1
        assert "command not found" in result.stderr

    def test_exec_command_timeout(self, client, mock_httpx):
        """Test exec_command handles timeout."""
        mock_response = {
            "exit_code": 124,
            "stdout": "partial output\n",
            "stderr": "Command timed out after 30s\n",
            "duration_ms": 30000,
            "timed_out": True,
        }
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.json.return_value = mock_response
        mock_client.post.return_value.status_code = 200

        result = client.exec_command("sb-d8f3a9c4", ["sleep", "100"], timeout=30)

        assert result.exit_code == 124
        assert result.timed_out is True
        assert result.duration_ms == 30000

    def test_api_error_handling(self, client, mock_httpx):
        """Test APIError is raised for server errors."""
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.status_code = 500
        mock_client.post.return_value.json.return_value = {
            "error": "Internal Server Error",
            "message": "Failed to communicate with Docker daemon",
            "code": "RUNTIME_ERROR",
        }

        with pytest.raises(APIError) as exc_info:
            spec = SandboxSpec(name="test", runtime="docker", image="alpine")
            client.create_sandbox(spec)

        assert "Failed to communicate with Docker daemon" in str(exc_info.value)
        assert exc_info.value.status_code == 500

    def test_validation_error_handling(self, client, mock_httpx):
        """Test SandboxError is raised for validation errors."""
        mock_client = mock_httpx.return_value.__enter__.return_value
        mock_client.post.return_value.status_code = 400
        mock_client.post.return_value.json.return_value = {
            "error": "Bad Request",
            "message": "Invalid sandbox specification",
            "code": "INVALID_SPEC",
        }

        with pytest.raises(SandboxError) as exc_info:
            spec = SandboxSpec(name="test-sandbox", runtime="docker", image="alpine")
            client.create_sandbox(spec)

        assert "Invalid sandbox specification" in str(exc_info.value)


class TestSandboxClientAsync:
    """Test asynchronous client operations."""

    @pytest.mark.asyncio
    async def test_async_create_sandbox(self):
        """Test async create_sandbox."""
        from unittest.mock import AsyncMock, MagicMock, patch
        
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "id": "sb-d8f3a9c4",
            "name": "test-agent",
            "state": "running",
            "runtime": "docker",
            "image": "alpine:3.18",
            "created": "2026-01-24T10:30:00Z",
            "resources": {"cpu": 4, "memory": "8G"},
            "network": "isolated",
        }
        mock_response.status_code = 201

        with patch("agentic_sandbox.client.httpx.AsyncClient") as mock_async_client_class:
            mock_client = AsyncMock()
            mock_client.post.return_value = mock_response
            mock_async_client_class.return_value.__aenter__.return_value = mock_client

            client = SandboxClient("http://localhost:8080")
            spec = SandboxSpec(name="test-agent", runtime="docker", image="alpine:3.18")
            sandbox = await client.async_create_sandbox(spec)

            assert sandbox.id == "sb-d8f3a9c4"
            assert sandbox.name == "test-agent"

    @pytest.mark.asyncio
    async def test_async_exec_command(self):
        """Test async exec_command."""
        from unittest.mock import AsyncMock, MagicMock, patch
        
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "exit_code": 0,
            "stdout": "hello\n",
            "stderr": "",
            "duration_ms": 45,
            "timed_out": False,
        }
        mock_response.status_code = 200

        with patch("agentic_sandbox.client.httpx.AsyncClient") as mock_async_client_class:
            mock_client = AsyncMock()
            mock_client.post.return_value = mock_response
            mock_async_client_class.return_value.__aenter__.return_value = mock_client

            client = SandboxClient("http://localhost:8080")
            result = await client.async_exec_command("sb-d8f3a9c4", ["echo", "hello"])

            assert result.exit_code == 0
            assert result.stdout == "hello\n"

