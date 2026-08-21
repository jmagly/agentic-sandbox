# Managed Celld fleet operations

## Purpose

This procedure deploys, checks, and reaps a disposable three-node Celld fleet
on Titan. It affects only exact-run Docker resources and the run directory
under `/dev/shm/agentic-celld-storage`; it does not replace or migrate local
filesystems, VM disks, agentshare, workspaces, bind mounts, container volumes,
or management state. The candidate lane is compatibility evidence only and
must not be treated as rollout qualification. Each workflow dispatch is
non-idempotent because it creates a new run ID; exact-run cleanup and janitor
preview are idempotent.

## System topology

- Execution host: `titan`, selected by the Gitea runner label `titan`.
- Control plane: Gitea workflow `.gitea/workflows/celld-fleet-fixture.yml`.
- Storage: one disposable, two-gateway SeaweedFS S3 fixture under
  `/dev/shm/agentic-celld-storage/titan-fleet-GITEA_RUN_ID`.
- Fleet: two active Celld containers and one reserve on a private Docker
  network; public Worker listeners bind only to host loopback.
- Approved lane: Celld v0.2.1 from
  `deploy/celld/qualification/celld-images.json`.
- Reviewed candidate lane: Celld v0.3.0 from
  `deploy/celld/qualification/celld-rollout-candidates.json`.
- Evidence: `celld-fleet-fixture-titan-GITEA_RUN_ID`, retained for 90 days.

The fleet manifest in `deploy/celld/fleet.example.json` is the deployment
contract for QEMU, Docker, or host substrates. The POC defaults to three QEMU
nodes with one reserved node. The pinned source archive is Celld v0.2.1 at
commit `ae8fac053d79f971bfcb996054bb43eb2f9b05da`, with the archive digest
recorded in the manifest.

## Procedure

### Preconditions

- Give each fleet one application bundle and one trust domain.
- Put the internal listener and advertised addresses on a private or encrypted overlay. Public ingress must route only to the public listener.
- Allocate a dedicated bucket per fleet. Shared-prefix isolation remains `NOT_RUN` until exact prefix IAM is proven. The credential reference resolves at service start to that bucket only; it must not resolve during image build or appear in environment captures, logs, metrics, or support bundles.
- Capture real conditional-create, conditional-overwrite, read-after-write, cleanup, and p99 latency evidence through the versioned raw-evidence qualifier described in [storage-qualification.md](storage-qualification.md). The deprecated `sandboxctl celld fleet-preflight` summary input always rejects because booleans are not proof.
- Verify the source/archive digest and application digest before installing them under immutable version paths.

### Package and node preflight

Linux packages include `agentic-celld.service`, the read-only `agentic-celld-preflight` helper, and redacted templates under `/usr/share/doc/agentic-sandbox/`. The unit uses a dynamic `celld` identity and creates its private state directory. Copy and edit the fleet, node, endpoint, and credential examples under `/etc/agentic-sandbox/celld/`; keep the populated credential file root-owned with mode 0600. Install the verified Celld binary at the immutable path in the unit and set `AGENTIC_CELLD_EXPECTED_BINARY_SHA256` to the digest of those exact bytes.

The service preflight checks the manifest, binary version and digest, local listener/store configuration, credential-file metadata, and that Celld's startup storage probe remains enabled. It neither reads credential contents nor contacts the store. Its JSON output has `scope=local_prestart`, `mutating=false`, and `live_qualification=false`; a successful local preflight is readiness evidence, not fleet qualification.

To classify captured observations through the management API, copy `deploy/celld/fleet-diagnose.example.json` and run `sandboxctl celld diagnose --file OBSERVATIONS.json`. `fixture` and `local` sources return `NOT_RUN` when their shapes agree, because only an allowlisted live driver may supply qualification evidence. Mismatches return `FAIL`. The upstream `celld diagnose --bucket ...` command is a separate live store probe; CLQ-02/CLQ-03 will capture and evaluate that evidence rather than trusting a caller-authored verdict.

### Disposable Titan fixture

The protected qualification lane uses `scripts/celld-fleet-fixture.mjs` after
the exact SeaweedFS fixture has been prepared and started. The default
`approved` channel creates three separately named Celld v0.2.1 containers from
the reviewed linux/amd64 manifest in
`deploy/celld/qualification/celld-images.json`. The explicit
`reviewed-candidate` channel instead uses Celld v0.3.0 commit
`89e4ffc53a14ecb496d2ca5014ff9d19b0061ad9`, index digest
`sha256:f47d97c2980aa98aef1d9c42205a313442f48acb606c5987dbb9b32983a23aaf`,
and linux/amd64 manifest digest
`sha256:e2983741d4733a537dcdb399671d3ce2f6968bfe4f15ce0a70c0279e10a930d1`.
The candidate step verifies its SLSA v1 release-workflow identity, source
commit/ref, Rekor inclusion, index platform mapping, and image config digest
before starting the fixture. Two nodes are active and one is the declared
rollout reserve. This is deliberately labeled `single-host multi-node`; it is
not evidence of physical-host, rack, or availability-zone resilience.

