# Test Suite Documentation

This directory contains test data and end-to-end (E2E) tests for Agentic Sandbox.

## Test Structure

```
tests/
├── e2e/                  # Legacy end-to-end tests (pytest)
│   ├── helpers/          # Shared test helpers
│   └── ...
├── testdata/             # Test fixtures and sample configurations
│   ├── sandbox-minimal.yaml
│   ├── sandbox-full.yaml
│   ├── sandbox-qemu.yaml
│   └── gateway-config.yaml
└── README.md             # This file
```

## Test Types

### Conformance Protocol

The standalone `roctinam/agentic-sandbox-conformance` harness is the published
wire-contract test. This repository's protocol for interpreting harness skips
and splitting stub, configured, live-agent, PTY, and restart testing tiers lives
in [`docs/testing/conformance-protocol.md`](../docs/testing/conformance-protocol.md).

### Rust Unit Tests

Run unit tests for Rust components:

```bash
cd management && cargo test
cd agent-rs && cargo test
cd cli && cargo test
```

### E2E Tests

End-to-end tests validate the complete system. The VM-backed delivery gate still
runs the legacy pytest suite while the Rust integration suite is being ported.
The current Rust slice lives under `management/tests/e2e_*` and covers the
management HTTP health endpoint, WebSocket ping/pong, and agent
registration/deregistration with isolated management and agent processes.

```bash
# Run the Rust E2E migration slice directly
cd management
AGENTIC_RUN_RUST_E2E=1 \
AGENTIC_AGENT_BIN=../agent-rs/target/release/agent-client \
  cargo test --test e2e_server_health --test e2e_agent_registration

# Run the full E2E gate (Rust slice, VM substrate prep, then pytest)
./scripts/run-e2e-tests.sh

# Or run the legacy pytest suite directly
pip install -r tests/e2e/requirements.txt  # or: uv pip install -r tests/e2e/requirements.txt
pytest tests/e2e/ -v
```

Required binaries:
- `management/target/release/agentic-mgmt`
- `agent-rs/target/release/agent-client`

### Browser Self-Tests

The dashboard includes manual browser self-tests under `management/ui/test/`.
Serve the management UI, then open the test page in a browser. For high-redraw
PTY renderer/reconnect coverage, use:

```text
http://127.0.0.1:8122/test/tui-redraw-stress.test.html
```

The harness uses the bundled xterm.js and a fake `pty-ws.v1` WebSocket, so it
does not require Docker, VMs, provider credentials, or a live agent session.
