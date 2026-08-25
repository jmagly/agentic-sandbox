import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { createConnection, createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { cleanupFixture, collectFixtureDiagnostics, FixtureCleanupError, FIXTURE_PROFILES, prepareFixture, startFixture, validateFixtureConfig } from "../../../scripts/celld-seaweedfs-fixture.mjs";
import { closeGatewayForwarders, inspectGatewayTarget, isPrivateIpv4, openStorageGatewayAccess, startLoopbackForwarder } from "../../../scripts/celld-storage-gateway-access.mjs";
import { formatStorageDriverFailure } from "../../../scripts/celld-live-storage-topology.mjs";
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
  assert.doesNotMatch(compose, /^    ports:/m);
  assert.match(compose, /storage-private:\n    internal: true/);
  assert.equal((compose.match(/dev\.agentic-sandbox\.run: \$\{CELLD_SEAWEED_RUN_ID:\?set CELLD_SEAWEED_RUN_ID\}/g) ?? []).length, 2);
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
  assert.match(workflow, /concurrency:\n  # [\s\S]*?group: agentic-sandbox-celld-qualification-titan/);
  assert.match(workflow, /group: agentic-sandbox-vm-e2e/);
  assert.match(workflow, /create_rounds|UAT-CELLD-010/);
  assert.match(workflow, /node --test --test-concurrency=10/);
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

function gatewayDocker(config, {
  service = "s3gateway1",
  address = "172.24.0.12",
  extraNetwork = false,
  published = false,
  internal = true,
  runLabel = config.run_id,
  networkRunLabel = runLabel,
} = {}) {
  const containerId = "a".repeat(64);
  const networkId = "b".repeat(64);
  const networkName = `${config.project}_storage-private`;
  const containerName = `${config.project}-${service}-1`;
  const container = [{
    Id: containerId,
    Name: `/${containerName}`,
    Config: { Labels: {
      "com.docker.compose.project": config.project,
      "com.docker.compose.service": service,
      "dev.agentic-sandbox.run": runLabel,
      "dev.agentic-sandbox.scope": "celld-qualification",
    } },
    HostConfig: { PortBindings: published ? { "8334/tcp": [{ HostIp: "127.0.0.1", HostPort: "64152" }] } : {} },
    NetworkSettings: {
      Ports: { "8334/tcp": published ? [{ HostIp: "127.0.0.1", HostPort: "64152" }] : null },
      Networks: {
        [networkName]: { NetworkID: networkId, IPAddress: address, Aliases: [service, containerName] },
        ...(extraNetwork ? { bridge: { NetworkID: "c".repeat(64), IPAddress: "172.25.0.2", Aliases: [service] } } : {}),
      },
    },
  }];
  const network = [{
    Id: networkId,
    Name: networkName,
    Driver: "bridge",
    Scope: "local",
    Internal: internal,
    Ingress: false,
    Labels: {
      "com.docker.compose.project": config.project,
      "com.docker.compose.network": "storage-private",
      "dev.agentic-sandbox.run": networkRunLabel,
      "dev.agentic-sandbox.scope": "celld-qualification",
    },
    Containers: { [containerId]: { Name: containerName, IPv4Address: `${address}/16` } },
  }];
  return (program, args) => {
    assert.equal(program, "docker");
    if (args.includes("compose")) return containerId;
    if (args[0] === "inspect") return JSON.stringify(container);
    if (args[0] === "network" && args[1] === "inspect") return JSON.stringify(network);
    throw new Error(`unexpected Docker call: ${args.join(" ")}`);
  };
}

test("live storage gateway discovery accepts only one unpublished run-owned private target", () => {
  const parent = mkdtempSync("/dev/shm/celld-gateway-access-");
  const runId = "gateway-access-001";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
    assert.deepEqual(inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config) }), { host: "172.24.0.12", port: 8334 });
    assert.throws(
      () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { published: true }) }),
      (error) => error.reasonCode === "gateway-unexpected-publication",
    );
    assert.throws(
      () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { extraNetwork: true }) }),
      (error) => error.reasonCode === "gateway-network-ambiguous",
    );
    for (const address of ["203.0.113.12", "10..2.3", "172.016.0.2"]) {
      assert.throws(
        () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { address }) }),
        (error) => error.reasonCode === "gateway-private-address-unavailable",
      );
    }
    assert.throws(
      () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { internal: false }) }),
      (error) => error.reasonCode === "gateway-network-ownership-invalid",
    );
    assert.throws(
      () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { runLabel: "foreign-run" }) }),
      (error) => error.reasonCode === "gateway-ownership-invalid",
    );
    assert.throws(
      () => inspectGatewayTarget(config, "s3gateway1", { runner: gatewayDocker(config, { networkRunLabel: "foreign-run" }) }),
      (error) => error.reasonCode === "gateway-network-ownership-invalid",
    );
    assert.equal(isPrivateIpv4("10..2.3"), false);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("protected gateway access reaps an earlier forwarder when a later gateway fails", async () => {
  const parent = mkdtempSync("/dev/shm/celld-gateway-access-");
  const runId = "gateway-access-002";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
    let service = "s3gateway1";
    const runner = (program, args, options) => {
      if (args.includes("compose")) service = args.at(-1);
      return gatewayDocker(config, { service, address: service === "s3gateway1" ? "172.24.0.12" : "172.24.0.13" })(program, args, options);
    };
    const closed = [];
    let opened = 0;
    await assert.rejects(
      openStorageGatewayAccess(config, {
        services: ["s3gateway1", "s3gateway2"],
        runner,
        forwarderFactory: async () => {
          opened += 1;
          if (opened === 2) throw new Error("injected second gateway failure");
          return { endpoint: "https://127.0.0.1:64152", async close() { closed.push("s3gateway1"); } };
        },
      }),
      /injected second gateway failure/,
    );
    assert.deepEqual(closed, ["s3gateway1"]);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("live storage loopback forwarder relays bytes and closes idempotently", async () => {
  const upstream = createServer((socket) => socket.pipe(socket));
  await new Promise((resolveListen, reject) => {
    upstream.once("error", reject);
    upstream.listen({ host: "127.0.0.1", port: 0, exclusive: true }, resolveListen);
  });
  const address = upstream.address();
  assert.ok(address && typeof address !== "string");
  const forwarder = await startLoopbackForwarder({ host: "127.0.0.1", port: address.port });
  try {
    const endpoint = new URL(forwarder.endpoint);
    assert.equal(endpoint.hostname, "127.0.0.1");
    const reply = await new Promise((resolveReply, reject) => {
      const socket = createConnection({ host: endpoint.hostname, port: Number(endpoint.port) });
      socket.setTimeout(2_000, () => socket.destroy(new Error("forwarder test timed out")));
      socket.once("error", reject);
      socket.once("connect", () => socket.write("qualification"));
      socket.once("data", (bytes) => { resolveReply(bytes.toString("utf8")); socket.destroy(); });
    });
    assert.equal(reply, "qualification");
  } finally {
    await forwarder.close();
    await forwarder.close();
    await new Promise((resolveClose, reject) => upstream.close((error) => error ? reject(error) : resolveClose()));
  }
});

test("live storage loopback forwarder destroys active downstream and upstream sockets", async () => {
  let acceptConnection;
  const accepted = new Promise((resolveAccepted) => { acceptConnection = resolveAccepted; });
  const upstream = createServer((socket) => acceptConnection(socket));
  await new Promise((resolveListen, reject) => {
    upstream.once("error", reject);
    upstream.listen({ host: "127.0.0.1", port: 0, exclusive: true }, resolveListen);
  });
  const address = upstream.address();
  assert.ok(address && typeof address !== "string");
  const forwarder = await startLoopbackForwarder({ host: "127.0.0.1", port: address.port });
  const endpoint = new URL(forwarder.endpoint);
  const downstream = createConnection({ host: endpoint.hostname, port: Number(endpoint.port) });
  const connected = new Promise((resolveConnect, reject) => {
    downstream.once("connect", resolveConnect);
    downstream.once("error", reject);
  });
  const upstreamSocket = await accepted;
  await connected;
  const downstreamClosed = new Promise((resolveClose) => downstream.once("close", resolveClose));
  const upstreamClosed = new Promise((resolveClose) => upstreamSocket.once("close", resolveClose));
  try {
    await forwarder.close();
    await Promise.all([downstreamClosed, upstreamClosed]);
    assert.equal(downstream.destroyed, true);
    assert.equal(upstreamSocket.destroyed, true);
  } finally {
    downstream.destroy();
    upstreamSocket.destroy();
    await new Promise((resolveClose, reject) => upstream.close((error) => error ? reject(error) : resolveClose()));
  }
});

test("live storage loopback forwarder treats refused upstream as request failure, not cleanup failure", async () => {
  const reservation = createServer();
  await new Promise((resolveListen, reject) => {
    reservation.once("error", reject);
    reservation.listen({ host: "127.0.0.1", port: 0, exclusive: true }, resolveListen);
  });
  const address = reservation.address();
  assert.ok(address && typeof address !== "string");
  await new Promise((resolveClose, reject) => reservation.close((error) => error ? reject(error) : resolveClose()));
  const forwarder = await startLoopbackForwarder({ host: "127.0.0.1", port: address.port });
  const endpoint = new URL(forwarder.endpoint);
  await new Promise((resolveAttempt) => {
    const socket = createConnection({ host: endpoint.hostname, port: Number(endpoint.port) });
    socket.setTimeout(2_000, () => socket.destroy());
    socket.on("error", () => {});
    socket.on("close", resolveAttempt);
  });
  await assert.doesNotReject(forwarder.close());
  await assert.doesNotReject(forwarder.close());

  const order = [];
  await assert.rejects(
    closeGatewayForwarders([
      { async close() { order.push("first"); throw new Error("first failed"); } },
      { async close() { order.push("second"); throw new Error("second failed"); } },
    ]),
    (error) => error.reasonCode === "gateway-forwarder-cleanup-failed",
  );
  assert.deepEqual(order, ["second", "first"]);
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
    assert.ok(calls.every((call) => call.options.env.CELLD_SEAWEED_RUN_ROOT === root && call.options.env.CELLD_SEAWEED_RUN_ID === runId));
    assert.throws(() => startFixture(config, { runner: () => "" }), /services are not running/);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("fixture cleanup proves exact project absence and removes only the run root", () => {
  const parent = mkdtempSync(join(tmpdir(), "celld-seaweed-cleanup-"));
  const runId = "fixture-cleanup-001";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
    const calls = [];
    const runner = (program, args) => { calls.push({ program, args }); return ""; };
    assert.deepEqual(cleanupFixture(config, { runner }), {
      status: "PASS",
      run_id: runId,
      project: config.project,
      compose_residue: [],
      run_root_removed: true,
    });
    assert.equal(existsSync(root), false);
    assert.deepEqual(calls.map((call) => call.args[0]), ["compose", "ps", "network", "volume"]);
    assert.ok(calls.slice(1).every((call) => call.args.includes(`label=com.docker.compose.project=${config.project}`)));
    assert.equal(existsSync(parent), true);
  } finally { rmSync(parent, { recursive: true, force: true }); }
});

test("fixture cleanup reports command, sweep, and retained-resource failures as exit 4", () => {
  const cases = [
    { id: "down", runner: (_program, args) => { if (args[0] === "compose") throw new Error("down failed"); return ""; } },
    { id: "sweep", runner: (_program, args) => { if (args[0] === "network") throw new Error("sweep failed"); return ""; } },
    { id: "residue", runner: (_program, args) => args[0] === "volume" ? "owned-volume" : "" },
  ];
  for (const entry of cases) {
    const parent = mkdtempSync(join(tmpdir(), `celld-seaweed-cleanup-${entry.id}-`));
    const runId = `fixture-cleanup-${entry.id}`;
    const root = join(parent, runId);
    try {
      const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
      assert.throws(
        () => cleanupFixture(config, { runner: entry.runner }),
        (error) => error instanceof FixtureCleanupError && error.exitCode === 4,
      );
      assert.equal(existsSync(root), true);
    } finally { rmSync(parent, { recursive: true, force: true }); }
  }
});

test("fixture cleanup treats unsafe ownership preconditions as exit 4 without Docker access", () => {
  const parent = mkdtempSync(join(tmpdir(), "celld-seaweed-cleanup-precondition-"));
  const runId = "fixture-cleanup-precondition";
  const root = join(parent, runId);
  try {
    const config = prepareFixture({ fixtureProfile: "single-process-protocol", runId, root });
    let runnerTouched = false;
    assert.throws(
      () => cleanupFixture({ ...config, project: "unsafe-project" }, { runner: () => { runnerTouched = true; return ""; } }),
      (error) => error instanceof FixtureCleanupError && error.exitCode === 4,
    );
    assert.equal(runnerTouched, false);
    assert.equal(existsSync(root), true);
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
