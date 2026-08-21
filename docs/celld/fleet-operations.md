# Managed Celld fleet operations

The fleet manifest in `deploy/celld/fleet.example.json` is the deployment contract for QEMU, Docker, or host substrates. The POC defaults to three QEMU nodes with one reserved node. The pinned source archive is Celld v0.2.1 at commit `ae8fac053d79f971bfcb996054bb43eb2f9b05da`, with the archive digest recorded in the manifest.

## Preconditions

- Give each fleet one application bundle and one trust domain.
- Put the internal listener and advertised addresses on a private or encrypted overlay. Public ingress must route only to the public listener.
- Allocate a dedicated bucket per fleet. Shared-prefix isolation remains `NOT_RUN` until exact prefix IAM is proven. The credential reference resolves at service start to that bucket only; it must not resolve during image build or appear in environment captures, logs, metrics, or support bundles.
- Capture real conditional-create, conditional-overwrite, read-after-write, cleanup, and p99 latency evidence through the versioned raw-evidence qualifier described in [storage-qualification.md](storage-qualification.md). The deprecated `sandboxctl celld fleet-preflight` summary input always rejects because booleans are not proof.
- Verify the source/archive digest and application digest before installing them under immutable version paths.

## Package and node preflight

Linux packages include `agentic-celld.service`, the read-only `agentic-celld-preflight` helper, and redacted templates under `/usr/share/doc/agentic-sandbox/`. The unit uses a dynamic `celld` identity and creates its private state directory. Copy and edit the fleet, node, endpoint, and credential examples under `/etc/agentic-sandbox/celld/`; keep the populated credential file root-owned with mode 0600. Install the verified Celld binary at the immutable path in the unit and set `AGENTIC_CELLD_EXPECTED_BINARY_SHA256` to the digest of those exact bytes.

The service preflight checks the manifest, binary version and digest, local listener/store configuration, credential-file metadata, and that Celld's startup storage probe remains enabled. It neither reads credential contents nor contacts the store. Its JSON output has `scope=local_prestart`, `mutating=false`, and `live_qualification=false`; a successful local preflight is readiness evidence, not fleet qualification.

To classify captured observations through the management API, copy `deploy/celld/fleet-diagnose.example.json` and run `sandboxctl celld diagnose --file OBSERVATIONS.json`. `fixture` and `local` sources return `NOT_RUN` when their shapes agree, because only an allowlisted live driver may supply qualification evidence. Mismatches return `FAIL`. The upstream `celld diagnose --bucket ...` command is a separate live store probe; CLQ-02/CLQ-03 will capture and evaluate that evidence rather than trusting a caller-authored verdict.

## Disposable Titan fixture

The protected qualification lane uses `scripts/celld-fleet-fixture.mjs` after
the exact SeaweedFS fixture has been prepared and started. It creates three
separately named Celld v0.2.1 containers from the reviewed linux/amd64 manifest
in `deploy/celld/qualification/celld-images.json`. Two nodes are active and one
is the declared rollout reserve. This is deliberately labeled
`single-host multi-node`; it is not evidence of physical-host, rack, or
availability-zone resilience.

The public Worker listener is published only on a dynamic host-loopback port.
The unauthenticated internal/operator listener is never published and remains
on the run's internal Compose network. Celld receives the bucket-scoped
credential file and public storage CA certificate as read-only mounts. It does
not receive the fixture administrator identity, CA private key, or S3 gateway
private key. Each node also receives one run-scoped, mode-0600 Worker-vars file
through `CELLD_VARS_FILE`; its request-HMAC value is never copied into a Docker
argument or environment value. The file is inventoried before creation,
mounted read-only, and removed before the run's fleet directory can be reaped.

