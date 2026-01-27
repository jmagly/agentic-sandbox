"""HTTP client for agentic-sandbox API."""
from typing import Dict, List, Optional, Union
import httpx

from .models import Sandbox, SandboxSpec, ExecResult
from .exceptions import SandboxError, NotFoundError, APIError


class SandboxClient:
    """Client for interacting with the agentic-sandbox API.

    Supports both synchronous and asynchronous operations.

    Example:
        >>> client = SandboxClient("http://localhost:8080")
        >>> spec = SandboxSpec(name="my-agent", runtime="docker", image="alpine:3.18")
        >>> sandbox = client.create_sandbox(spec)
        >>> result = client.exec_command(sandbox.id, ["echo", "hello"])
        >>> print(result.stdout)

    Args:
        base_url: Base URL of the sandbox manager API
        async_mode: If True, use async client (default: False)
        timeout: Request timeout in seconds (default: 30)
    """

    def __init__(
        self,
        base_url: str,
        async_mode: bool = False,
        timeout: float = 30.0,
    ):
        """Initialize the sandbox client."""
        self.base_url = base_url.rstrip("/")
        self._api_base = f"{self.base_url}/api/v1"
        self._async_mode = async_mode
        self._timeout = timeout

    def _handle_error(self, response: httpx.Response) -> None:
        """Handle error responses from the API.

        Args:
            response: HTTP response

        Raises:
            NotFoundError: If resource not found (404)
            APIError: For other error responses
        """
        try:
            error_data = response.json()
            message = error_data.get("message", "Unknown error")
            code = error_data.get("code")
        except Exception:
            message = f"HTTP {response.status_code}: {response.text}"
            code = None

        if response.status_code == 404:
            raise NotFoundError(message, code)
        elif response.status_code >= 400:
            raise APIError(message, response.status_code, code)

    def create_sandbox(self, spec: SandboxSpec) -> Sandbox:
        """Create a new sandbox.

        Args:
            spec: Sandbox specification

        Returns:
            Created sandbox details

        Raises:
            SandboxError: If sandbox creation fails
            APIError: If API request fails
        """
        if self._async_mode:
            raise RuntimeError("Use async create_sandbox for async mode")

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.post(
                "sandboxes",
                json=spec.model_dump(exclude_none=True),
            )

            if response.status_code != 201:
                self._handle_error(response)

            return Sandbox(**response.json())

    async def async_create_sandbox(self, spec: SandboxSpec) -> Sandbox:
        """Create a new sandbox (async).

        Args:
            spec: Sandbox specification

        Returns:
            Created sandbox details
        """
        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.post(
                "sandboxes",
                json=spec.model_dump(exclude_none=True),
            )

            if response.status_code != 201:
                self._handle_error(response)

            return Sandbox(**response.json())

    def get_sandbox(self, sandbox_id: str) -> Sandbox:
        """Get sandbox details.

        Args:
            sandbox_id: Sandbox ID or name

        Returns:
            Sandbox details

        Raises:
            NotFoundError: If sandbox not found
        """
        if self._async_mode:
            raise RuntimeError("Use async get_sandbox for async mode")

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.get(f"sandboxes/{sandbox_id}")

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    async def async_get_sandbox(self, sandbox_id: str) -> Sandbox:
        """Get sandbox details (async).

        Args:
            sandbox_id: Sandbox ID or name

        Returns:
            Sandbox details
        """
        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.get(f"sandboxes/{sandbox_id}")

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    def list_sandboxes(
        self,
        runtime: Optional[str] = None,
        state: Optional[str] = None,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
    ) -> Dict[str, Union[List[Sandbox], int]]:
        """List all sandboxes with optional filters.

        Args:
            runtime: Filter by runtime type (docker, qemu)
            state: Filter by state (creating, running, stopped, error)
            limit: Maximum number of results
            offset: Number of results to skip

        Returns:
            Dict with 'sandboxes' list, 'total', 'limit', 'offset'
        """
        if self._async_mode:
            raise RuntimeError("Use async list_sandboxes for async mode")

        params = {}
        if runtime:
            params["runtime"] = runtime
        if state:
            params["state"] = state
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.get("sandboxes", params=params)

            if response.status_code != 200:
                self._handle_error(response)

            data = response.json()
            return {
                "sandboxes": [Sandbox(**s) for s in data["sandboxes"]],
                "total": data["total"],
                "limit": data["limit"],
                "offset": data["offset"],
            }

    async def async_list_sandboxes(
        self,
        runtime: Optional[str] = None,
        state: Optional[str] = None,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
    ) -> Dict[str, Union[List[Sandbox], int]]:
        """List all sandboxes with optional filters (async).

        Args:
            runtime: Filter by runtime type (docker, qemu)
            state: Filter by state (creating, running, stopped, error)
            limit: Maximum number of results
            offset: Number of results to skip

        Returns:
            Dict with 'sandboxes' list, 'total', 'limit', 'offset'
        """
        params = {}
        if runtime:
            params["runtime"] = runtime
        if state:
            params["state"] = state
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset

        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.get("sandboxes", params=params)

            if response.status_code != 200:
                self._handle_error(response)

            data = response.json()
            return {
                "sandboxes": [Sandbox(**s) for s in data["sandboxes"]],
                "total": data["total"],
                "limit": data["limit"],
                "offset": data["offset"],
            }

    def start_sandbox(self, sandbox_id: str) -> Sandbox:
        """Start a stopped sandbox.

        Args:
            sandbox_id: Sandbox ID or name

        Returns:
            Updated sandbox details
        """
        if self._async_mode:
            raise RuntimeError("Use async start_sandbox for async mode")

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.post(f"sandboxes/{sandbox_id}/start")

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    async def async_start_sandbox(self, sandbox_id: str) -> Sandbox:
        """Start a stopped sandbox (async).

        Args:
            sandbox_id: Sandbox ID or name

        Returns:
            Updated sandbox details
        """
        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.post(f"sandboxes/{sandbox_id}/start")

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    def stop_sandbox(self, sandbox_id: str, timeout: int = 10) -> Sandbox:
        """Stop a running sandbox.

        Args:
            sandbox_id: Sandbox ID or name
            timeout: Timeout in seconds before force kill

        Returns:
            Updated sandbox details
        """
        if self._async_mode:
            raise RuntimeError("Use async stop_sandbox for async mode")

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.post(
                f"sandboxes/{sandbox_id}/stop",
                params={"timeout": timeout},
            )

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    async def async_stop_sandbox(self, sandbox_id: str, timeout: int = 10) -> Sandbox:
        """Stop a running sandbox (async).

        Args:
            sandbox_id: Sandbox ID or name
            timeout: Timeout in seconds before force kill

        Returns:
            Updated sandbox details
        """
        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.post(
                f"sandboxes/{sandbox_id}/stop",
                params={"timeout": timeout},
            )

            if response.status_code != 200:
                self._handle_error(response)

            return Sandbox(**response.json())

    def delete_sandbox(self, sandbox_id: str, force: bool = False) -> None:
        """Delete a sandbox.

        Args:
            sandbox_id: Sandbox ID or name
            force: Force deletion even if running
        """
        if self._async_mode:
            raise RuntimeError("Use async delete_sandbox for async mode")

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.delete(
                f"sandboxes/{sandbox_id}",
                params={"force": force},
            )

            if response.status_code != 204:
                self._handle_error(response)

    async def async_delete_sandbox(self, sandbox_id: str, force: bool = False) -> None:
        """Delete a sandbox (async).

        Args:
            sandbox_id: Sandbox ID or name
            force: Force deletion even if running
        """
        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.delete(
                f"sandboxes/{sandbox_id}",
                params={"force": force},
            )

            if response.status_code != 204:
                self._handle_error(response)

    def exec_command(
        self,
        sandbox_id: str,
        command: List[str],
        working_dir: Optional[str] = None,
        environment: Optional[Dict[str, str]] = None,
        timeout: Optional[int] = None,
        stdin: Optional[str] = None,
        user: Optional[str] = None,
        tty: bool = False,
    ) -> ExecResult:
        """Execute a command in a sandbox.

        Args:
            sandbox_id: Sandbox ID or name
            command: Command and arguments to execute
            working_dir: Working directory for command
            environment: Additional environment variables
            timeout: Timeout in seconds (0 = no timeout)
            stdin: Data to send to stdin
            user: User to run command as
            tty: Allocate a pseudo-TTY

        Returns:
            Command execution result
        """
        if self._async_mode:
            raise RuntimeError("Use async exec_command for async mode")

        request_body = {"command": command}
        if working_dir:
            request_body["working_dir"] = working_dir
        if environment:
            request_body["environment"] = environment
        if timeout is not None:
            request_body["timeout"] = timeout
        if stdin:
            request_body["stdin"] = stdin
        if user:
            request_body["user"] = user
        if tty:
            request_body["tty"] = tty

        with httpx.Client(base_url=self._api_base, timeout=self._timeout) as client:
            response = client.post(
                f"sandboxes/{sandbox_id}/exec",
                json=request_body,
            )

            if response.status_code != 200:
                self._handle_error(response)

            return ExecResult(**response.json())

    async def async_exec_command(
        self,
        sandbox_id: str,
        command: List[str],
        working_dir: Optional[str] = None,
        environment: Optional[Dict[str, str]] = None,
        timeout: Optional[int] = None,
        stdin: Optional[str] = None,
        user: Optional[str] = None,
        tty: bool = False,
    ) -> ExecResult:
        """Execute a command in a sandbox (async).

        Args:
            sandbox_id: Sandbox ID or name
            command: Command and arguments to execute
            working_dir: Working directory for command
            environment: Additional environment variables
            timeout: Timeout in seconds (0 = no timeout)
            stdin: Data to send to stdin
            user: User to run command as
            tty: Allocate a pseudo-TTY

        Returns:
            Command execution result
        """
        request_body = {"command": command}
        if working_dir:
            request_body["working_dir"] = working_dir
        if environment:
            request_body["environment"] = environment
        if timeout is not None:
            request_body["timeout"] = timeout
        if stdin:
            request_body["stdin"] = stdin
        if user:
            request_body["user"] = user
        if tty:
            request_body["tty"] = tty

        async with httpx.AsyncClient(
            base_url=self._api_base, timeout=self._timeout
        ) as client:
            response = await client.post(
                f"sandboxes/{sandbox_id}/exec",
                json=request_body,
            )

            if response.status_code != 200:
                self._handle_error(response)

            return ExecResult(**response.json())

    # Provide unified interface that dispatches to sync/async
    def __getattr__(self, name: str):
        """Dispatch to async methods if in async mode."""
        if self._async_mode and name in [
            "create_sandbox",
            "get_sandbox",
            "list_sandboxes",
            "start_sandbox",
            "stop_sandbox",
            "delete_sandbox",
            "exec_command",
        ]:
            return getattr(self, f"async_{name}")
        raise AttributeError(f"'{type(self).__name__}' object has no attribute '{name}'")
