# Spike 005: Native AF_VSOCK + tonic Transport

**Status:** In progress
**Date:** 2026-05-31
**Issue:** agentic-sandbox#406

## Objective

De-risk ADR-023 risk R-1 by proving that the current Rust transport stack can
carry a tonic 0.12 bidirectional stream over native AF_VSOCK and expose the
peer CID through tonic's `Connected` extension.

## Current Result

`tokio-vsock 0.7.2` provides a `tonic012` feature that implements
`tonic::transport::server::Connected` for `VsockStream` and stores the stream
peer address in `VsockConnectInfo`. This spike wraps `VsockStream` in a small
adapter so tonic 0.12 can satisfy both the client-side hyper I/O bounds and the
server-side tokio I/O bounds while still surfacing the peer CID and port through
tonic connection extensions.

This repository now carries an opt-in integration spike:

```bash
cd management
AGENTIC_RUN_NATIVE_VSOCK_SPIKE=1 cargo test --test native_vsock_tonic_spike
```

The test starts a tonic server on `VMADDR_CID_LOCAL`, connects with a
`VsockStream`, opens a bidirectional stream, echoes a frame, and asserts that
the server observed a non-empty vsock peer address through tonic connection
extensions.

## Remaining Proof

The opt-in test proves the `tokio-vsock` + tonic shim on the host kernel. It
does not by itself satisfy the issue's full guest-to-host microVM acceptance
criterion. Before ADR-023 can move to Accepted, run the same pattern with a
real QEMU/Firecracker guest connecting to the management host and record:

- guest CID assigned by the VM runtime,
- host-side listener CID/port,
- tonic bidirectional stream success,
- server-observed peer CID matching the expected guest CID.

## Decision So Far

Keep the host-side AF_UNIX bridge as the default VM transport because it reuses
tonic's first-class Unix-domain-socket support. Treat native host-side
AF_VSOCK as an optional VM transport until the guest-to-host proof above is
green. mTLS-TCP remains the fallback when neither vsock path is available.

## Files

- `management/tests/native_vsock_tonic_spike.rs`
- `proto/vsock_spike.proto`
- `management/Cargo.toml`
