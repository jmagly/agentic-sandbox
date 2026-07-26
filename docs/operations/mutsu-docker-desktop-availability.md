# Mutsu Docker Desktop Availability

## Purpose

This runbook defines the operator-owned availability policy and bounded
recovery procedure for Docker Desktop on `mutsu` when the serialized
exact-commit macOS validation lane is eligible to run. Repository automation
must fail explicitly when Docker Desktop is unavailable; it must never install
Docker, start the application, change login items or launch services, inspect
credentials, or mutate unrelated host state. The diagnostic procedure is
read-only and idempotent. Starting Docker Desktop is a single-host,
service-affecting operation with **HIGH** blast radius and requires explicit
mutsu host authority.

## System Topology

- Apple validation host: `mutsu` (`10.0.42.41`), Apple Silicon arm64 macOS.
- Orchestrator runner: Titan. Teroknor and grissom have no Apple build,
  package, or validation responsibility.
- Workflow: `.gitea/workflows/macos-validation.yml`.
- Remote validation base:
  `/Volumes/build/agentic-sandbox/macos-validation`.
- Serialized lock:
  `/Volumes/build/agentic-sandbox/macos-validation/.lock`.
- Read-only diagnostic: `scripts/check-macos-docker-desktop.sh`.
- Supported runtime: Docker Desktop with an active `desktop-linux` or
  `default` CLI context exposing a Linux `aarch64` daemon.

**Availability policy:** Docker Desktop should remain running on this dedicated
build host during periods when the macOS validation lane is eligible. The
repository does not enforce availability by starting it. An unavailable daemon
causes a visible preflight failure and operator action. Enabling a login item,
launch agent, or any other automatic-start mechanism is outside this runbook
and requires a separate, explicit mutsu host change authorization.

## Procedure

1. From the exact repository checkout on mutsu, run the sanitized read-only
   diagnostic:

   ```bash
   cd /Volumes/build/agentic-sandbox/current
   scripts/check-macos-docker-desktop.sh
   ```

   Expected output when ready:

   ```text
   status=ready diagnostic=none docker_os=linux docker_arch=aarch64
   ```

   A not-ready result prints exactly one sanitized diagnostic code:
   `cli-missing`, `context-inactive`, or `daemon-unreachable`. It does not print
   context endpoints, credential helpers, registry configuration, environment
   variables, or Docker's full `info` response.

2. If the result is `context-inactive`, select an already-configured Docker
   Desktop context. This changes only the invoking user's active Docker CLI
   context:

   ```bash
   docker context use desktop-linux
   ```

   Expected output:

   ```text
   desktop-linux
   Current context is now "desktop-linux"
   ```

   If `desktop-linux` is not already configured, stop. Do not create, copy, or
   inspect another user's context.

3. If the result is `daemon-unreachable`, stop unless the current operator has
   explicit authority to start Docker Desktop on mutsu. After that authority
   is recorded, a human operator may run:

   ```bash
   open -a Docker
   ```

   Expected output: none. Docker Desktop opens in the logged-in user's session.
   Wait for its UI to report that the engine is running; do not automate UI
   interaction or bypass a prompt.

4. Repeat the read-only diagnostic:

   ```bash
   cd /Volumes/build/agentic-sandbox/current
   scripts/check-macos-docker-desktop.sh
   ```

   Expected output:

   ```text
   status=ready diagnostic=none docker_os=linux docker_arch=aarch64
   ```

5. Dispatch `.gitea/workflows/macos-validation.yml` against the exact commit
   requiring evidence. Do not substitute a newer commit. The workflow owns its
   per-run archive, workspace, image, credential, and lock cleanup.

## Verification

1. Confirm readiness without exposing daemon or context configuration:

   ```bash
   cd /Volumes/build/agentic-sandbox/current
   scripts/check-macos-docker-desktop.sh
   ```

   Expected:

   ```text
   status=ready diagnostic=none docker_os=linux docker_arch=aarch64
   ```

2. In the Gitea run for the exact commit, confirm these stages succeed:

   ```text
   stage=native-host-secure-enrollment-task-session-lifecycle
   stage=docker-desktop-arm64-smoke
   stage=docker-secure-enrollment-task-session-lifecycle
   macOS validation completed
   ```

3. Confirm the run uploads `macos-validation-<full-commit-sha>` and the
   `Remove local SSH material` step succeeds. The remote collection step must
   remove only that run's workspace; the validation script traps must remove
   the temporary image, container, credentials, sockets, and lock ownership.

## Troubleshooting

- **`diagnostic=cli-missing`**: Docker CLI is absent from the workflow PATH.
  Stop and report host provisioning drift. This runbook does not authorize
  installation or PATH mutation.
- **`diagnostic=context-inactive`**: The selected context is missing, not one of
  the supported Docker Desktop contexts, or does not expose Linux arm64.
  Select the already-configured `desktop-linux` context only. Do not inspect or
  copy credential/context files.
- **`diagnostic=daemon-unreachable`**: The accepted context exists but its
  daemon does not answer. A host-authorized human may start Docker Desktop.
  Repository automation must not do so.
- **Validation lock is held**: Read only the run ID, PID, and start time from
  the lock owner record. Confirm the Gitea run is inactive and the PID no
  longer exists before removing only the stale `.lock` directory. Never remove
  the validation base or another run's workspace.
- **Docker becomes unavailable after preflight**: Treat the run as failed.
  Restore availability through the same authority gate and dispatch the same
  commit again; do not weaken or skip the Docker lifecycle stage.

## House Rules for Agents

- DO run the diagnostic exactly as written and compare its sanitized output.
- DO stop on any nonzero diagnostic or failed lifecycle stage.
- DO preserve exact-commit execution, the serialized mutsu lock, and per-run
  cleanup.
- DON'T install software, open applications, enable login items, or change
  launch services.
- DON'T read context endpoints, credential helpers, registry auth, raw
  environment dumps, or another user's Docker files.
- DON'T involve teroknor or grissom in Apple build, package, or validation
  work.
- DON'T turn a failed Docker preflight into a skipped or successful lane.

## What NOT to Fix

- The explicit preflight failure is intentional; it prevents a green run from
  silently omitting Docker Desktop validation.
- Titan only orchestrates SSH. Apple compilation and runtime validation remain
  on mutsu.
- Docker Desktop is not started by the workflow, even though the availability
  policy expects it to be running during eligible validation windows.
- The `desktop-linux` and `default` context names are accepted because Docker
  Desktop versions differ; readiness still requires a Linux arm64 daemon.
- Grissom remains an active workstation and local-only validation host.

## Audit Trail

| Date | Author | Change | Applicable host | Last verified |
|---|---|---|---|---|
| 2026-07-26 | AIWG Al, operator-authorized under issue #675 | Initial availability policy, sanitized diagnostic, and bounded recovery procedure | mutsu only | Pending exact-commit Gitea run |
