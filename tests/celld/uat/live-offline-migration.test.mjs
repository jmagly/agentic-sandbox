import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import test from "node:test";

import { buildMigrationFailureEvidence, LiveMigrationJournal, LiveS3MigrationStore, summarizeDestinationQualificationRows } from "../../../scripts/celld-live-offline-migration.mjs";

function fixtureClient() {
  const calls = [];
  const objects = new Map([
    ["fleet/a", { body: Buffer.from("alpha"), metadata: { "content-type": "application/octet-stream", "x-amz-meta-generation": "1" } }],
    ["fleet/b", { body: Buffer.from("bravo!"), metadata: { "content-type": "application/json", "cache-control": "no-store" } }],
  ]);
  return {
    calls, objects,
    async createBucket() { return { status: 200 }; },
    async listPrefix() { return { status: 200 }; },
    async request(method, key, options) {
      calls.push({ method, key, options });
      const second = options.query["continuation-token"] === "page-2";
      const body = second
        ? "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>qualification/run-001/fleet/b</Key><Size>6</Size></Contents></ListBucketResult>"
        : "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>page-2</NextContinuationToken><Contents><Key>qualification/run-001/fleet/a</Key><Size>5</Size></Contents></ListBucketResult>";
      return { status: 200, body: Buffer.from(body), headers: {} };
    },
    async head(key) { const value = objects.get(key); return { status: value ? 200 : 404, headers: value ? { "content-length": String(value.body.length), etag: "ignored", ...value.metadata } : {} }; },
    async get(key) { const value = objects.get(key); return { status: value ? 200 : 404, body: value?.body ?? Buffer.alloc(0), headers: value?.metadata ?? {} }; },
    async put(key, body, options) { objects.set(key, { body: Buffer.from(body), metadata: options.headers }); return { status: 200 }; },
    async delete(key) { objects.delete(key); return { status: 204 }; },
    close() {},
  };
}

function store(client = fixtureClient()) {
  const config = { project: "celld-s3-test", bucket: "celld-test-bucket" };
  const profile = { run_prefix: "qualification/run-001" };
  return { value: new LiveS3MigrationStore(config, { profile, client }), client };
}

test("live migration store paginates the exact prefix and preserves copyable metadata", async () => {
  const { value, client } = store();
  await value.ensureBucket();
  assert.equal(await value.ready(), true);
  assert.deepEqual(await value.list(), [
    { key: "fleet/a", size: 5, metadata: { "content-type": "application/octet-stream", "x-amz-meta-generation": "1" } },
    { key: "fleet/b", size: 6, metadata: { "cache-control": "no-store", "content-type": "application/json" } },
  ]);
  assert.equal(client.calls.length, 2);
  assert.equal(client.calls[1].options.query["continuation-token"], "page-2");
  assert.deepEqual(await value.get("fleet/a"), { body: Buffer.from("alpha"), metadata: { "content-type": "application/octet-stream", "x-amz-meta-generation": "1" } });
  await value.put("fleet/c", "charlie", { "content-type": "text/plain" });
  assert.equal(client.objects.get("fleet/c").metadata["content-type"], "text/plain");
  await value.delete("fleet/c");
  assert.equal(client.objects.has("fleet/c"), false);
});

test("live migration store accepts only a proven pre-existing destination bucket", async () => {
  const ready = fixtureClient();
  ready.createBucket = async () => ({ status: 409 });
  await store(ready).value.ensureBucket();
  const missing = fixtureClient();
  missing.createBucket = async () => ({ status: 409 });
  missing.listPrefix = async () => ({ status: 403 });
  await assert.rejects(store(missing).value.ensureBucket(), /returned 409/);
});

test("live migration listing rejects keys outside the exact source prefix", async () => {
  const client = fixtureClient();
  client.request = async () => ({ status: 200, headers: {}, body: Buffer.from("<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>another-run/fleet/a</Key><Size>5</Size></Contents></ListBucketResult>") });
  await assert.rejects(store(client).value.list(), /escaped the exact run prefix/);
});

test("live migration listing rejects duplicate keys and pagination loops", async () => {
  const duplicate = fixtureClient();
  duplicate.request = async (_method, _key, options) => ({
    status: 200,
    headers: {},
    body: Buffer.from(options.query["continuation-token"]
      ? "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>qualification/run-001/fleet/a</Key></Contents></ListBucketResult>"
      : "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>page-2</NextContinuationToken><Contents><Key>qualification/run-001/fleet/a</Key></Contents></ListBucketResult>"),
  });
  await assert.rejects(store(duplicate).value.list(), /duplicated a key/);

  const looping = fixtureClient();
  looping.request = async () => ({ status: 200, headers: {}, body: Buffer.from("<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>same-page</NextContinuationToken></ListBucketResult>") });
  await assert.rejects(store(looping).value.list(), /repeated its continuation token/);
});

test("destination qualification raw evidence must contain every round and denial exactly once", () => {
  const rows = [
    { family: "create", round: 0 }, { family: "create", round: 1 },
    { family: "overwrite", round: 0 },
    ...["invalid_identity", "expired_identity", "wrong_bucket", "cross_bucket"].map((value) => ({ family: "denial", class: value })),
  ];
  assert.deepEqual(summarizeDestinationQualificationRows(rows, { create_rounds: 2, overwrite_rounds: 1 }), { create_rows: 2, overwrite_rows: 1, denial_rows: 4, total_rows: 7 });
  assert.throws(() => summarizeDestinationQualificationRows(rows.slice(1), { create_rounds: 2, overwrite_rounds: 1 }), /incomplete or duplicated/);
  assert.throws(() => summarizeDestinationQualificationRows([...rows, rows[0]], { create_rounds: 2, overwrite_rounds: 1 }), /incomplete or duplicated/);
});

