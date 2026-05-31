# References Register — Agent Transport Security

**Document Version**: 0.2 (Draft — external refs verified 2026-05-31)
**Date**: 2026-05-31
**Classification**: Internal
**Owner**: agentic-sandbox / roctinam
**Status**: Draft

---

## Purpose

Single citable source of truth for every reference used across the Agent
Transport Security suite. Downstream artifacts cite by `REF-ID`.

## Verification protocol (read first)

The deep-research web pass was re-run directly (operator approved
out-of-sandbox web access). **External references below were fetched/searched
on 2026-05-31 and carry real URLs.** Per the project `citation-policy` rule,
no URLs/DOIs were fabricated. Verification levels:

| Level | Meaning |
|-------|---------|
| **VERIFIED-INTERNAL** | A fact in this repo (file:line / committed artifact). |
| **VERIFIED-WEB** | Confirmed via web search/fetch this session; URL is real and on-point. |
| **STABLE-STANDARD (canonical URL)** | A named standard whose canonical URL is well-established; cited by that URL but **not individually fetched this session** — confirm before relying on a specific clause. |
| **PRACTITIONER** | Tooling behavior confirmed via vendor docs this session; GRADE noted; tool-version specifics still to pin against the version actually used. |

GRADE hedging is applied to PRACTITIONER-derived claims. Residual gate
(R-9) is now **mostly lifted**: the load-bearing tool/standard claims are
VERIFIED-WEB. Remaining work before Accept: pin exact crate versions against
`Cargo.lock` and run the two spikes (S-VSOCK native, S-RUSTLS-RELOAD).

---

## A. Internal references (VERIFIED-INTERNAL)

