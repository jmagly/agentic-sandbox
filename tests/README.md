# Test Suite Documentation

This directory contains test data and end-to-end (E2E) tests for Agentic Sandbox.

## Test Structure

```
tests/
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

End-to-end tests validate the complete system. The delivery gate is Rust-only:
local process coverage lives under `management/tests/e2e_*`, and the VM-backed
resource-limit coverage runs from `management/tests/e2e_resource_limits.rs`.
The retired pytest harness was removed after the default-running Python surface
reached Rust parity. Its opt-in direct SSH destructive stress/recovery probes
were intentionally not carried forward because they run outside the agent
service cgroup and were not part of the default CI gate.

The current Rust suite covers the management HTTP health endpoint, WebSocket
handshake/idle connection, WebSocket ping/pong, agent
registration/deregistration, WebSocket agent-list metadata, command dispatch
output streaming, missing-agent errors, missing command dispatch, non-zero exit
dispatch continuity, stdin routing, concurrent-agent routing, subscription
filtering, unsubscribe behavior with isolated management and agent processes,
and VM-backed resource-limit coverage for agent service cgroups, memory
pressure containment, agentshare small writes and quota overruns, and
dispatch-backed PID, file descriptor, and I/O throughput stress.

```bash
# Run the Rust E2E migration slice directly
cd management
AGENTIC_RUN_RUST_E2E=1 \
AGENTIC_AGENT_BIN=../agent-rs/target/release/agent-client \
  cargo test \
    --test e2e_server_health \
    --test e2e_agent_registration \
    --test e2e_command_dispatch \
    --test e2e_concurrent_agents

# Run the VM-backed Rust resource-limit slice directly after preparing TEST_VM
cd management
AGENTIC_RUN_RUST_VM_E2E=1 cargo test --test e2e_resource_limits

# Run the full E2E gate (local Rust suite, then VM-backed Rust suite)
./scripts/run-e2e-tests.sh
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
