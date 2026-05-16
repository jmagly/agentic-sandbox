# [HIGH] gRPC/HTTP/WS all bind plaintext TCP; bearer tokens sniffable on virbr0

**Labels**: `priority: high`, `area: security`, `area: network`, `type: incident`

## Threat model note

agentic-sandbox is deployed single-host, local-only by default. This issue is rated HIGH (not CRITICAL) under that model: the realistic attacker is a **process inside a compromised agent VM** with sniff access to `virbr0`, capturing replayable bearer credentials of other agents and operators. If the system is ever exposed beyond loopback / `virbr0`, this becomes BLOCK.

## Summary

All three management transports bind plaintext TCP:

- gRPC :8120 (`management/src/main.rs:60`)
- WebSocket :8121 (`management/src/ws/hub.rs:61`)
- HTTP admin :8122 (`management/src/http/server.rs:510`)

Code default in `management/src/config.rs:36` is `LISTEN_ADDR=0.0.0.0:8120` (WS and HTTP derive from the same IP). No `tonic::transport::ServerTlsConfig` / `axum_server::bind_rustls` call exists in the codebase. The `mtls_config` in `http/server.rs` is only consulted *if* an upstream proxy terminates mTLS — the server itself does not negotiate TLS.

Agents authenticate by sending their 32-byte hex `AGENT_SECRET` as a bearer credential. On the wire (including `virbr0` between guest VMs), this is plaintext and replayable.

## Impact

Any process inside any agent VM with the `CAP_NET_RAW` capability (root-equivalent in the guest, which is the default agent execution context per current Dockerfiles + cloud-init) can `tcpdump virbr0` and harvest:
- Other agents' bearer secrets on connect
- Operator tokens from dashboard / API requests
- Command payloads and PTY content in cleartext

Combined with H1 (WS-no-auth), an attacker doesn't even need to sniff to pivot — but with sniff capability, every credential transiting the host is at risk.

## Remediation

Pick one and ship:

**Path A — Native TLS via rustls** (recommended):
```rust
let tls = tonic::transport::ServerTlsConfig::new()
    .identity(Identity::from_pem(cert_pem, key_pem))
    .client_ca_root(Certificate::from_pem(client_ca_pem));
Server::builder().tls_config(tls)?.add_service(...).serve(addr).await?;
```
Generate per-host certs at provision time, mirroring the existing per-VM SSH-key infrastructure in `images/qemu/lib/secrets.sh`. Agents validate the host cert against a CA distributed via cloud-init.

**Path B — Loopback only + Unix socket** (cheaper, lower defense):
- Change `LISTEN_ADDR` default to `127.0.0.1:8120` in `management/src/config.rs:36`
- Have agents connect via a per-VM Unix socket exposed through virtio-vsock or a per-VM SSH tunnel rather than the network
- This removes `virbr0` sniff exposure entirely but requires per-VM socket plumbing

## Acceptance

- `tcpdump -i virbr0 -A port 8120` (or 8121/8122) returns no readable bearer tokens.
- Existing agent connect flow works against TLS-enabled mgmt server.
- README documents the deployment requirement (TLS certs or vsock).

## References

- RFC 8446 §1 (TLS 1.3 mandate for bearer-token transports)
- NIST SP 800-52 Rev. 2 §3.1 (TLS for management interfaces)
- OWASP API Security Top 10 2023 — API2
- Internal audit finding H2 (re-rated from B2 under local-only model)
- Companion finding H1 (WS auth — TLS without auth is incomplete; do both)