The public Worker listener is published only on a dynamic host-loopback port.
The unauthenticated internal/operator listener is never published and remains
on the run's internal Compose network. Celld receives the bucket-scoped
credential file and public storage CA certificate as read-only mounts. It does
not receive the fixture administrator identity, CA private key, or S3 gateway
private key. Each node also receives one run-scoped, mode-0600 Worker-vars file
through `CELLD_VARS_FILE`; its request-HMAC value is never copied into a Docker
argument or environment value. The file is inventoried before creation,
mounted read-only, and removed before the run's fleet directory can be reaped.
The pinned Worker runtime cannot originate a client-certificate handshake.
Consequently each node gets a separately inventoried callback-relay container
that shares only that node's network namespace. The Worker calls a fixed HTTP
listener on node loopback; the relay forwards opaque bytes to the management
bridge address using the run CA and its exact `agentic-celld-worker-callback`
client certificate. The relay receives neither the Worker HMAC key nor object
store credentials, publishes no port, and is removed before its Celld node.
Management's private key remains host-only. This preserves mutual transport
authentication without weakening the Worker sandbox or exposing callback
traffic on the fleet network.

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

1. Dispatch the default approved lane from a clean, pushed `main`:

   ```sh
   tea actions workflows dispatch celld-fleet-fixture.yml \
     --repo roctinam/agentic-sandbox \
     --ref main \
     --input celld_channel=approved
   ```

   Expected: Gitea creates one `Celld Fleet Fixture on Titan` run for the
   exact `main` commit. Confirm with the verification command below before
   retrying, even if an older `tea` client reports a response-decoding error.

2. After the approved run finishes and only when candidate compatibility
   evidence is required, dispatch the non-promoting candidate lane:

   ```sh
   tea actions workflows dispatch celld-fleet-fixture.yml \
     --repo roctinam/agentic-sandbox \
     --ref main \
     --input celld_channel=reviewed-candidate
   ```

   Expected: one capacity-serialized Titan run whose provenance artifact has
   `status=VERIFIED`, `channel=reviewed-candidate`, and
   `qualification_status=reviewed_unqualified`.

`diagnose` runs the pinned upstream conditional-write storage probe and signed
direct peer probes for all three advertised internal addresses. A merely
running container is therefore not reported as ready membership.

`probe-worker` sends a signed request to the primary node's loopback-only
public listener and requires the reviewed Worker to return its exact
missing-cell response. It also requires a forged signature and replayed nonce
to be denied. Only status/code tuples and the reviewed Worker digest enter the
artifact; HMAC keys, signatures, headers, and nonces do not. This is deployment
readiness evidence and does not promote the 1,000-attempt UAT-012 hard gate.

`start-relays` accepts only an executable regular non-symlink binary, hashes
its exact bytes into the inventory, derives the RFC1918 target from the
owner-validated private network, and mounts only the public CA plus callback
client certificate/key. The relay itself accepts only a fixed loopback listen
socket and a fixed unicast management target. A run that cannot build the
relay as a static `x86_64-unknown-linux-musl` binary remains `NOT_RUN`.
Normal fleet runs pass `--fault-signal disabled`. An explicitly authorized
qualification controller may instead start the relay with
`--fault-signal enabled` and send `SIGUSR1` to drop exactly the next management
response after the relay has observed response bytes. This creates a real
post-effect response-loss window without parsing callback contents; the relay
then returns immediately to normal forwarding.

### Rolling update

Run `sandboxctl celld fleet-plan-upgrade --file fleet.json --from OLD --to NEW` to obtain advisory reserve arithmetic. An unknown pair is refused. Every returned plan currently has `mutating=false` and `execution_controller=null`: it does not drain, replace, or restart a node. Operators must not interpret it as authorization or proof of a rolling update.

The Titan qualification workflow installs `celld-live-rollout`, which checks
the exact immutable artifact inventory before mutation. The approved inventory
currently contains only Celld v0.2.1 at one digest, while v0.3.0 remains in the
separate reviewed-but-unqualified candidate inventory. UAT-CELLD-011 therefore
returns the typed pre-mutation result `CELLD_ROLLOUT_CANDIDATE_UNQUALIFIED`.
Running the candidate fixture does not copy it into `celld-images.json`, create
an approved old/new pair, or authorize node mutation. Once a second artifact
is separately qualified and added to the approved inventory, the driver still
remains `NOT_RUN` with `CELLD_ROLLOUT_CONTROLLER_UNAVAILABLE` until the
replacement controller and its destructive authorization gate are reviewed.

