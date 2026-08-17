import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  GCS_RESERVED_DIALECT,
  S3_DIALECT,
  S3V1Client,
  STORAGE_EVIDENCE_SCHEMA,
  STORAGE_PROFILE_SCHEMA,
  classifyS3Outcome,
  evaluateStorageEvidence,
  nearestRank,
  resolveStorageProfile,
  signS3Request,
  validateStorageEvidence,
  validateStorageProfile,
} from "../../../scripts/celld-storage-qualifier.mjs";
import { startS3BehaviorFixture } from "./fixtures/s3-behavior-fixture.mjs";

function profile(overrides = {}) {
  return {
    schema_version: STORAGE_PROFILE_SCHEMA,
    profile_id: "storage-test",
    run_id: "run-001",
    dialect: S3_DIALECT,
    scope: "fixture_reduced",
    endpoint: "http://127.0.0.1:9000",
    region: "us-east-1",
    addressing_mode: "path",
    bucket: "celld-run-001",
    run_prefix: "qualification/run-001",
    identity_file_ref: "/run/credentials/storage-test",
    backend: {
      product: "broken-store-fixture",
      version: "test-v1",
      artifact_sha256: "a".repeat(64),
      configuration_sha256: "b".repeat(64),
      gateway_endpoints: ["http://127.0.0.1:9000", "http://127.0.0.1:9001"],
      topology: "one-process-two-gateway-fixture",
    },
    limits: { create_rounds: 3, overwrite_rounds: 3, contenders: 2, warmups: 0, max_workers: 2, max_connections: 4 },
    ...overrides,
  };
}

function evidence({ scope = "fixture_reduced", rounds = 3, samples = 3 } = {}) {
  const contenders = 2;
  return {
    schema_version: STORAGE_EVIDENCE_SCHEMA,
    dialect: S3_DIALECT,
    scope,
    profile_id: "storage-test",
    run_id: "run-001",
    identity: {
      backend_product: "fixture",
      backend_version: "test-v1",
      artifact_sha256: "a".repeat(64),
      configuration_sha256: "b".repeat(64),
      bucket_scope_sha256: "c".repeat(64),
      gateway_endpoints: ["http://127.0.0.1:9000", "http://127.0.0.1:9001"],
      topology: "one-process-two-gateway-fixture",
      gateway_count: 2,
    },
    parameters: { create_rounds: rounds, overwrite_rounds: rounds, contenders, warmups: scope === "live_candidate" ? 100 : 0, max_workers: 2, max_connections: 4 },
    races: {
      create: { rounds, commits: rounds, conditional_losers: rounds, ambiguous: 0, unknown: 0, total_outcomes: rounds * contenders },
      overwrite: { rounds, commits: rounds, conditional_losers: rounds, ambiguous: 0, unknown: 0, total_outcomes: rounds * contenders },
    },
    reads: { first_read_mismatches: 0, hash_mismatches: 0, gateways_observed: 2 },
    latencies_ms: {
      create_winner: Array(samples).fill(100),
      overwrite_winner: Array(samples).fill(110),
      get: Array(samples).fill(80),
      head: Array(samples).fill(70),
    },
    retries: { logical_requests: 100, total_attempts: 100, safety_retries: 0, auth_cases: 4, auth_attempts: 4, max_attempts_per_transient: 1, max_elapsed_ms_per_transient: 100 },
    denials: { invalid_identity_attempts: 1, invalid_identity_denied: 1, expired_identity_attempts: 1, expired_identity_denied: 1, wrong_bucket_attempts: 1, wrong_bucket_denied: 1, cross_bucket_attempts: 1, cross_bucket_denied: 1 },
    broken_store_variants: [
      { id: "ignored-if-none-match", measurements: { unexpected_commits: 1, split_commits: 0, first_read_mismatches: 0, ambiguous: 0, unknown: 0 } },
      { id: "ignored-or-stale-if-match", measurements: { unexpected_commits: 1, split_commits: 0, first_read_mismatches: 0, ambiguous: 0, unknown: 0 } },
      { id: "gateway-local-locking", measurements: { unexpected_commits: 0, split_commits: 1, first_read_mismatches: 0, ambiguous: 0, unknown: 0 } },
      { id: "stale-first-read", measurements: { unexpected_commits: 0, split_commits: 0, first_read_mismatches: 1, ambiguous: 0, unknown: 0 } },
      { id: "misleading-outcome", measurements: { unexpected_commits: 0, split_commits: 0, first_read_mismatches: 0, ambiguous: 1, unknown: 0 } },
    ],
    cleanup: { created_keys: rounds * 2, deleted_keys: rounds * 2, enumerated_remaining: 0, run_prefix_absent: true },
  };
}