test("live migration journal is protected, hash chained, and fail-closed after an incomplete plan", () => {
  const runId = `test-${randomUUID()}`, destinationRunId = `test-${randomUUID()}`;
  const root = `/dev/shm/agentic-celld-storage/${runId}`, path = `${root}/migration-journal.json`;
  mkdirSync(root, { recursive: true, mode: 0o700 });
  try {
    let tick = 0;
    const journal = new LiveMigrationJournal(path, { runId, destinationRunId, now: () => new Date(1_800_000_000_000 + tick++ * 1_000) });
    const plan = journal.plan({ phase: "forward_copy", mutation: "synchronize_namespace", details: { objects: 2 } });
    assert.equal(plan.status, "planned");
    assert.throws(() => journal.plan({ phase: "cutover", mutation: "activate", details: {} }), /incomplete prior mutation/);
    const completion = journal.complete(plan.id, { result: { objects: 2 }, result_sha256: "a".repeat(64) });
    assert.equal(completion.plan_id, plan.id);
    const evidence = journal.evidence();
    assert.deepEqual(evidence.incomplete_plan_ids, []);
    assert.equal(evidence.entries.length, 2);
    assert.equal(evidence.entries[1].previous_entry_sha256, evidence.entries[0].entry_sha256);
    assert.match(evidence.journal_sha256, /^[0-9a-f]{64}$/);
    const tampered = JSON.parse(readFileSync(path, "utf8"));
    tampered.entries[0].details.objects = 3;
    writeFileSync(path, `${JSON.stringify(tampered)}\n`, { mode: 0o600 });
    assert.throws(() => journal.evidence(), /hash chain/);
  } finally {
    rmSync(root, { recursive: true, force: false });
  }
});

test("migration failure evidence hashes the error and binds retained namespaces to the last journal phase", () => {
  const journal = { entries: [{ phase: "destination_cutover", mutation: "activate_destination" }] };
  const failure = buildMigrationFailureEvidence({
    sourceStorage: { run_id: "source-run", bucket: "source-bucket", run_prefix: "qualification/source-run", backend: { product: "SeaweedFS" } },
    destinationStorage: { bucket: "destination-bucket", run_prefix: "qualification/destination-run", backend: { product: "SeaweedFS" } },
    destinationRunId: "destination-run",
    startedAt: "2026-08-23T09:00:00.000Z",
    endedAt: "2026-08-23T09:01:00.000Z",
    sandboxGit: "1".repeat(40),
    operationError: new Error("sensitive diagnostic text"),
    cleanupErrors: [],
    journalEvidence: journal,
    journalErrorSha256: null,
    retainedNamespaces: true,
  });
  assert.match(failure.error_sha256, /^[0-9a-f]{64}$/);
  assert.equal(JSON.stringify(failure).includes("sensitive diagnostic text"), false);
  assert.equal(failure.last_phase, "destination_cutover");
  assert.equal(failure.last_mutation, "activate_destination");
  assert.equal(failure.retain_namespaces, true);
  assert.match(failure.source_namespace_sha256, /^[0-9a-f]{64}$/);
  assert.match(failure.destination_namespace_sha256, /^[0-9a-f]{64}$/);
});

test("live migration adapter restores the reusable source policy and fails cleanup closed", () => {
  const source = readFileSync(new URL("../../../scripts/celld-live-offline-migration.mjs", import.meta.url), "utf8");
  assert.match(source, /await setBucketWrite\(sourceStorage, true, sourceStore\)/);
  assert.match(source, /celld-offline-migration-error\.json/);
  assert.match(source, /retain_namespaces: retainedNamespaces/);
  assert.match(source, /LiveMigrationJournal/);
  assert.match(source, /observeAuthorities\(\)/);
  assert.match(source, /await deployFleetWorker\(destinationFleetPath\)/);
  assert.ok(source.indexOf("cleanupFleet(destinationFleetPath)") < source.indexOf("setBucketWrite(sourceStorage, true, sourceStore)"));
  assert.match(source, /destinationRoot !== `\/dev\/shm\/agentic-celld-migration\/\$\{destinationRunId\}`/);
  assert.match(source, /sendWorkerCommand\(\{/);
  assert.match(source, /runS3Qualification\(/);
  assert.match(source, /openStorageGatewayAccess\(sourceStorage, \{ services: \["s3gateway1"\] \}\)/);
  assert.match(source, /openStorageGatewayAccess\(destinationStorage, \{ services: \["s3gateway1", "s3gateway2"\] \}\)/);
  assert.match(source, /gatewayEndpoints: destinationGatewayAccess\.endpoints/);
  assert.doesNotMatch(source, /compose[^\n]*\["port",\s*"s3gateway/);
  assert.match(source, /evaluateStorageEvidence\(destinationQualification\)/);
  assert.doesNotMatch(source, /getWorkerCell/);
  assert.doesNotMatch(source, /local_storage_touched:\s*true/);
});