The eventual controller must add or select a healthy reserve node, verify bucket access and protocol compatibility, drain one node, replace it, diagnose the resulting live observations, then wait for membership and cell reconciliation before proceeding. It must never reduce available nodes below `count - max_unavailable` or consume the declared reserve. Roll back to the still-installed previous digest if error rate exceeds 1%, p99 coordination latency regresses over 20%, any acknowledged command disappears, or any stale generation performs an effect.

Node loss must not alter durable intent. Replacement nodes rebuild from the object store and management observations. Destroying compute retains the bucket when `retain_on_destroy` is true; bucket deletion is a separate, explicitly approved retention operation.

### Credential rotation

Create a new scoped credential, update the broker reference version, canary one node, confirm read/write/conditional operations, roll remaining nodes, then revoke the old credential. The overlap window is at most 15 minutes. A failed canary restores the old reference; it does not broaden bucket scope.

### Substrate decisions

QEMU is the production candidate because it preserves the platform's hardware boundary. Docker is accepted only for trusted development and integration fleets. Host mode is a diagnostic POC target and is not approved for production because Celld and its bucket-root credential share the management host's failure domain.

## Verification

1. Verify the checked-in candidate record without contacting Titan:

   ```sh
   node scripts/celld-rollout-candidate.mjs check \
     --input deploy/celld/qualification/celld-rollout-candidates.json
   ```

   Expected output:

   ```json
   {"status":"PASS","candidates":1,"versions":["0.3.0"],"qualification_statuses":["reviewed_unqualified"]}
   ```

2. Verify the focused structural behavior:

   ```sh
   node --test tests/celld/uat/rollout-candidate.test.mjs \
     tests/celld/uat/fleet-fixture.test.mjs \
     tests/celld/uat/live-rollout.test.mjs \
     tests/celld/uat/titan-workflow.test.mjs
   ```

   Expected: all tests pass, with zero failed, cancelled, skipped, or todo.

3. Confirm the exact-head Titan run after dispatch:

   ```sh
   tea actions runs list \
     --repo roctinam/agentic-sandbox \
     --branch main \
     --limit 5 \
     --output json | jq '.[0] | {id,status,commit_sha,name}'
   ```

   Expected: `name` identifies the Celld fleet fixture, `commit_sha` equals
   the dispatched `main` commit, and `status` eventually becomes `success`.
   The uploaded provenance and diagnosis artifacts must agree on the selected
   channel, version, and manifest digest.

## Troubleshooting

- **Candidate checker exits 3**: the candidate file no longer matches the
  reviewed record. Do not loosen validation; repeat the provenance review and
  update the record and validator together.
- **`gh attestation verify` fails**: retain the candidate as unqualified and
  inspect the upstream release identity. Do not fall back to tag-only trust.
- **OCI index or config comparison fails**: stop the run. A tag or index may
  have resolved to bytes outside the reviewed linux/amd64 chain.
- **Titan run stays queued**: confirm the `titan` runner service and capacity;
  do not reroute this fixture to the local workstation or a generic runner.
- **Cleanup exits 4**: preserve the inventory and inspect exact-run resources.
  Never replace exact cleanup with a global Docker prune.

## House Rules for Agents

- Do run structural tests locally and resource-intensive fixtures only on
  Titan.
- Do compare the workflow commit SHA and all immutable digests before using
  evidence.
- Do preserve the default `approved` channel and require an explicit input for
  `reviewed-candidate`.
- Do stop on provenance, cleanup, ownership, or readiness disagreement.
- Do not promote a candidate, enable rollout mutation, or broaden credentials
  based on a successful fixture smoke run.
- Do not expose object-store credentials, Worker HMAC material, signatures,
  headers, or nonces in artifacts or logs.

## What NOT to Fix

- `reviewed_unqualified` is intentional after a successful candidate fixture;
  fixture compatibility is not rollout qualification.
- `single-host multi-node` is intentionally narrower than multi-host
  resilience evidence.
- Local filesystems and volume mounts intentionally remain the default storage
  paths; S3 backing applies only to explicitly enabled Celld fleets.
- UAT 016 soak and UAT 017 human acceptance intentionally remain operator-run.
- The one-node reserve and capacity-one Titan workflow concurrency are safety
  constraints, not under-utilization bugs.

## Audit Trail

- 2026-08-21: Agentic Sandbox maintainers — documented strict Celld v0.3.0
  candidate provenance and the non-promotion boundary.
- Last verified: 2026-08-21 on the local structural suite; live candidate
  verification remains pending on Titan.
- Applicable host: Titan only for live fleet execution; repository-only checks
  apply to supported development hosts.
