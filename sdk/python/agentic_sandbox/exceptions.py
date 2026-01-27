"""Custom exceptions for agentic-sandbox SDK."""


class SandboxError(Exception):
    """Base exception for sandbox-related errors."""

    def __init__(self, message: str, code: str = None):
        """Initialize SandboxError.

        Args:
            message: Human-readable error message
            code: Machine-readable error code
        """
        self.message = message
        self.code = code
        super().__init__(message)


class NotFoundError(SandboxError):
    """Raised when a sandbox is not found."""

    pass


class APIError(SandboxError):
    """Raised when the API returns an error response."""

    def __init__(self, message: str, status_code: int, code: str = None):
        """Initialize APIError.

        Args:
            message: Human-readable error message
            status_code: HTTP status code
            code: Machine-readable error code
        """
        self.status_code = status_code
        super().__init__(message, code)
