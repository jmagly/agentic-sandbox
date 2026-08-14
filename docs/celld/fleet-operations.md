# Managed Celld fleet operations

The fleet manifest in `deploy/celld/fleet.example.json` is the deployment contract for QEMU, Docker, or host substrates. The POC defaults to three QEMU nodes with one reserved node. The pinned source archive is Celld v0.2.1 at commit `ae8fac053d79f971bfcb996054bb43eb2f9b05da`, with the archive digest recorded in the manifest.

## Preconditions

- Give each fleet one application bundle and one trust domain.
- Put the internal listener and advertised addresses on a private or encrypted overlay. Public ingress must route only to the public listener.
- Allocate a dedicated bucket/prefix. The credential reference resolves at service start to bucket-root access for that prefix only; it must not resolve during image build or appear in environment captures, logs, metrics, or support bundles.
- Capture real conditional-create, conditional-overwrite, read-after-write, cleanup, and p99 latency evidence. `sandboxctl celld fleet-preflight` rejects incomplete semantics or p99 over 250 ms.
- Verify the source/archive digest and application digest before installing them under immutable version paths.

## Rolling update

Run `sandboxctl celld fleet-plan-upgrade --file fleet.json --from OLD --to NEW`. An unknown pair is refused. For each returned batch: add or select a healthy reserve node, verify bucket access and protocol compatibility, drain one node, replace it, run `celld diagnose`, then wait for membership and cell reconciliation before proceeding. Never reduce available nodes below `count - max_unavailable`, and never consume the declared reserve. Roll back to the still-installed previous digest if error rate exceeds 1%, p99 coordination latency regresses over 20%, any acknowledged command disappears, or any stale generation performs an effect.

Node loss must not alter durable intent. Replacement nodes rebuild from the object store and management observations. Destroying compute retains the bucket when `retain_on_destroy` is true; bucket deletion is a separate, explicitly approved retention operation.

## Credential rotation

Create a new scoped credential, update the broker reference version, canary one node, confirm read/write/conditional operations, roll remaining nodes, then revoke the old credential. The overlap window is at most 15 minutes. A failed canary restores the old reference; it does not broaden bucket scope.

## Substrate decisions

QEMU is the production candidate because it preserves the platform's hardware boundary. Docker is accepted only for trusted development and integration fleets. Host mode is a diagnostic POC target and is not approved for production because Celld and its bucket-root credential share the management host's failure domain.
