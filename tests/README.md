# Test Suite Documentation

This directory contains test data and end-to-end (E2E) tests for Agentic Sandbox.

## Test Structure

```
tests/
├── e2e/                  # End-to-end tests (pytest)
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

End-to-end tests validate the complete system using pytest.

```bash
# Prerequisites
cd tests/e2e
pip install -r requirements.txt  # or: uv pip install -r requirements.txt

# Run E2E tests (requires built binaries)
./scripts/run-e2e-tests.sh

# Or run directly
pytest tests/e2e/ -v
```

Required binaries:
- `management/target/release/agentic-mgmt`
- `agent-rs/target/release/agent-client`
