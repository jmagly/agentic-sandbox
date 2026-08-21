import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { LiveS3MigrationStore, summarizeDestinationQualificationRows } from "../../../scripts/celld-live-offline-migration.mjs";

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

test("live migration adapter restores the reusable source policy and fails cleanup closed", () => {
  const source = readFileSync(new URL("../../../scripts/celld-live-offline-migration.mjs", import.meta.url), "utf8");
  assert.match(source, /await setBucketWrite\(sourceStorage, true, sourceStore\)/);
  assert.match(source, /if \(cleanupErrors\.length\) throw new Error/);
  assert.ok(source.indexOf("cleanupFleet(destinationFleetPath)") < source.indexOf("setBucketWrite(sourceStorage, true, sourceStore)"));
  assert.match(source, /destinationRoot !== `\/dev\/shm\/agentic-celld-migration\/\$\{destinationRunId\}`/);
  assert.match(source, /sendWorkerCommand\(\{/);
  assert.match(source, /runS3Qualification\(/);
  assert.match(source, /allGateways: true/);
  assert.match(source, /evaluateStorageEvidence\(destinationQualification\)/);
  assert.doesNotMatch(source, /getWorkerCell/);
  assert.doesNotMatch(source, /local_storage_touched:\s*true/);
});