| REF-ID | Source | Establishes |
|--------|--------|-------------|
| INT-1 | `agent-rs/src/main.rs:1430` | Agent dials `http://` (plaintext h2c). |
| INT-2 | `agent-rs/src/main.rs:1772-1782` | `x-agent-id` + `x-agent-secret` gRPC metadata. |
| INT-3 | `management/src/grpc.rs:78-94` | `authenticate()` bearer-secret check. |
| INT-4 | `management/src/auth.rs` | `SecretStore`: SHA-256, const-time, rotation, **TOFU auto-register**. |
| INT-5 | `management/src/http/tls_listener.rs:1-210` | Operator-API mTLS listener (#238): `ServerTlsConfig`, `with_client_cert_verifier`, **x509 CN extraction** (note: CN; see STD-SVCID — gRPC path should extract URI-SAN instead). |
| INT-6 | `deploy/cloud-init/user-data.template:18`; `images/qemu/provision-vm.sh:741` | Plaintext `AGENT_SECRET` in cidata ISO. |
| INT-7 | `management/Cargo.toml:17,98-120` | `tonic[tls]`, `rustls 0.23`, `tokio-rustls`, `rustls-pemfile`, `x509-parser`, `rcgen 0.13` present. |
| INT-8 | `agent-rs/Cargo.toml:18` | Agent tonic lacks `tls` feature. |
| INT-9 | `proto/agent.proto` | PTY = opaque bytes on bidi stream; transport-agnostic. |
| ADR-004/005/015/018/020 | `@.aiwg/architecture/adr/` | Network isolation / egress gateway / external auth roadmap / A2A base / pty-ws binding. Scope boundaries (NG-1..3). |
| RULE-1..4 | `.claude/rules/{no-unauthenticated-encryption,sec-key-material-handling,token-security,no-adhoc-kdf,no-key-reuse-across-purposes,crypto-flag-verification}.md` | Project crypto/secret rules. |

## B. Standards & specifications

| REF-ID | Document | Level | URL | Used for |
|--------|----------|-------|-----|----------|
| STD-SPIFFE-ID | SPIFFE-ID standard (CNCF) | VERIFIED-WEB | https://github.com/spiffe/spiffe/blob/main/standards/SPIFFE-ID.md | `spiffe://` identity scheme. |
| STD-SVID | SPIFFE **X.509-SVID** standard | VERIFIED-WEB | https://spiffe.io/docs/latest/spiffe-specs/x509-svid/ · https://github.com/spiffe/spiffe/blob/main/standards/X509-SVID.md | **SVID MUST carry exactly one URI SAN = the SPIFFE ID; scheme must be `spiffe`; >1 URI SAN ⇒ reject.** Drives ADR-024. |
| STD-SVCID | **RFC 9525** — *Service Identity in TLS* (Nov 2023, **obsoletes RFC 6125**) | VERIFIED-WEB | https://www.rfc-editor.org/rfc/rfc9525.html | **Identity in SAN only; CN-ID no longer valid** ⇒ gRPC mTLS path extracts URI-SAN, not CN (cf. INT-5). |
| STD-6125 | RFC 6125 (obsoleted by 9525) | VERIFIED-WEB | https://datatracker.ietf.org/doc/html/rfc6125 | Historical context. |
| STD-TLS13 | RFC 8446 — TLS 1.3 | STABLE-STANDARD | https://www.rfc-editor.org/rfc/rfc8446.html | AEAD channel baseline. |
| STD-X509 | RFC 5280 — X.509 / CRL profile | STABLE-STANDARD | https://www.rfc-editor.org/rfc/rfc5280.html | Cert/SAN/chain semantics. |
| STD-PEM | RFC 7468 — PEM encodings | STABLE-STANDARD | https://www.rfc-editor.org/rfc/rfc7468.html | On-disk encoding. |
| STD-ZTA | NIST SP 800-207 — Zero Trust Architecture | STABLE-STANDARD | https://csrc.nist.gov/pubs/sp/800/207/final | "Authenticate every connection". |
| STD-MSVC | NIST SP 800-204A — *Building Secure Microservices-based Applications Using Service-Mesh Architecture* (Chandramouli & Butcher, 2020) | VERIFIED-WEB | https://csrc.nist.gov/pubs/sp/800/204/a/final | mTLS-between-services rationale. |
| STD-KEY | NIST SP 800-57 Pt 1 — Key Management | STABLE-STANDARD | https://csrc.nist.gov/pubs/sp/800/57/pt1/r5/final | Key/cert TTL guidance. |
| STD-VSOCK-FC | Firecracker vsock device docs | VERIFIED-WEB | https://github.com/firecracker-microvm/firecracker/blob/main/docs/vsock.md | **Host assigns CID per guest, no in-guest config; Firecracker bridges guest AF_VSOCK ↔ host AF_UNIX (uds_path), CONNECT preamble.** |
| STD-VSOCK-QEMU | QEMU VirtioVsock feature | VERIFIED-WEB | https://wiki.qemu.org/Features/VirtioVsock | Native AF_VSOCK host↔guest. |
| STD-PEERCRED | tonic `UdsConnectInfo` (peer cred) | VERIFIED-WEB | https://docs.rs/tonic/latest/tonic/transport/server/struct.UdsConnectInfo.html | `peer_cred: Option<UCred>` via `SO_PEERCRED` — **first-class**. |

## C. Tools & libraries (VERIFIED-WEB unless noted; pin versions before Accept)

| REF-ID | Tool/lib | Claim supported | URL | GRADE |
|--------|----------|-----------------|-----|-------|
| TOOL-RCGEN | `rcgen` | In-process CA (`IsCa::Ca(BasicConstraints::Unconstrained)` + `KeyCertSign`/`CrlSign`) + leaf with `SanType::Uri(..)` URI-SAN; `der`/`pem` out. | https://docs.rs/rcgen/latest/rcgen/enum.SanType.html · https://github.com/rustls/rcgen | HIGH |
| TOOL-RUSTLS | `rustls` `ResolvesServerCert` | Queried on every ClientHello → hot cert swap without dropping live conns. | https://docs.rs/rustls/latest/rustls/server/trait.ResolvesServerCert.html | HIGH |
| TOOL-RELOAD | `tls-hot-reload` / `rustls-hot-reload` | Off-the-shelf resolver + file-watch/SIGHUP zero-downtime reload. | https://github.com/sebadob/tls-hot-reload · https://lib.rs/crates/rustls-hot-reload | MODERATE |
| TOOL-TONIC-UDS | tonic UDS server transport | First-class UDS; `UdsConnectInfo` in request extensions. | https://docs.rs/tonic/latest/src/tonic/transport/server/unix.rs.html | HIGH |
| TOOL-TONIC-VSOCK | `tokio-vsock` (+ tonic `Connected` shim) | Async AF_VSOCK mirroring Tokio TCPListener/TCPStream; "writing agents for microvm". **No first-party tonic vsock binding — shim required (R-1, native case only).** | https://github.com/rust-vsock/tokio-vsock | MODERATE |
| TOOL-VHOST-VSOCK | `vhost-device-vsock` | Host-side via **UDS (`--uds-path`)** *or* native vsock (`--forward-cid`) — UDS path reuses TOOL-TONIC-UDS. | https://github.com/rust-vmm/vhost-device/blob/main/vhost-device-vsock/README.md | MODERATE |
| TOOL-STEPCA | smallstep `step-ca` | One-time bootstrap tokens (JWK provisioner), short-lived certs, daemon renew at **~2/3 (66%)** lifetime. | https://smallstep.com/docs/step-ca/provisioners/ · https://smallstep.com/docs/step-ca/renewal/ | HIGH |
| TOOL-SPIRE | SPIRE | SVID rotation at **~50% + jitter**; defaults `default_svid_ttl 1h`, `ca_ttl 24h`; short TTL ⇒ no CRL. | https://github.com/spiffe/spire/issues/4268 | MODERATE |
| TOOL-VAULT | Vault / **OpenBao** PKI + Agent | Dynamic short-lived certs; Vault Agent renews at **50%** of lease (72h→36h); ephemeral, in-memory. OpenBao = MPL-2.0 fork. | https://developer.hashicorp.com/vault/docs/secrets/pki | HIGH |
| TOOL-MKCERT | `mkcert` | `-install` writes a CA into the **system/browser trust store**; **dev/test only, never production** → **rejected** for machine identity. | https://github.com/FiloSottile/mkcert | HIGH |
| TOOL-GRPC-AUTH | gRPC auth guide | mTLS as channel credentials is the documented gRPC pattern. | https://grpc.io/docs/guides/auth/ | STABLE-STANDARD |

## D. Verified design facts (used across the suite)

- **F-1 (renewal cadence)**: machine-identity certs renew at **50–66% of
  lifetime** with short TTL and no CRL — SPIRE ~50%+jitter [TOOL-SPIRE],
  Vault Agent 50% [TOOL-VAULT], step-ca ~66% [TOOL-STEPCA]. → ADR-027.
- **F-2 (host-side socket for VMs)**: Firecracker & `vhost-device-vsock`
  expose a **host AF_UNIX** socket bridging to guest AF_VSOCK
  [STD-VSOCK-FC][TOOL-VHOST-VSOCK]; native AF_VSOCK is the alternative
  [STD-VSOCK-QEMU][TOOL-TONIC-VSOCK]. → narrows R-1; refines ADR-023/SAD.
- **F-3 (SVID SAN)**: exactly one URI SAN = SPIFFE ID; CN-ID invalid per
  RFC 9525 [STD-SVID][STD-SVCID]. → ADR-024; gRPC path extracts URI-SAN.
- **F-4 (peercred)**: tonic surfaces UDS peer creds first-class
  [STD-PEERCRED]. → ADR-023/024, UDS path low-risk.

## References

- @.aiwg/vision/agent-transport-security-vision.md
- @.aiwg/risks/agent-transport-security-risks.md
- @.aiwg/management/agent-transport-security-traceability.md
