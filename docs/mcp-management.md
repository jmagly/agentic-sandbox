# MCP management adapter

The management daemon exposes an optional, authenticated Model Context Protocol
endpoint at `POST /mcp` on the existing HTTP/admin listener. The prototype uses
Streamable HTTP with JSON responses and no server-initiated SSE stream; an
authenticated `GET /mcp` therefore returns `405 Method Not Allowed`.

The adapter implements the stable MCP `2025-11-25` lifecycle and accepts the
earlier Streamable HTTP protocol versions `2025-03-26` and `2025-06-18`. It is
stateless and does not mint `MCP-Session-Id` values.

## Enable the endpoint

Create `<SECRETS_DIR>/mcp-principals.toml`. The default Linux secrets directory
is `/var/lib/agentic-sandbox/secrets`; use
[`configs/mcp-principals.toml.example`](../configs/mcp-principals.toml.example)
as the starting point.

```toml
enabled = true
allowed_origins = ["https://admin.example.com"]

[[principals]]
client_id = "codex-local"
token_hash = "sha256:<64 lowercase hexadecimal characters>"
scopes = ["fleet.read", "session.read", "output.read"]
```

Generate the stored hash without putting the token in shell history:

```bash
python3 -c 'import getpass,hashlib; print("sha256:"+hashlib.sha256(getpass.getpass("MCP token: ").encode()).hexdigest())'
```

Set the configuration file mode to `0600` and restart `agentic-mgmt`. A missing
file or `enabled = false` leaves `/mcp` disabled. A present but malformed file
fails daemon startup rather than silently weakening authentication.

Native MCP clients normally omit `Origin`. If a request supplies `Origin`, it
must exactly match an `allowed_origins` entry. The endpoint authenticates its
own scoped bearer principals independently from `operator-tokens.toml`.

## Scopes and surfaces

| Scope | Tools | Resources |
|---|---|---|
| `fleet.read` | `list_sandboxes` | `sandbox://fleet`, `sandbox://instances/{instance_id}` |
| `session.read` | — | agent sessions, session screen, session transcript |
| `output.read` | `tail_output` | `sandbox://commands/{command_id}/output` |

`list_sandboxes` reuses the canonical `/api/v2/admin/instances` inventory,
including stopped and degraded runtime entries. `tail_output` returns bounded
replay from the existing per-command output buffer and preserves both readable
text and the original bytes as base64. Live follow and raw PTY tunneling remain
outside this prototype.

Tool and resource discovery is scope-filtered. Calling a known surface without
its scope returns HTTP `403` with JSON-RPC error code `-32003`.

## Protocol example

The Streamable HTTP request advertises both supported response media types:

```http
POST /mcp HTTP/1.1
Authorization: Bearer <token>
Content-Type: application/json
Accept: application/json, text/event-stream
MCP-Protocol-Version: 2025-11-25

{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
```

The supported request methods are:

- `initialize`, `ping`, and `notifications/initialized`
- `tools/list` and `tools/call`
- `resources/list`, `resources/templates/list`, and `resources/read`

The adapter returns one JSON-RPC response per POST. Notifications return
`202 Accepted` with no response body.
