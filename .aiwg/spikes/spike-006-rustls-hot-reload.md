# Spike 006: Rustls Cert Hot Reload

**Status:** Verified
**Date:** 2026-05-31
**Issue:** agentic-sandbox#407

## Objective

De-risk ADR-027 risk R-2 by proving the pinned Rust TLS stack can rotate the
server certificate in process without dropping an already-established
long-lived stream.

## Result

`rustls 0.23` supports the intended hot-reload shape through
`rustls::server::ResolvesServerCert`. This repository now carries an
integration spike that backs a resolver with `ArcSwap<CertifiedKey>`, then
proves:

- a long-lived PTY-like TLS stream continues to send and receive bytes after
  the resolver swaps to a renewed certificate,
- the live stream remains bound to the certificate negotiated at its original
  handshake,
- a new TLS handshake after the swap observes the rotated certificate.

Run it with:

```bash
cd management
cargo test --test rustls_hot_reload_spike -- --nocapture
```

## Decision

Use an in-house `ArcSwap<CertifiedKey>` resolver for the production transport
core rather than adopting `tls-hot-reload` or `rustls-hot-reload` as a runtime
dependency. The behavior needed for ADR-027 is small: parse/write cert material
through the existing renewal path, validate it, then atomically swap the
`CertifiedKey`. Off-the-shelf file-watch crates remain useful references, but
they add watcher policy and reload timing that should stay outside the TLS
accept loop.

## Integration Notes

The spike intentionally models the PTY requirement as a long-lived
bidirectional TLS byte stream. The later Phase 1/Phase 4 transport work should
reuse this resolver shape in the management listener and attach it to the real
PTY/WebSocket path, with telemetry for reload success/failure.

## Files

- `management/tests/rustls_hot_reload_spike.rs`
- `management/Cargo.toml`
