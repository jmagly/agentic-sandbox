import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { collectFixtureDiagnostics, FIXTURE_PROFILES, prepareFixture, startFixture, validateFixtureConfig } from "../../../scripts/celld-seaweedfs-fixture.mjs";
import { formatStorageDriverFailure, publishedGatewayEndpoint } from "../../../scripts/celld-live-storage-topology.mjs";
import { runS3Qualification } from "../../../scripts/celld-storage-race-runner.mjs";
import { STORAGE_PROFILE_SCHEMA } from "../../../scripts/celld-storage-qualifier.mjs";

function response(status, { code, body = "", etag } = {}) {
  return { status, error_code: code, body: Buffer.from(body), headers: etag ? { etag } : {}, duration_ms: 1 };
}

function fakeClients(profile) {
  const buckets = new Map();
  let revision = 0;
  const etag = (body) => `\"${Buffer.from(body).toString("hex")}\"`;
  return {
    factory(value) {
      const admin = value.identity_file_ref.includes("admin");
      const revoked = value.identity_file_ref.includes("revoked");
      const allowed = (bucket) => admin || (!revoked && bucket === profile.bucket);
      const objects = (bucket) => buckets.get(bucket);
      return {
        async createBucket(bucket) { if (!admin) return response(403, { code: "AccessDenied" }); buckets.set(bucket, new Map()); return response(200); },
        async deleteBucket(bucket) { if (!admin) return response(403, { code: "AccessDenied" }); buckets.delete(bucket); return response(204); },
        async put(key, body, conditions = {}) {
          if (!allowed(profile.bucket)) return response(403, { code: "AccessDenied" });
          const map = objects(profile.bucket); const current = map.get(key);
          if (conditions.ifNoneMatch === "*" && current) return response(412, { code: "PreconditionFailed" });
          if (conditions.ifMatch && (!current || current.etag !== conditions.ifMatch)) return response(412, { code: "PreconditionFailed" });
          const value = Buffer.from(body); const tag = etag(`${value.toString()}:${revision++}`); map.set(key, { value, etag: tag }); return response(200, { etag: tag });
        },
        async get(key) { const item = objects(profile.bucket)?.get(key); return item ? response(200, { body: item.value, etag: item.etag }) : response(404, { code: "NoSuchKey" }); },
        async head(key) { const item = objects(profile.bucket)?.get(key); return item ? response(200, { etag: item.etag }) : response(404, { code: "NoSuchKey" }); },
        async delete(key) { objects(profile.bucket)?.delete(key); return response(204); },
        async listPrefix() {
          const keys = [...(objects(profile.bucket)?.keys() ?? [])].filter((key) => key.startsWith(`${profile.run_prefix}/`));
          return response(200, { body: `<ListBucketResult>${keys.map((key) => `<Key>${key}</Key>`).join("")}</ListBucketResult>` });
        },
        async request(_method, key, options) {
          if (!allowed(options.bucket)) return response(403, { code: "AccessDenied" });
          objects(options.bucket)?.set(key, { value: Buffer.from(options.body), etag: etag(options.body) }); return response(200);
        },
        close() {},
      };
    },
  };
}

