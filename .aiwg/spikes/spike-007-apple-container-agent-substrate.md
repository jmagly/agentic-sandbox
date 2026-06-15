# Spike 007: Apple `container` as macOS VM-Backed Agent Substrate

**Status:** execution pending on Apple Silicon macOS 26 host  
**Issues:** #438, #488, #489  
**Created:** 2026-06-15  

## Purpose

Validate whether Apple's `container` project can satisfy the minimum
agentic-sandbox runtime contract before implementing an `apple-container`
provider. This is a hardware/OS-bound spike: it must run on Apple Silicon with
macOS 26 and the Apple `container` tool installed.

## Current Upstream Baseline

Authoritative upstream references:

- https://github.com/apple/container
- https://github.com/apple/container/blob/main/docs/technical-overview.md

Observed from the upstream repository on 2026-06-15:

- `container` creates and runs Linux containers as lightweight virtual
  machines on macOS.
- It is written in Swift and optimized for Apple silicon.
- It consumes and produces OCI-compatible container images.
- Upstream states Apple Silicon is required.
- Upstream states macOS 26 is the supported release family.
- The project is active; the visible latest release is `1.0.0` dated
  2026-06-09.

## Host Prerequisites

Record exact values before running:

```bash
sw_vers
uname -a
system_profiler SPHardwareDataType | sed -n '1,40p'
container --version
container system status || true
```

Required baseline:

- Apple Silicon Mac.
- macOS 26.
- Apple `container` installed from the upstream signed installer.
- `container system start` succeeds.
- Network path from the container VM to the management server is known.

## Image Under Test

Use an arm64 OCI image that contains `agent-rs` and the automation-control
loadout helpers. Prefer the same image convention used by GHCR release work
once #478 lands. Until then, test with a local image built from this repo:

```bash
# From repository root on the Apple host.
images/container/build.sh --platform linux/arm64 --tag agentic-sandbox-agent:apple-spike
container image list | grep agentic-sandbox-agent
```

If the current container image build path is Linux-only, record that as a
provider-contract gap and test a minimal arm64 image that can run the agent
binary plus a shell.

## Management Server Setup

Run management in a mode reachable from Apple `container` guests without
exposing plaintext bearer traffic to untrusted networks:

```bash
export LISTEN_ADDR=127.0.0.1:8120
export SECRETS_DIR=/var/lib/agentic-sandbox/secrets
export RUST_LOG=info
management/target/release/agentic-mgmt
```

If the guest VM cannot reach loopback on the macOS host, do not switch to
non-loopback plaintext as the default result. Instead, test one explicit
remote-access option and record the implications:

- SSH tunnel from guest/host network namespace to `127.0.0.1:8120`.
- gRPC mTLS listener with `AGENTIC_GRPC_MTLS_*`.
- A documented Apple `container` network endpoint that does not expose
  plaintext tokens to unrelated guests.

## Runtime Contract Checks

### 1. Create and Start

```bash
container run --name agentic-spike \
  --rm \
  agentic-sandbox-agent:apple-spike \
  /usr/local/bin/agent-rs --version
```

Pass criteria:

- Container VM starts deterministically with the requested name.
- Exit status is surfaced.
- Logs can be collected after process exit.

### 2. Management Connectivity

Run an agent container with environment equivalent to the normal bootstrap
contract. Record the exact network address that works.

```bash
container run --name agentic-spike-agent \
  --env MANAGEMENT_SERVER=<reachable-management-host>:8120 \
  --env AGENT_ID=apple-spike-01 \
  agentic-sandbox-agent:apple-spike
```

Pass criteria:

- Agent reaches management.
- Management registry shows the agent.
- No legacy shared secret path is required.

### 3. Workspace / Agentshare

Test whichever mount or file-transfer mechanism Apple `container` supports:

```bash
mkdir -p /tmp/agentic-apple-workspace
echo "apple container workspace probe" > /tmp/agentic-apple-workspace/probe.txt

container run --name agentic-spike-workspace \
  <mount flags discovered from container help/docs> \
  agentic-sandbox-agent:apple-spike \
  sh -lc 'cat /workspace/probe.txt && touch /workspace/agent-created.txt'
```

Pass criteria:

- Host-to-guest workspace data is visible.
- Guest-created output can be recovered or exported.
- Isolation semantics and permission behavior are documented.

### 4. Task Execution and Observation

From the management host:

```bash
curl -sS http://127.0.0.1:8122/api/v1/agents | jq
curl -sS -X POST http://127.0.0.1:8122/api/v1/agents/apple-spike-01/sessions \
  -H 'content-type: application/json' \
  -d '{"command":"sh","args":["-lc","echo apple-container-session; sleep 2"],"cols":120,"rows":30}' | jq
```

Pass criteria:

- Managed session starts.
- Observer stream or transcript contains the expected output.
- Cleanup removes the runtime and session without orphaned processes.

### 5. Stale Runtime Cleanup

```bash
container list --all
container stop agentic-spike-agent || true
container delete agentic-spike-agent || true
container list --all
```

Pass criteria:

- Stale runtimes can be discovered by name/metadata.
- Stop/delete failures produce parseable error output.
- Provider can map upstream states to agentic-sandbox states.

## Provider Contract Gap Checklist

Record `yes`, `partial`, or `no` for each:

| Capability | Result | Notes |
|------------|--------|-------|
| create/start by deterministic name | pending | |
| stop/destroy | pending | |
| state query | pending | |
| IP/endpoint discovery | pending | |
| logs | pending | |
| exec/attach strategy | pending | |
| workspace/agentshare setup | pending | |
| image pull/build | pending | |
| resource limits | pending | |
| stale runtime cleanup | pending | |
| bootstrap enrollment | pending | |
| secure transport without plaintext non-loopback | pending | |
| credential-aware startup helpers | pending | |

## Recommendation Template

After running, update this section with one of:

- **Proceed:** Apple `container` satisfies the minimum contract; start #489.
- **Proceed with gaps:** viable, but file follow-up issues for listed gaps.
- **Defer:** blocker in networking, mounts, lifecycle, or secure transport.
- **Reject:** cannot satisfy the model; choose another Apple provider.

## Current Recommendation

Execution is pending because the current workspace host is Linux x86_64:

```text
Linux grissom 6.17.0-35-generic ... x86_64 GNU/Linux
```

Do not close #488 until this document is filled with Apple-host command output
from a supported macOS 26 Apple Silicon machine.
