#!/usr/bin/env python3
"""
Auth Injection Gateway for Agentic Sandbox
Adds authentication tokens to requests in-flight
"""

import os
import sys
import logging
import time
from urllib.parse import urljoin, urlparse
from http.server import HTTPServer, BaseHTTPRequestHandler
import http.client
import ssl
import yaml

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s %(levelname)s %(message)s'
)
logger = logging.getLogger(__name__)


class Route:
    def __init__(self, prefix: str, upstream: str, auth: dict = None, strip_prefix: bool = False):
        self.prefix = prefix
        self.upstream = upstream
        self.auth = auth or {}
        self.strip_prefix = strip_prefix
        self.parsed_upstream = urlparse(upstream)


class GatewayHandler(BaseHTTPRequestHandler):
    routes: list[Route] = []
    default_action: str = "deny"

    def log_message(self, format, *args):
        # Override to use our logger
        pass

    def do_request(self):
        start = time.time()
        path = self.path

        # Find matching route
        route = None
        for r in self.routes:
            if path.startswith(r.prefix):
                route = r
                break

        if route is None:
            if self.default_action == "deny":
                logger.warning(f"DENIED {self.command} {path} (no matching route)")
                self.send_error(403, "Route not allowed")
                return

        # Build upstream path
        upstream_path = path
        if route.strip_prefix:
            upstream_path = path[len(route.prefix):]
            if not upstream_path:
                upstream_path = "/"

        # Create connection to upstream
        if route.parsed_upstream.scheme == "https":
            conn = http.client.HTTPSConnection(
                route.parsed_upstream.netloc,
                context=ssl.create_default_context()
            )
        else:
            conn = http.client.HTTPConnection(route.parsed_upstream.netloc)

        try:
            # Copy headers
            headers = {}
            for key, value in self.headers.items():
                if key.lower() not in ('host', 'connection'):
                    headers[key] = value

            # Inject auth token
            if route.auth.get('type') and route.auth['type'] != 'none':
                token_env = route.auth.get('token_env', '')
                token = os.environ.get(token_env, '')

                if token:
                    auth_header = route.auth.get('header', 'Authorization')
                    auth_type = route.auth['type']

                    if auth_type == 'bearer':
                        headers[auth_header] = f"Bearer {token}"
                    elif auth_type == 'token':
                        headers[auth_header] = token

            # Read request body
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else None

            # Make upstream request
            full_path = route.parsed_upstream.path.rstrip('/') + upstream_path
            conn.request(self.command, full_path, body=body, headers=headers)

            response = conn.getresponse()

            # Send response
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in ('transfer-encoding', 'connection'):
                    self.send_header(key, value)
            self.end_headers()

            # Stream response body
            self.wfile.write(response.read())

            elapsed = (time.time() - start) * 1000
            logger.info(f"{self.command} {path} -> {route.upstream} [{response.status}] ({elapsed:.1f}ms)")

        except Exception as e:
            logger.error(f"Upstream error: {e}")
            self.send_error(502, f"Upstream error: {e}")
        finally:
            conn.close()

    def do_GET(self):
        self.do_request()

    def do_POST(self):
        self.do_request()

    def do_PUT(self):
        self.do_request()

    def do_DELETE(self):
        self.do_request()

    def do_PATCH(self):
        self.do_request()

    def do_OPTIONS(self):
        self.do_request()


def load_config(path: str) -> dict:
    with open(path) as f:
        return yaml.safe_load(f)


def main():
    config_path = sys.argv[1] if len(sys.argv) > 1 else "gateway.yaml"

    config = load_config(config_path)

    # Parse routes
    routes = []
    for r in config.get('routes', []):
        routes.append(Route(
            prefix=r['prefix'],
            upstream=r['upstream'],
            auth=r.get('auth', {}),
            strip_prefix=r.get('strip_prefix', False)
        ))

    GatewayHandler.routes = routes
    GatewayHandler.default_action = config.get('default_action', 'deny')

    listen = config.get('listen', ':8080')
    port = int(listen.split(':')[-1])

    logger.info(f"Auth Gateway starting on port {port}")
    logger.info(f"Loaded {len(routes)} routes:")
    for r in routes:
        logger.info(f"  {r.prefix} -> {r.upstream}")

    server = HTTPServer(('0.0.0.0', port), GatewayHandler)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        logger.info("Shutting down")
        server.shutdown()


if __name__ == "__main__":
    main()
