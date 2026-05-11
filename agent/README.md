# Legacy — superseded by `agent-rs/`

**Deprecated:** 2026-01-26

This directory contains the original Python agent client implementation. It has been superseded by the Rust agent client at `../agent-rs/`, which is the supported implementation deployed inside VMs.

The Python code here is retained for historical reference only. Do not modify or extend it. New agent work belongs in `../agent-rs/`.

## Replacement

- Rust agent client: `../agent-rs/`
- Deployment scripts: `../scripts/deploy-agent.sh`, `../scripts/dev-deploy-all.sh`

## Contents

- `grpc_client.py` — original gRPC client (Python)
- `proto/` — generated Python protobuf bindings (regenerate from `../proto/agent.proto` if needed)
- `systemd/agent-client.service` — historical systemd unit; current units live in `../agent-rs/` deployment scripts