test("named fixtures preserve the non-promoting protocol boundary and exact Titan topology", () => {
  assert.equal(FIXTURE_PROFILES["single-process-protocol"].promoting, false);
  assert.equal(FIXTURE_PROFILES["single-process-protocol"].gateway_count, 1);
  assert.equal(FIXTURE_PROFILES["titan-single-host-storage"].promoting, true);
  assert.equal(FIXTURE_PROFILES["titan-single-host-storage"].gateway_count, 2);
  assert.match(FIXTURE_PROFILES["titan-single-host-storage"].topology, /3-master,3-rack-volume,3-postgres-filer,2-tls-s3-gateway/);
  const compose = readFileSync(join(process.cwd(), "deploy/celld/qualification/seaweedfs-titan.compose.yml"), "utf8");
  assert.equal((compose.match(/^  master[123]:$/gm) ?? []).length, 3);
  assert.equal((compose.match(/^  volume[123]:$/gm) ?? []).length, 3);
  assert.equal((compose.match(/^  filer[123]:$/gm) ?? []).length, 3);
  assert.equal((compose.match(/^  s3gateway[12]:$/gm) ?? []).length, 2);
  assert.match(compose, /defaultReplicaPlacement=010/);
  assert.match(compose, /cap_drop:\n    - ALL\n(?:.*\n){3}  cap_add:\n    - SETGID\n    - SETUID/);
  assert.equal((compose.match(/^    - SET(?:GID|UID)$/gm) ?? []).length, 2);
  assert.equal((compose.match(/^      - target: 8334$/gm) ?? []).length, 2);
  assert.equal((compose.match(/^        published: "49152-65535"$/gm) ?? []).length, 2);
  assert.equal((compose.match(/^        host_ip: 127\.0\.0\.1$/gm) ?? []).length, 2);
  assert.equal((compose.match(/^        protocol: tcp$/gm) ?? []).length, 2);
  assert.equal((compose.match(/^        mode: host$/gm) ?? []).length, 2);
  assert.doesNotMatch(compose, /127\.0\.0\.1::8334/);
  assert.doesNotMatch(compose, /chrislusf\/seaweedfs:(?:latest|4\.)/);
});

