# Security Audit — agentic-sandbox

**Date**: 2026-05-15
**Auditors**: applied-cryptographer + secure-bootstrap-reviewer (subagents), security/devops/container review (inline grep-based)
**Scope**: Rust mgmt (gRPC :8120, WS :8121, HTTP :8122) + agent-rs + management UI + VM provisioning chain + Dockerfiles + Gitea CI workflows + docker-compose

## Threat model

agentic-sandbox is **single-host, local-only by default**. Mgmt server, libvirt, and agent VMs live together on one host with no LAN/WAN exposure expected. Findings about transports are rated against this model: the realistic attacker is **a compromised agent VM on `virbr0`** pivoting to sibling agents, not a remote network attacker. Code default in `config.rs:36` is still `0.0.0.0:8120`, so any future change in deployment posture re-elevates transport findings to BLOCK.

## Top-line verdict

**3 BLOCK / 11 HIGH / 5 MEDIUM / 6 LOW** (re-rated under local-only model; H11 added 2026-05-15 covering npm `install -g` propagation surface).

BLOCKERS (3): all local-host or local-host-adjacent — no ISO/qcow2 verification, cloud-init plaintext-secret leak via world-readable ISO, dev compose mounts docker.sock.

HIGH (+2 vs original rating): WS-no-auth and plaintext transports moved here from BLOCK — still serious (VM-to-VM lateral movement, defense-in-depth gone) but not WAN-exploitable in default posture.

Design intent (per-VM ephemeral secrets, hash-on-host plaintext-on-VM, KVM hardware isolation) is sound. Execution gaps cluster around **VM-to-VM lateral movement** (no WS auth, no TLS), **bootstrap chain provenance** (no ISO signature verification, no qcow2 hash pinning), **host-local secret disclosure** (cloud-init plaintext ISO, world-readable secrets dir), and **CI/Dockerfile supply-chain pinning** (every workflow + Dockerfile uses floating tags).

## Findings index

### BLOCKERS (file as `priority: critical`)

| ID | Title | Source | File |
|----|-------|--------|------|
| B1 | Base ISO + qcow2 backing image have no signature/hash verification | secure-bootstrap | `images/qemu/build-base-image.sh:77-91` |
| B2 | Cloud-init seed ISO contains plaintext `AGENT_SECRET` in world-readable host path | secure-bootstrap | `images/qemu/provision-vm.sh:545-547` |
| B3 | docker-compose.dev.yaml bind-mounts host `/var/run/docker.sock` into mgmt container | inline | `docker-compose.dev.yaml:21` |

### HIGH (file as `priority: high`)

| ID | Title | Source | File |
|----|-------|--------|------|
| H1 | WS server :8121 accepts unauthenticated commands → cross-VM RCE | inline | `management/src/ws/{hub,connection}.rs` |
| H2 | All transports plaintext, code default `0.0.0.0`; VM-to-VM sniff on `virbr0` reads bearer tokens | inline + applied-crypto | `management/src/config.rs:36` |
| H3 | Non-constant-time SHA-256 hash compare in token verify | applied-cryptographer | `management/src/auth.rs:95` |
| H4 | virtiofs RW mounts missing `nodev,nosuid,noexec` | secure-bootstrap | `images/qemu/cloud-init/ubuntu.sh:43-50` |
| H5 | `$SECRETS_DIR` is 0755, token files 0644 — leaks agent identities | secure-bootstrap | `images/qemu/lib/secrets.sh:25-28` |
| H6 | `HEALTH_TOKEN_PLACEHOLDER` literal in agentic-dev profile → unauth health endpoint | secure-bootstrap | `images/qemu/cloud-init/ubuntu.sh:238-242` |
| H7 | Secret-rotation race in `generate_agent_secret()` non-atomic write | secure-bootstrap | `images/qemu/lib/secrets.sh:38-67` |
| H8 | All Gitea workflow `uses:` are tag-pinned, not SHA-pinned (17+ violations) | inline | `.gitea/workflows/*.yml` |
| H9 | All Dockerfile `FROM` lines use floating tags, no digest pin (10 files) | inline | `Dockerfile.dev`, `deploy/docker/*`, `images/**` |
| H10 | `actions/upload-artifact@v3` is DEPRECATED / EOL | inline | `.gitea/workflows/ci.yaml:217`, `conformance.yml:110` |
| H11 | All `npm install -g` invocations unpinned — Mini Shai-Hulud propagation surface | follow-up | `.gitea/workflows/schema-lint.yml:45`, `images/container/Dockerfile.codex:17`, `images/agent/claude/Dockerfile:29`, `images/qemu/cloud-init/ubuntu.sh:817`, `images/qemu/profiles/agentic-dev-cloud-init.yaml:57`, `images/qemu/loadouts/generate-from-manifest.sh:343,475` |

### MEDIUM (file as `priority: medium`)