test("storage profile is strict, S3-v1 only resolves for enabled Celld, and GCS stays reserved", () => {
  assert.deepEqual(validateStorageProfile(profile()), []);
  const unknown = profile({ access_key: "inline" });
  assert.ok(validateStorageProfile(unknown).some((error) => error.includes("access_key is not allowed")));
  assert.ok(validateStorageProfile(profile({ run_prefix: "qualification/run-001/token=inline" })).some((error) => error.includes("secret-like")));
  assert.ok(validateStorageProfile(profile({ endpoint: "http://store.example.test" })).some((error) => error.includes("HTTPS or loopback")));
  assert.deepEqual(validateStorageProfile(profile({ dialect: GCS_RESERVED_DIALECT })), []);

  assert.equal(resolveStorageProfile({ celldEnabled: false, profilePath: "/does/not/exist" }), null);
  const dir = mkdtempSync(join(tmpdir(), "celld-storage-profile-"));
  try {
    const path = join(dir, "profile.json");
    writeFileSync(path, JSON.stringify(profile()));
    assert.equal(resolveStorageProfile({ celldEnabled: true, profilePath: path }).dialect, S3_DIALECT);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("S3-v1 classifier distinguishes commit, 412, reconciled 409, auth, and unknown", () => {
  assert.equal(classifyS3Outcome({ status: 200 }).kind, "commit");
  assert.equal(classifyS3Outcome({ status: 412, error_code: "PreconditionFailed" }).kind, "conditional_loser");
  const reconciliation = { winner_visible: true, winner_bytes_match: true, current_validator_match: true, losing_bytes_absent: true, winner_sha256: "abc" };
  assert.equal(classifyS3Outcome({ status: 409, error_code: "ConditionalRequestConflict", reconciliation }, { winner_sha256: "abc" }).kind, "conditional_loser");
  assert.equal(classifyS3Outcome({ status: 409, error_code: "ConditionalRequestConflict", reconciliation: { ...reconciliation, losing_bytes_absent: false } }).kind, "ambiguous");
  assert.equal(classifyS3Outcome({ status: 403, error_code: "AccessDenied" }).kind, "auth_denied");
  assert.equal(classifyS3Outcome({ status: 500, error_code: "InternalError" }).kind, "unknown");
  assert.equal(classifyS3Outcome({ status: 412, error_code: "WrongCode" }).kind, "unknown");
});

test("SigV4 client signs conditional requests without putting identity material in URL", async () => {
  const calls = [];
  const credentials = { accessKeyId: "TESTACCESS", secretAccessKey: "test-secret" };
  const client = new S3V1Client(profile(), {
    identityLoader: () => credentials,
    now: () => new Date("2026-08-17T12:34:56Z"),
    requestImpl: async (request) => { calls.push(request); return { status: 412, headers: {}, body: Buffer.from("<Error><Code>PreconditionFailed</Code></Error>") }; },
  });
  const response = await client.put("create/key", "candidate", { ifNoneMatch: "*" });
  assert.equal(response.error_code, "PreconditionFailed");
  assert.equal(calls[0].url, "http://127.0.0.1:9000/celld-run-001/qualification/run-001/create/key");
  assert.equal(calls[0].headers["if-none-match"], "*");
  assert.match(calls[0].headers.authorization, /^AWS4-HMAC-SHA256 Credential=TESTACCESS\//);
  assert.equal(calls[0].url.includes("test-secret"), false);

  const signed = signS3Request({ method: "PUT", url: calls[0].url, headers: { "content-length": "9", "if-none-match": "*" }, body: Buffer.from("candidate"), region: "us-east-1", credentials, now: new Date("2026-08-17T12:34:56Z") });
  assert.equal(signed.authorization, calls[0].headers.authorization);
});

test("identity file reader is reached only by a request and enforces protected metadata", async () => {
  const dir = mkdtempSync(join(tmpdir(), "celld-storage-identity-"));
  try {
    const identity = join(dir, "identity");
    writeFileSync(identity, "[default]\naws_access_key_id = TESTACCESS\naws_secret_access_key = test-secret\n");
    chmodSync(identity, 0o644);
    const client = new S3V1Client(profile({ identity_file_ref: identity }), { requestImpl: async () => ({ status: 200, headers: {}, body: Buffer.alloc(0) }) });
    await assert.rejects(client.head("key"), /0600 or stricter/);
    chmodSync(identity, 0o600);
    assert.equal((await client.head("key")).status, 200);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("reduced local S3 fixture exercises conditional and read semantics without promotion", async () => {
  const fixture = await startS3BehaviorFixture();
  const client = new S3V1Client(profile({ endpoint: fixture.endpoints[0], backend: { ...profile().backend, gateway_endpoints: fixture.endpoints } }), {
    identityLoader: () => ({ accessKeyId: "TESTACCESS", secretAccessKey: "test-secret" }),
  });
  try {
    assert.equal((await client.put("key", "first", { ifNoneMatch: "*" })).status, 200);
    assert.equal(classifyS3Outcome(await client.put("key", "second", { ifNoneMatch: "*" })).kind, "conditional_loser");
    const head = await client.head("key");
    assert.equal((await client.put("key", "third", { ifMatch: head.headers.etag })).status, 200);
    assert.equal(classifyS3Outcome(await client.put("key", "fourth", { ifMatch: head.headers.etag })).kind, "conditional_loser");
    assert.equal((await client.get("key")).body.toString(), "third");
    assert.equal((await client.delete("key")).status, 204);
    assert.equal(evaluateStorageEvidence(evidence()).status, "NOT_RUN");
  } finally { await fixture.close(); }
});

test("five executable broken-store fixtures expose the unsafe behavior", async () => {
  const identityLoader = () => ({ accessKeyId: "TESTACCESS", secretAccessKey: "test-secret" });
  for (const variant of ["ignored-if-none-match", "ignored-or-stale-if-match", "stale-first-read", "misleading-outcome"]) {
    const fixture = await startS3BehaviorFixture(variant);
    const client = new S3V1Client(profile({ endpoint: fixture.endpoints[0], backend: { ...profile().backend, gateway_endpoints: fixture.endpoints } }), { identityLoader });
    try {
      const first = await client.put("key", "first", { ifNoneMatch: "*" });
      if (variant === "ignored-if-none-match") assert.equal((await client.put("key", "second", { ifNoneMatch: "*" })).status, 200);
      if (variant === "ignored-or-stale-if-match") {
        assert.equal(first.status, 200);
        assert.equal((await client.put("key", "second", { ifMatch: '"stale"' })).status, 200);
      }
      if (variant === "stale-first-read") assert.equal((await client.get("key")).status, 404);
      if (variant === "misleading-outcome") {
        assert.equal(classifyS3Outcome(first).kind, "unknown");
        assert.equal((await client.get("key")).body.toString(), "first");
      }
    } finally { await fixture.close(); }
  }

  const local = await startS3BehaviorFixture("gateway-local-locking");
  try {
    const clients = local.endpoints.map((endpoint) => new S3V1Client(profile({ endpoint, backend: { ...profile().backend, gateway_endpoints: local.endpoints } }), { identityLoader }));
    const outcomes = await Promise.all(clients.map((client, index) => client.put("same-key", `writer-${index}`, { ifNoneMatch: "*" })));
    assert.deepEqual(outcomes.map((outcome) => outcome.status), [200, 200]);
  } finally { await local.close(); }
});

test("nearest-rank p99 and exact retry/count formulas are evaluator-owned", () => {
  assert.equal(nearestRank(Array.from({ length: 100 }, (_, index) => index + 1), 0.99), 99);
  const fixture = evidence();
  assert.deepEqual(validateStorageEvidence(fixture), []);
  const result = evaluateStorageEvidence(fixture);
  assert.equal(result.status, "NOT_RUN");
  assert.equal(result.live_qualification, false);

  const amplified = structuredClone(fixture);
  amplified.retries.total_attempts = 301;
  assert.equal(evaluateStorageEvidence(amplified).status, "FAIL");
});

test("a complete 10000-round envelope can pass only in live-candidate scope", () => {
  const live = evidence({ scope: "live_candidate", rounds: 10_000, samples: 10_000 });
  assert.equal(evaluateStorageEvidence(live).status, "PASS");
  live.latencies_ms.head.fill(251, 9_899);
  assert.equal(evaluateStorageEvidence(live).status, "FAIL");
});

test("summary booleans and GCS evidence cannot qualify a backend", () => {
  const summary = evidence();
  summary.qualified = true;
  assert.equal(evaluateStorageEvidence(summary).status, "ERROR");
  const gcs = evidence();
  gcs.dialect = GCS_RESERVED_DIALECT;
  assert.equal(evaluateStorageEvidence(gcs).status, "NOT_RUN");
  assert.equal(evaluateStorageEvidence(gcs).reason_code, "CELLD_STORAGE_GCS_RESERVED");
});

test("all five broken-store behaviors fail the deterministic gate", () => {
  const variants = [
    (value) => { value.races.create.commits += 1; value.races.create.conditional_losers -= 1; },
    (value) => { value.races.overwrite.commits += 1; value.races.overwrite.conditional_losers -= 1; },
    (value) => { value.races.create.total_outcomes -= 1; },
    (value) => { value.reads.first_read_mismatches = 1; },
    (value) => { value.races.overwrite.unknown = 1; },
  ];
  for (const breakStore of variants) {
    const value = evidence();
    breakStore(value);
    assert.equal(evaluateStorageEvidence(value).status, "FAIL");
  }
});