test("Titan storage topology gates each dependent tier on SeaweedFS health", () => {
  const compose = readFileSync(join(process.cwd(), "deploy/celld/qualification/seaweedfs-titan.compose.yml"), "utf8");
  const serviceBlock = (name) => {
    const match = compose.match(new RegExp(`^  ${name}:\\n[\\s\\S]*?(?=^  [a-zA-Z0-9_-]+:\\n|^networks:\\n)`, "m"));
    assert.ok(match, `missing ${name} service`);
    return match[0];
  };
  for (const name of ["master1", "master2", "master3"]) {
    assert.match(serviceBlock(name), /healthcheck:[\s\S]*\/cluster\/status/);
  }
  for (const name of ["volume1", "volume2", "volume3"]) {
    const block = serviceBlock(name);
    assert.match(block, /healthcheck:[\s\S]*127\.0\.0\.1:8080\/status/);
    for (const dependency of ["master1", "master2", "master3"]) {
      assert.match(block, new RegExp(`${dependency}:\\n        condition: service_healthy`));
    }
  }
  for (const name of ["filer1", "filer2", "filer3"]) {
    const block = serviceBlock(name);
    assert.match(block, /healthcheck:[\s\S]*127\.0\.0\.1:8888\/"/);
    for (const dependency of ["postgres", "volume1", "volume2", "volume3"]) {
      assert.match(block, new RegExp(`${dependency}:\\n        condition: service_healthy`));
    }
  }
  for (const name of ["filer2", "filer3"]) {
    assert.match(serviceBlock(name), /filer1:\n        condition: service_healthy/);
  }
  for (const name of ["s3gateway1", "s3gateway2"]) {
    const block = serviceBlock(name);
    for (const dependency of ["filer1", "filer2", "filer3"]) {
      assert.match(block, new RegExp(`${dependency}:\\n        condition: service_healthy`));
    }
  }
});

test("dedicated storage qualification is manual, Titan-only, serialized, and evidence-retaining", () => {
  const workflow = readFileSync(join(process.cwd(), ".gitea/workflows/celld-storage-qualification.yml"), "utf8");
  assert.match(workflow, /on:\n  workflow_dispatch:/);
  assert.doesNotMatch(workflow, /^  (?:push|pull_request):/m);
  assert.match(workflow, /runs-on: titan/);
  assert.match(workflow, /group: agentic-sandbox-vm-e2e/);
  assert.match(workflow, /create_rounds|UAT-CELLD-010/);
  assert.match(workflow, /CELLD_STORAGE_EVIDENCE/);
  assert.match(workflow, /stderr_sha256: \.command\.stderr_sha256/);
  assert.doesNotMatch(workflow, /cat [^\n]*evidence\.jsonl/);
  assert.match(workflow, /retention-days: 90/);
});

test("live storage failures expose only a bounded stage, cleanup result, and digest", () => {
  const failure = new Error("secret=do-not-print target bucket create returned 403");
  failure.stage = "storage-measurement";
  failure.cleanupStatus = "passed";
  const diagnostic = formatStorageDriverFailure(failure);
  assert.match(diagnostic, /^CELLD_STORAGE_DRIVER_ERROR stage=storage-measurement reason=unclassified cleanup=passed cause_sha256=[0-9a-f]{64}$/);
  assert.doesNotMatch(diagnostic, /do-not-print|target bucket|403/);

  const untrusted = new Error("password=also-hidden");
  untrusted.stage = "bad\nstage";
  untrusted.cleanupStatus = "invented";
  assert.match(formatStorageDriverFailure(untrusted), /^CELLD_STORAGE_DRIVER_ERROR stage=unclassified reason=unclassified cleanup=unknown cause_sha256=[0-9a-f]{64}$/);
});

test("live storage gateway discovery uses one Docker-inspected loopback publisher", () => {
  const row = { NetworkSettings: { Ports: { "8334/tcp": [{ HostIp: "127.0.0.1", HostPort: "49153" }] } } };
  assert.equal(publishedGatewayEndpoint(JSON.stringify([row]), "s3gateway1"), "https://127.0.0.1:49153");
  assert.throws(
    () => publishedGatewayEndpoint(JSON.stringify([{ NetworkSettings: { Ports: { "8334/tcp": [{ HostIp: "0.0.0.0", HostPort: "49153" }] } } }]), "s3gateway1"),
    (error) => error.message === "could not resolve s3gateway1 TLS port" && error.reasonCode === "gateway-binding-not-loopback",
  );
  assert.throws(
    () => publishedGatewayEndpoint(JSON.stringify([{ NetworkSettings: { Ports: { "8334/tcp": [
      { HostIp: "127.0.0.1", HostPort: "49153" },
      { HostIp: "127.0.0.1", HostPort: "49154" },
    ] } } }]), "s3gateway1"),
    (error) => error.message === "could not resolve s3gateway1 TLS port" && error.reasonCode === "gateway-mapping-ambiguous",
  );
  assert.throws(
    () => publishedGatewayEndpoint(JSON.stringify([{ NetworkSettings: { Ports: {} } }]), "s3gateway1"),
    (error) => error.message === "could not resolve s3gateway1 TLS port" && error.reasonCode === "gateway-mapping-unavailable",
  );
  assert.throws(
    () => publishedGatewayEndpoint("not-json", "s3gateway1"),
    (error) => error.message === "could not resolve s3gateway1 TLS port" && error.reasonCode === "gateway-service-unavailable",
  );
});

test("fixture preparation creates an unpredictable bucket and only protected secret-bearing files", () => {
  const parent = mkdtempSync(join(tmpdir(), "celld-seaweed-test-"));
  const runId = "fixture-run-001";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
    assert.deepEqual(validateFixtureConfig(config), []);
    assert.match(config.bucket, /^celld-[a-f0-9]{24}$/);
    assert.equal(config.promoting, false);
    assert.equal(JSON.stringify(config).includes("aws_secret_access_key"), false);
    for (const name of ["identity", "identity-admin", "identity-revoked", "access-key", "secret-key", "postgres-password", "s3.json", "filer.toml"]) {
      assert.equal(statSync(join(root, name)).mode & 0o077, 0, name);
    }
    assert.match(readFileSync(join(root, "s3.json"), "utf8"), new RegExp(`Read:${config.bucket}`));
    const tampered = structuredClone(config); tampered.backend.artifact_sha256 = "0".repeat(64);
    assert.ok(validateFixtureConfig(tampered).some((error) => error.includes("reviewed profile")));
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("storage start requires every exact profile service after bounded Compose startup", () => {
  const parent = mkdtempSync(join(tmpdir(), "celld-seaweed-start-"));
  const runId = "fixture-start-001";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
    const calls = [];
    const runner = (_program, args, options) => {
      calls.push({ args, options });
      if (args.includes("ps")) return "seaweedfs";
      return "";
    };
    assert.deepEqual(startFixture(config, { runner }), {
      status: "READY",
      run_id: runId,
      fixture_profile: "single-process-protocol",
      scope: "fixture_reduced",
      services: ["seaweedfs"],
    });
    assert.deepEqual(calls.map((call) => call.args.slice(5)), [
      ["pull", "--quiet"],
      ["up", "-d", "--wait", "--wait-timeout", "240"],
      ["ps", "--services", "--status", "running"],
    ]);
    assert.ok(calls.every((call) => call.options.env.CELLD_SEAWEED_RUN_ROOT === root));
    assert.throws(() => startFixture(config, { runner: () => "" }), /services are not running/);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("startup diagnostics select failed services, bound logs, and redact fixture credentials", () => {
  const parent = mkdtempSync(join(tmpdir(), "celld-seaweed-diagnostics-"));
  const runId = "fixture-diagnostics-001";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
    const secret = readFileSync(join(root, "secret-key"), "utf8").trim();
    const runner = (_program, args) => {
      if (args.includes("ps")) return [
        JSON.stringify({ Service: "waiting-service", State: "created", Health: "", ExitCode: 0 }),
        JSON.stringify({ Service: "seaweedfs", State: "exited", Health: "", ExitCode: 1 }),
      ].join("\n");
      if (args.includes("logs")) return `startup failed secret=${secret}`;
      throw new Error(`unexpected command: ${args.join(" ")}`);
    };
    const result = collectFixtureDiagnostics(config, { runner });
    assert.deepEqual(result.affected_services, ["seaweedfs"]);
    assert.equal(result.services[1].ExitCode, 1);
    assert.equal(result.logs, "startup failed secret=[REDACTED]");
    assert.equal(result.truncated, false);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("provider-neutral race runner derives exact outcomes, cross-gateway reads, denials, and cleanup", async () => {
  const profile = {
    schema_version: STORAGE_PROFILE_SCHEMA,
    profile_id: "fake-storage",
    run_id: "fake-run-001",
    dialect: "s3-v1",
    scope: "fixture_reduced",
    endpoint: "http://127.0.0.1:9000",
    region: "us-east-1",
    addressing_mode: "path",
    bucket: "celld-fake-run-001",
    run_prefix: "qualification/fake-run-001",
    identity_file_ref: "/run/fake/identity",
    backend: { product: "fake", version: "1", artifact_sha256: "a".repeat(64), configuration_sha256: "b".repeat(64), gateway_endpoints: ["http://127.0.0.1:9000", "http://127.0.0.1:9001"], topology: "fake-two-gateway" },
    limits: { create_rounds: 3, overwrite_rounds: 3, contenders: 2, warmups: 1, max_workers: 2, max_connections: 4 },
  };
  const fake = fakeClients(profile);
  const rawRows = [];
  const evidence = await runS3Qualification(profile, { adminIdentityFileRef: "/run/fake/admin", revokedIdentityFileRef: "/run/fake/revoked", rawRows, clientFactory: fake.factory });
  assert.deepEqual(evidence.races.create, { rounds: 3, commits: 3, conditional_losers: 3, ambiguous: 0, unknown: 0, total_outcomes: 6 });
  assert.deepEqual(evidence.races.overwrite, { rounds: 3, commits: 3, conditional_losers: 3, ambiguous: 0, unknown: 0, total_outcomes: 6 });
  assert.deepEqual(evidence.reads, { first_read_mismatches: 0, hash_mismatches: 0, gateways_observed: 2 });
  assert.equal(Object.values(evidence.denials).every((value) => value === 1), true);
  assert.deepEqual(evidence.cleanup, { created_keys: 6, deleted_keys: 6, enumerated_remaining: 0, run_prefix_absent: true });
  assert.equal(rawRows.filter((row) => row.family === "create").length, 3);
  assert.equal(rawRows.filter((row) => row.family === "overwrite").length, 3);
});