| ID | Title | Source | File |
|----|-------|--------|------|
| M1 | No `<seclabel>`/sVirt enforcement in generated libvirt domain XML | inline | `images/qemu/provision-vm.sh` |
| M2 | Management UI has 25+ `innerHTML` sinks; no Content-Security-Policy meta | inline | `management/ui/app.js`, `index.html` |
| M3 | `deploy/docker/Dockerfile.management,.agent-rust,.agent-python` define no `USER` → run as root | inline | listed Dockerfiles |
| M4 | `images/agent/claude/Dockerfile` uses `FROM agentic-sandbox-base:latest` | inline | `images/agent/claude/Dockerfile:4` |
| M5 | No operator provenance recorded for loadout manifests (no SHA in vm-info.json) | secure-bootstrap | `images/qemu/loadouts/resolve-manifest.sh` |

### LOW / informational

L1 No crash-path revocation hook for VM secrets (`destroy-vm.sh` only on clean shutdown)
L2 No TPM/Secure Boot (documented gap, acceptable for current threat model)
L3 `runtimes/docker/docker-compose.yml`: `read_only: false` + caps NET_BIND_SERVICE/CHOWN/SETUID/SETGID — review for minimization
L4 Mixed Rust toolchain versions across Dockerfiles (1.76 vs 1.88)
L5 No `cargo audit` wired into CI
L6 Many `curl | sh` installers in legacy provision-vm.sh backups + cloud-init profiles (executes inside VM, lower severity but still pin-less)

## Gap analysis vs AIWG rules

| AIWG Rule | Status in repo | Worst violation |
|-----------|---------------|-----------------|
| `ci-action-pinning` | **FAIL** | 17+ tag-pinned `uses:` across 7 workflows |
| `token-security` | PARTIAL | Hash store correct; transport leaks the plaintext |
| `no-unauthenticated-encryption` | N/A (no app-level encryption) | — |
| `crypto-flag-verification` | PASS | Only `openssl rand`, no `enc`/`gpg` |
| `no-adhoc-kdf` | PASS | SHA-256(256-bit CSPRNG) is correct primitive for verifier storage |
| `no-key-reuse-across-purposes` | PASS | agent-secret and health-token are independent CSPRNG outputs |
| `dev-secret-hygiene` | PARTIAL | Secrets not in env-vars/build-args, but cloud-init seed ISO leaks |
| `dev-idempotent-builds` | **FAIL** | No FROM digest pinning; mixed Rust versions |
| `sys-immutable-base` | PARTIAL | `chattr +i` on base qcow2 is good; no SHA backstop |
| `evidence-integrity` (if applied to provisioning records) | FAIL | No vm-info.json provenance hashes |

## Recommended remediation sequencing

**Week 1 (BLOCKERS, ship-blocking)**
1. Add bearer auth to WebSocket hub accept path (B1) — 1 day
2. Flip `LISTEN_ADDR` default to `127.0.0.1` and document TLS-proxy requirement (B2 quick win) — 1 hour
3. `chmod 700` `/var/lib/agentic-sandbox/vms/*` and `chmod 600` cloud-init.iso (B4 hotfix) — 30 min
4. Remove docker.sock bind-mount from dev compose; use Docker API over TCP with TLS or a rootless socket proxy (B5) — 1 day

**Week 2 (B-blockers proper fixes + supply-chain hygiene)**
5. Native TLS on all three transports via rustls (B2 full) — 3-5 days
6. Switch to SSH-push secret delivery, detach cloud-init.iso post-boot (B4 full) — 2 days
7. ISO signature verification + qcow2 hash manifest (B3) — 1 week

**Week 3 (HIGH)**
8. SHA-pin all `uses:` in workflows + replace `upload-artifact@v3` (H6, H8) — 1 day
9. Digest-pin Dockerfile FROMs + add USER directives + bump Rust to single supported toolchain (H7, M3, M4, L4) — 1 day
10. Constant-time hash compare via `subtle` crate (H1) — 30 min
11. virtiofs `nodev,nosuid,noexec` (H2) — 30 min
12. Tighten `SECRETS_DIR` perms (H3), substitute HEALTH_TOKEN placeholder (H4), atomic-swap on hash file (H5) — 2 hours total

**Week 4 (MEDIUM)**
13. Add `<seclabel type='dynamic' model='apparmor'/>` to generated domain XML; verify sVirt enabled (M1)
14. Add CSP `<meta>` to index.html + audit innerHTML sinks for user-controlled data (M2)
15. Record loadout manifest SHA in vm-info.json (M5)
16. Wire `cargo audit` into CI (L5)

## Issue files

Pre-formatted issue bodies for the top 10 items are in `./issues/`. Each is sized for direct paste into Gitea (`roctinam/agentic-sandbox/issues/new`). Labels follow the `ops-issue-tracking` rule's standard taxonomy.
