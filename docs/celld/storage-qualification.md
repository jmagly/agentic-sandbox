# Celld object-store qualification

S3-compatible storage is an optional backing store for enabled Celld fleets. It is not a general Sandbox volume driver. Local filesystems, workspaces, agentshare mounts, VM disks, container volumes, bind mounts, and management state remain on their existing paths and remain the default when Celld is disabled.

## Versioned contracts

The provider-neutral profile is `tests/celld/uat/storage-profile-v1.schema.json`; raw measurements use `storage-evidence-v1.schema.json`. Both bind the backend artifact and non-secret fixture configuration by SHA-256. The first implemented dialect is `s3-v1`. `gcs-reserved` is structurally recognizable but always evaluates to `NOT_RUN`; S3 ETags and conditional requests cannot establish Google Cloud Storage generation semantics.

Profiles contain endpoint/topology identity and protected file references, never access-key or secret values. The client reads the default AWS shared-credentials profile only when an enabled Celld storage run sends a request, requires a regular non-symlink identity file with mode 0600 or stricter, uses path-style SigV4 requests, and performs no implicit retries. HTTP is limited to loopback fixtures; other endpoints require HTTPS.

Validate a profile with:

```sh
node scripts/celld-storage-qualifier.mjs profile-check --input PROFILE.json
```

Evaluate raw evidence with:

```sh
node scripts/celld-storage-qualifier.mjs evaluate --input EVIDENCE.json
```

Exit codes are 0 for a derived `PASS`, 1 for a derived `FAIL`, 2 for `NOT_RUN`, and 3 for invalid evidence or evaluator error. The legacy `sandboxctl celld fleet-preflight` summary endpoint is retained only to return a typed rejection; caller-supplied booleans cannot qualify storage.

## S3-v1 outcome rules

- One 2xx result is the commit for each conditional round.
- A 412 is a loser only with `PreconditionFailed`.
- A 409 is a loser only with `ConditionalRequestConflict` and an immediate HEAD/GET reconciliation proving the committed winner bytes, current validator, and absence of the losing bytes. Any missing or contradictory fact is ambiguous and fails.
- Invalid/expired identity, wrong-bucket, and cross-bucket cases must all be denied. Shared-prefix IAM is not inferred; it remains `NOT_RUN` until separately proven.
- Timeouts, unfamiliar statuses/error codes, stale first reads, missing counts, and mismatched totals fail closed.

A live candidate uses 10,000 seeded create-if-absent rounds and 10,000 seeded conditional-overwrite rounds, at least two simultaneously released contenders on distinct gateways, at most 32 workers and 64 connections, and exactly 100 non-gating warmups. Safety races and immediate reads retry zero times. Authentication cases run once. Other transient experiments allow at most three attempts and 30 seconds, with total request amplification at most 3.0.

The evaluator computes nearest-rank p99 as the sorted sample at `ceil(0.99 × sample_count)` after warmups. Successful create winners, overwrite winners, immediate GET, and HEAD each require at least 10,000 samples and independently require p99 at most 250 ms. Conditional losers and fault intervals do not dilute those samples.

## Self-hosted fixture profiles

The repository reviews and pins SeaweedFS 4.41 for Linux/amd64 at manifest `sha256:3bbe24f6d5f5818327adcfeda7d85240ed53212dab05f91af14484c6446ec5eb`. The tag is documentation only; Compose consumes the immutable manifest identity. A different version, platform manifest, configuration, or topology requires a new qualification run.

`single-process-protocol` runs the pinned `weed mini` image on a private Docker network with one loopback HTTP gateway and a 2 CPU/2 GiB ceiling. It is a fast protocol fixture only. It cannot measure cross-gateway serialization or component loss and always remains non-promoting. The independent broken-store fixtures exercise ignored `If-None-Match`, ignored/stale `If-Match`, gateway-local locking, stale first read, and misleading success/error classification. Even complete reduced evidence returns `NOT_RUN` with `live_qualification=false`.

