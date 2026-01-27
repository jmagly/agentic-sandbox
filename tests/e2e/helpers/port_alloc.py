"""Dynamic port allocation for E2E tests."""

import socket
from dataclasses import dataclass


BASE_PORT = 18120


@dataclass(frozen=True)
class Ports:
    """Port assignments for a test session."""
    grpc: int
    ws: int
    http: int


class PortAllocator:
    """Allocates free ports for test sessions."""

    @staticmethod
    def is_port_free(port: int) -> bool:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind(("127.0.0.1", port))
                return True
            except OSError:
                return False

    @classmethod
    def allocate(cls) -> Ports:
        """Find three consecutive free ports starting from BASE_PORT."""
        port = BASE_PORT
        max_attempts = 100
        for _ in range(max_attempts):
            if all(cls.is_port_free(port + i) for i in range(3)):
                return Ports(grpc=port, ws=port + 1, http=port + 2)
            port += 3
        raise RuntimeError(
            f"Could not find 3 consecutive free ports after {max_attempts} attempts"
        )