After storage is ready and before the nodes start, the fixture's `deploy`
operation creates the unpredictable run bucket with the controller-only
administrator identity and deploys the reviewed reference Worker by its
committed digest. The short-lived deployer is read-only, capability-dropped,
resource-limited, attached only to the run network, and mounts only the Worker
project, exact esbuild executable, bucket-scoped identity, and public CA. Raw
deployment output is not retained; evidence records its SHA-256 digest. The
administrator identity is never mounted into either the deployer or a Celld
node. The deployer target and bucket mutation are inventoried before execution,
and interrupted deployers are removed only after exact ownership validation.

Every directory, container creation, start, diagnosis, and removal is persisted
to `fleet-inventory.json` before mutation. Cleanup verifies exact repository,
workflow, run, and scope labels before removing a named container, never prunes
shared Docker state, and independently sweeps the exact run labels. Residue is
an error with exit code 4. The janitor requires an owner-matching inventory and
a minimum age of at least one hour; ambiguous and partial inventories are
retained for operator review.

```sh
node scripts/celld-fleet-fixture.mjs prepare \
  --storage-config /dev/shm/agentic-celld-storage/RUN/fixture.json
node scripts/celld-fleet-fixture.mjs deploy \
  --config /dev/shm/agentic-celld-storage/RUN/fleet.json
node scripts/celld-fleet-fixture.mjs start \
  --config /dev/shm/agentic-celld-storage/RUN/fleet.json
node scripts/celld-fleet-fixture.mjs diagnose \
  --config /dev/shm/agentic-celld-storage/RUN/fleet.json
node scripts/celld-fleet-fixture.mjs probe-worker \
  --config /dev/shm/agentic-celld-storage/RUN/fleet.json
node scripts/celld-fleet-fixture.mjs cleanup \
  --config /dev/shm/agentic-celld-storage/RUN/fleet.json
node scripts/celld-fleet-fixture.mjs janitor-preview \
  --root /dev/shm/agentic-celld-storage --minimum-age-seconds 21600
```

`diagnose` runs the pinned upstream conditional-write storage probe and signed
direct peer probes for all three advertised internal addresses. A merely
running container is therefore not reported as ready membership.

`probe-worker` sends a signed request to the primary node's loopback-only
public listener and requires the reviewed Worker to return its exact
missing-cell response. It also requires a forged signature and replayed nonce
to be denied. Only status/code tuples and the reviewed Worker digest enter the
artifact; HMAC keys, signatures, headers, and nonces do not. This is deployment
readiness evidence and does not promote the 1,000-attempt UAT-012 hard gate.

## Rolling update

Run `sandboxctl celld fleet-plan-upgrade --file fleet.json --from OLD --to NEW` to obtain advisory reserve arithmetic. An unknown pair is refused. Until CLQ-10 implements the rollout controller, every returned plan has `mutating=false` and `execution_controller=null`: it does not drain, replace, or restart a node. Operators must not interpret it as authorization or proof of a rolling update.

The eventual controller must add or select a healthy reserve node, verify bucket access and protocol compatibility, drain one node, replace it, diagnose the resulting live observations, then wait for membership and cell reconciliation before proceeding. It must never reduce available nodes below `count - max_unavailable` or consume the declared reserve. Roll back to the still-installed previous digest if error rate exceeds 1%, p99 coordination latency regresses over 20%, any acknowledged command disappears, or any stale generation performs an effect.

Node loss must not alter durable intent. Replacement nodes rebuild from the object store and management observations. Destroying compute retains the bucket when `retain_on_destroy` is true; bucket deletion is a separate, explicitly approved retention operation.

## Credential rotation

Create a new scoped credential, update the broker reference version, canary one node, confirm read/write/conditional operations, roll remaining nodes, then revoke the old credential. The overlap window is at most 15 minutes. A failed canary restores the old reference; it does not broaden bucket scope.

## Substrate decisions

QEMU is the production candidate because it preserves the platform's hardware boundary. Docker is accepted only for trusted development and integration fleets. Host mode is a diagnostic POC target and is not approved for production because Celld and its bucket-root credential share the management host's failure domain.