`titan-single-host-storage` is the live candidate: three Raft masters, three volume servers labeled as separate logical racks with replication `010`, three filers sharing one dedicated PostgreSQL metadata service, and two standalone TLS S3 gateways. Client traffic is HTTPS on loopback-only published ports; component traffic stays on an internal Docker network. The topology has explicit CPU, memory, and PID ceilings. Passing evidence may support one-master, one-volume-server, and one-gateway experiments. It does not establish PostgreSQL metadata-service, physical-host, physical-rack, or availability-zone tolerance.

Every run creates an unpredictable bucket, a bucket-scoped identity, a separate fixture administrator, a two-day run CA/server certificate, and a revoked negative-test identity under a mode-0700 `/dev/shm` tmpfs directory. Secret-bearing files are mode 0600 and their values never enter profiles, command lines, or evidence. Shared-prefix IAM remains `NOT_RUN`; bucket-per-run isolation is the default. The live driver empties and deletes the run bucket, removes Compose services/networks/volumes, and emits cleanup failure as exit 4.

Fixture cleanup does not trust a successful `compose down` result by itself. It
performs label-scoped container, network, and volume absence sweeps for the
unpredictable exact project, then removes and verifies only the marker-owned run
root. A failed down, unavailable sweep, retained project resource, or retained
run root is `CELLD_SEAWEEDFS_CLEANUP_RESIDUE` with exit 4; shared Docker state is
never pruned.

Provider exit is offline-only. `scripts/celld-offline-migration.mjs` implements the provider-neutral state machine: enumerate and stop every writer class, deny application writes, require two identical listings, copy every key/body/metadata value, compare independent hashes, canary the destination while source writes remain denied, rehearse direct rollback only before destination writes, record cutover, prove a post-cutover application write changed the durable manifest, and require the same quiesced verified process in reverse. The controller rejects concurrent authorities and accepts only the `celld_object_store_only` scope; it cannot target local filesystems, volume mounts, workspaces, VM disks, agentshare, or management state.

`scripts/celld-live-offline-migration.mjs` is the Titan adapter for that state machine. It first subjects a distinct destination to the full 10,000-create/10,000-overwrite two-gateway S3-v1 qualification gate, then seeds and changes Celld state through signed Worker commands rather than direct test writes. It toggles only the disposable run-bucket application policy, stops all three Celld nodes (including their alarm writers), observes the deployment CLI and management-reconciler classes, and compares the complete run namespace in both directions. The success record binds the exact commit, host, Celld/Worker pins, backend pins/topologies, hashed bucket namespaces, cutover manifests, destination qualification artifacts, and cleanup result. The workflow verifies a SHA-256 manifest and uploads both aggregate and raw destination evidence.

This live rehearsal owns two newly generated, run-scoped fixtures under `/dev/shm`; it is not an operator migration command and never accepts an existing Sandbox storage path. Successful and failed runs remove only exact label/marker-owned disposable fleets, Compose resources, and destination roots before the postflight baseline comparison. Deleting retained non-disposable source data remains a separately approved operator action.

The provider-neutral race runner records one raw JSONL row per create/overwrite/denial case and a separately validated evidence envelope. The envelope binds the bucket/prefix scope by SHA-256 without recording credentials. Live evidence does not duplicate the five broken-store results: those are owned by the deterministic supporting assertion, while the live assertion owns the exact backend races, reads, latency, denials, and cleanup.

Run the heavyweight candidate only on Titan through the manual `Celld Storage Qualification on Titan` workflow (`.gitea/workflows/celld-storage-qualification.yml`). It retains evidence for 90 days and intentionally accepts UAT-010's bounded exit 2 only when `CELLD.010.STORAGE` and its deterministic support pass and the separately owned `CELLD.010.ISOLATION` assertion remains `NOT_RUN`. The complete qualification workflow also enables this storage driver, but still fails overall while any other mandatory live assertion is not run.

This fixture adds an S3-compatible option for enabled Celld fleets. It does not replace or migrate local filesystems, workspaces, agentshare, VM disks, container volumes, bind mounts, or management state.
