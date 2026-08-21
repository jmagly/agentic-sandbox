import assert from "node:assert/strict";
import { createHmac, randomUUID } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  CleanupResidueError,
  cleanupFleet,
  deployFleetWorker,
  diagnoseFleet,
  janitorPreview,
  prepareFleet,
  probeFleetWorker,
  startCallbackRelays,
  startFleet,
  validateFleetConfig,
  validateFleetInventory,
} from "../../../scripts/celld-fleet-fixture.mjs";
import { prepareFixture } from "../../../scripts/celld-seaweedfs-fixture.mjs";

const TEST_ROOT = "/dev/shm/agentic-celld-fleet-tests";
const roots = new Set();

test.after(() => {
  for (const root of roots) if (existsSync(root)) rmSync(root, { recursive: true, force: false });
  if (existsSync(TEST_ROOT)) {
    try { rmSync(TEST_ROOT, { recursive: false }); } catch { /* another concurrent test owns entries */ }
  }
});

function fixture(now = new Date()) {
  const runId = `test-${randomUUID()}`;
  const root = join(TEST_ROOT, runId);
  roots.add(root);
  const storage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
  const config = prepareFleet({ storageConfigPath: join(root, "fixture.json"), now });
  return { root, storage, config, configPath: join(root, "fleet.json"), inventoryPath: join(root, "fleet-inventory.json") };
}

class FakeDocker {
  constructor(config, storage) {
    this.config = config;
    this.storage = storage;
    this.containers = new Map();
    this.createCount = 0;
    this.failCreate = null;
    this.forceResidue = false;
    this.onCreate = null;
    this.onRun = null;
    this.failRunLeavesContainer = false;
  }

  labels() {
    return {
      "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
      "dev.agentic-sandbox.workflow": "celld-qualification",
      "dev.agentic-sandbox.run": this.config.run_id,
      "dev.agentic-sandbox.scope": "celld-qualification",
    };
  }

  inspect(name) {
    const container = this.containers.get(name);
    if (!container) throw new Error("not found");
    return JSON.stringify([{ Config: { Labels: container.labels, Image: container.image }, State: { Running: container.running } }]);
  }

  run = (program, args) => {
    assert.equal(program, "docker");
    if (args[0] === "network" && args[1] === "inspect") {
      return JSON.stringify([{ Labels: { "com.docker.compose.project": this.storage.project, "dev.agentic-sandbox.scope": "celld-qualification" }, IPAM: { Config: [{ Gateway: "172.29.0.1" }] } }]);
    }
    if (args[0] === "pull") return this.config.pins.celld.image_ref;
    if (args[0] === "image" && args[1] === "inspect") return "[]";
    if (args[0] === "inspect") return this.inspect(args[1]);
    if (args[0] === "run") {
      const name = args[args.indexOf("--name") + 1];
      this.onRun?.(args);
      if (this.failRunLeavesContainer) {
        this.containers.set(name, { labels: this.labels(), image: this.config.pins.celld.image_ref, running: false });
        throw new Error("injected deploy interruption");
      }
      return "deployment committed";
    }
    if (args[0] === "create") {
      this.createCount += 1;
      const name = args[args.indexOf("--name") + 1];
      this.onCreate?.(name);
      if (this.failCreate === this.createCount) throw new Error("injected create failure");
      if (name.endsWith("-callback-relay")) {
        assert.ok(args.includes(`container:${name.slice(0, -"-callback-relay".length)}`));
        assert.ok(args.includes("/usr/local/bin/agentic-celld-callback-relay"));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.relay_client_cert_file_ref},dst=/run/tls/client.crt,readonly`));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.relay_client_key_file_ref},dst=/run/tls/client.key,readonly`));
        assert.ok(args.includes("172.29.0.1:8122"));
        assert.ok(!args.join(" ").includes(readFileSync(this.config.callback.relay_client_key_file_ref, "utf8").trim()));
        this.containers.set(name, { labels: this.labels(), image: this.config.pins.celld.image_ref, running: false });
        return name;
      }
      assert.ok(args.includes(this.config.pins.celld.image_ref));
      assert.ok(args.includes("AWS_SHARED_CREDENTIALS_FILE=/run/identity/credentials"));
      assert.ok(args.includes(`type=bind,src=${this.storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
      assert.ok(args.includes(`type=bind,src=${this.config.worker_vars_file_ref},dst=/run/worker/vars,readonly`));
      assert.ok(args.includes("CELLD_VARS_FILE=/run/worker/vars"));
      assert.ok(!args.some((value) => value.includes("/tls,dst=/run/tls")));
      assert.ok(!args.join(" ").includes(readFileSync(this.storage.secret_key_file_ref ?? join(this.storage.run_root, "secret-key"), "utf8").trim()));
      const workerAuthKey = /^CELL_AUTH_KEY=(.+)$/m.exec(readFileSync(this.config.worker_vars_file_ref, "utf8"))[1];
      assert.ok(!args.join(" ").includes(workerAuthKey));
      this.containers.set(name, { labels: this.labels(), image: this.config.pins.celld.image_ref, running: false });
      return name;
    }
    if (args[0] === "start") {
      this.containers.get(args[1]).running = true;
      return args[1];
    }
    if (args[0] === "port") {
      const index = this.config.nodes.findIndex((node) => node.name === args[1]);
      return `127.0.0.1:${18080 + index}`;
    }
    if (args[0] === "exec") {
      assert.equal(args[2], "/usr/local/bin/celld");
      assert.equal(args[3], "diagnose");
      assert.equal(args.filter((value) => value === "--peer").length, 3);
      return "ok bucket conditional write\nok peer node-1\nok peer node-2\nok peer node-3";
    }
    if (args[0] === "rm") {
      this.containers.delete(args.at(-1));
      return args.at(-1);
    }
    if (args[0] === "ps") return this.forceResidue ? "unexpected-residue" : [...this.containers.keys()].join("\n");
    throw new Error(`unexpected fake Docker command: ${args.join(" ")}`);
  };
}

test("fleet preparation fixes three exact addressed nodes and one reserve", () => {
  const { config, storage, inventoryPath } = fixture();
  assert.deepEqual(validateFleetConfig(config), []);
  assert.equal(config.scope, "single-host multi-node");
  assert.equal(config.nodes.length, 3);
  assert.equal(new Set(config.nodes.map((node) => node.name)).size, 3);
  assert.deepEqual(config.nodes.map((node) => node.role), ["active", "active", "reserve"]);
  assert.deepEqual(config.operator_commands, ["prepare", "deploy", "start", "start-relays", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"]);
  assert.equal(config.pins.celld.manifest_digest, "sha256:8634eac20f69ffe99103d403b985c0afd43fd970badadd01435f297ba0df797a");
  assert.equal(config.pins.worker_digest, "sha256:97ba7bb98beb18d007e471d8bd731006d29f5c35c3c7829ee27c71ba0d487716");
  assert.equal(config.network.public_publish, "127.0.0.1::8080");
  assert.equal(config.worker_vars_file_ref, join(config.run_root, "fleet/worker-vars"));
  assert.equal(lstatSync(config.worker_vars_file_ref).mode & 0o077, 0);
  const workerVars = readFileSync(config.worker_vars_file_ref, "utf8");
  assert.match(workerVars, /^CELL_AUTH_KEY_ID=run-[a-f0-9]{20}$/m);
  assert.match(workerVars, /^CELL_AUTH_KEY=[A-Za-z0-9_-]{43}$/m);
  assert.match(workerVars, /^MANAGEMENT_URL=http:\/\/127\.0\.0\.1:8125\/$/m);
  assert.ok(!JSON.stringify(config).includes(/^CELL_AUTH_KEY=(.+)$/m.exec(workerVars)[1]));
  assert.equal(config.callback.client_cn, "agentic-celld-worker-callback");
  assert.equal(config.callback.management_tls_port, 8122);
  assert.equal(lstatSync(config.callback.management_auth_key_file_ref).mode & 0o077, 0);
  assert.equal(readFileSync(config.callback.management_auth_key_file_ref, "utf8"), /^CELL_AUTH_KEY=(.+)$/m.exec(workerVars)[1]);
  const tlsExtensions = readFileSync(join(config.run_root, "tls/server-ext.cnf"), "utf8");
  assert.match(tlsExtensions, /DNS:s3gateway1/);
  assert.match(tlsExtensions, /DNS:s3gateway2/);
  assert.match(readFileSync(join(config.run_root, "management-tls/management-server-ext.cnf"), "utf8"), /DNS:management\.internal/);
  assert.match(readFileSync(join(config.run_root, "management-tls/callback-client-ext.cnf"), "utf8"), /clientAuth/);
  for (const name of [
    "management-server.key", "management-server.csr", "management-server.crt", "management-server-ext.cnf",
    "callback-client.key", "callback-client.csr", "callback-client.crt", "callback-client-ext.cnf",
  ]) assert.equal(lstatSync(join(config.run_root, "management-tls", name)).mode & 0o077, 0, name);
  const compose = readFileSync(storage.compose_file, "utf8");
  assert.ok(!compose.includes("management-tls"));
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.deepEqual(validateFleetInventory(inventory, config), []);
  assert.equal(inventory.resources.filter((resource) => resource.type === "directory" && resource.status === "created").length, 4);
});

test("Worker deployment is exact-pinned, least-mounted, and inventoried before mutation", async () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.onRun = (args) => {
    const deployer = args[args.indexOf("--name") + 1];
    const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    assert.equal(inventory.resources.find((resource) => resource.type === "docker_container" && resource.id === deployer)?.status, "planned");
    assert.equal(inventory.actions.at(-1).kind, "celld_deploy");
    assert.equal(inventory.actions.at(-1).target, deployer);
    assert.equal(inventory.actions.at(-1).status, "planned");
    assert.ok(args.includes("--rm"));
    assert.ok(args.includes(config.pins.celld.image_ref));
    assert.ok(args.includes("CELLD_ESBUILD=/usr/local/bin/esbuild"));
    assert.ok(args.includes(`type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`));
    assert.ok(args.includes(`type=bind,src=${storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
    assert.ok(!args.join(" ").includes(storage.admin_identity_file_ref));
    assert.ok(!args.join(" ").includes(join(storage.run_root, "tls/ca.key")));
    assert.ok(!args.join(" ").includes(config.worker_vars_file_ref));
    assert.ok(!args.join(" ").includes(readFileSync(join(storage.run_root, "secret-key"), "utf8").trim()));
    assert.deepEqual(args.slice(-8), [
      "deploy", "/workspace",
      "--bucket", `s3://${storage.bucket}/${storage.run_prefix}/fleet`,
      "--endpoint", "https://s3gateway1:8334",
      "--region", storage.region,
    ]);
  };
  const result = await deployFleetWorker(configPath, {
    runner: docker.run,
    esbuildPath: process.execPath,
    ensureBucket: async () => {
      const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
      assert.equal(inventory.actions.at(-1).kind, "s3_create_bucket");
      assert.equal(inventory.actions.at(-1).status, "planned");
      assert.match(inventory.actions.at(-1).target_sha256, /^[0-9a-f]{64}$/);
      return { created: true };
    },
  });
  assert.equal(result.status, "DEPLOYED");
  assert.equal(result.worker_digest, config.pins.worker_digest);
  assert.match(result.deployment_sha256, /^[0-9a-f]{64}$/);
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.resources.find((resource) => resource.id.endsWith("-worker-deploy")).status, "removed");
  assert.equal(inventory.actions.at(-1).status, "completed");
  assert.equal(inventory.worker_digest, config.pins.worker_digest);
  const replay = await deployFleetWorker(configPath, {
    runner: docker.run,
    esbuildPath: process.execPath,
    ensureBucket: async () => { throw new Error("completed deployment must not mutate the bucket"); },
  });
  assert.deepEqual(replay, result);
  assert.equal(JSON.parse(readFileSync(inventoryPath, "utf8")).actions.length, inventory.actions.length);
});

test("interrupted Worker deployment is recoverable by exact-owned cleanup", async () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.failRunLeavesContainer = true;
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, ensureBucket: async () => ({ created: true }) }),
    /injected deploy interruption/,
  );
  const interrupted = JSON.parse(readFileSync(inventoryPath, "utf8"));
  const deployer = interrupted.resources.find((resource) => resource.id.endsWith("-worker-deploy"));
  assert.equal(deployer.status, "planned");
  assert.equal(interrupted.actions.at(-1).status, "planned");
  docker.failRunLeavesContainer = false;
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, ensureBucket: async () => { throw new Error("must not retry bucket mutation"); } }),
    /outcome is unknown/,
  );
  docker.containers.get(deployer.id).labels = {};
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, ensureBucket: async () => { throw new Error("foreign ownership must fail before bucket mutation"); } }),
    /refusing unowned container/,
  );
  assert.throws(() => cleanupFleet(configPath, { runner: docker.run }), /refusing unowned container/);
  docker.containers.get(deployer.id).labels = docker.labels();
  assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
  const cleaned = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(cleaned.resources.find((resource) => resource.id === deployer.id).status, "removed");
  assert.equal(docker.containers.size, 0);
});

test("fleet persists every container target before creation and reports real readiness", () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.onCreate = (name) => {
    const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    assert.equal(inventory.resources.find((resource) => resource.type === "docker_container" && resource.id === name)?.status, "planned");
    assert.equal(inventory.actions.at(-1).kind, "docker_create");
    assert.equal(inventory.actions.at(-1).target, name);
    assert.equal(inventory.actions.at(-1).status, "planned");
  };
  const diagnosis = startFleet(configPath, { runner: docker.run });
  assert.equal(diagnosis.status, "READY");
  assert.equal(diagnosis.membership.running, 3);
  assert.equal(diagnosis.membership.reserve, 1);
  assert.equal(diagnosis.membership.probe, "passed");
  assert.match(diagnosis.membership.probe_sha256, /^[0-9a-f]{64}$/);
  assert.ok(diagnosis.nodes.every((node) => node.public_endpoint.startsWith("http://127.0.0.1:")));
  assert.equal(diagnoseFleet(configPath, { runner: docker.run }).status, "READY");
});

test("callback relays share only node loopback and authenticate management with protected mTLS material", () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  startFleet(configPath, { runner: docker.run });
  const result = startCallbackRelays(configPath, { runner: docker.run, relayBinaryPath: process.execPath });
  assert.equal(result.status, "READY");
  assert.equal(result.relays.length, 3);
  assert.ok(result.relays.every((relay) => relay.listener === "node-loopback" && relay.management_transport === "private-ca-mtls"));
  assert.match(result.binary_sha256, /^[a-f0-9]{64}$/);
  assert.ok(!JSON.stringify(result).includes(readFileSync(config.callback.relay_client_key_file_ref, "utf8").trim()));
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.resources.filter((resource) => resource.id.endsWith("-callback-relay") && resource.status === "started").length, 3);
  assert.equal(inventory.actions.filter((action) => action.kind === "callback_relay_create" && action.status === "completed").length, 3);
  assert.match(inventory.callback_relays_sha256, /^[a-f0-9]{64}$/);
  assert.equal(cleanupFleet(configPath, { runner: docker.run }).removed_containers, 6);
});

test("deployed Worker probe proves signed readiness and denials without retaining auth material", async () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  startFleet(configPath, { runner: docker.run });
  const vars = Object.fromEntries(readFileSync(config.worker_vars_file_ref, "utf8").trim().split("\n").map((line) => line.split(/=(.*)/s).slice(0, 2)));
  const seenNonces = new Set();
  const seenSignatures = [];
  const fetcher = async (url, init) => {
    const headers = init.headers;
    const digest = headers["x-agentic-body-sha256"];
    const canonical = [init.method, url.pathname, headers["x-agentic-operation-id"], headers["x-agentic-generation"], headers["x-agentic-timestamp"], headers["x-agentic-nonce"], digest].join("\n");
    const expected = createHmac("sha256", vars.CELL_AUTH_KEY).update(canonical).digest("hex");
    seenSignatures.push(headers["x-agentic-signature"]);
    if (headers["x-agentic-signature"] !== expected) return Response.json({ error: { code: "cell.signature_invalid" } }, { status: 401 });
    if (seenNonces.has(headers["x-agentic-nonce"])) return Response.json({ error: { code: "cell.signature_replayed" } }, { status: 409 });
    seenNonces.add(headers["x-agentic-nonce"]);
    return Response.json({ error: { code: "cell.missing" } }, { status: 404 });
  };
  const result = await probeFleetWorker(configPath, { runner: docker.run, fetcher, nonceFactory: () => "a".repeat(32) });
  assert.equal(result.status, "READY");
  assert.deepEqual(result.checks, {
    signed_missing_cell: { status: 404, code: "cell.missing" },
    forged_signature: { status: 401, code: "cell.signature_invalid" },
    nonce_replay: { status: 409, code: "cell.signature_replayed" },
  });
  assert.equal(seenSignatures.length, 3);
  assert.ok(!JSON.stringify(result).includes(vars.CELL_AUTH_KEY));
  assert.ok(seenSignatures.every((signature) => !JSON.stringify(result).includes(signature)));
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.actions.at(-1).kind, "worker_public_auth_probe");
  assert.equal(inventory.actions.at(-1).status, "completed");
  assert.match(inventory.worker_probe_sha256, /^[a-f0-9]{64}$/);
});

test("each partial container creation boundary is crash-resumable and teardown is idempotent", () => {
  for (let failure = 1; failure <= 3; failure += 1) {
    const { config, storage, configPath } = fixture();
    const docker = new FakeDocker(config, storage);
    docker.failCreate = failure;
    assert.throws(() => startFleet(configPath, { runner: docker.run }), /injected create failure/);
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
    assert.equal(existsSync(config.worker_vars_file_ref), false);
    assert.ok(JSON.parse(readFileSync(join(config.run_root, "fleet-inventory.json"), "utf8")).resources.every((resource) => resource.status === "removed"));
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
  }
});

test("cleanup recovers at both protected-file creation crash boundaries", async () => {
  for (const fileExists of [false, true]) {
    const { config, storage, configPath, inventoryPath } = fixture();
    const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    inventory.resources.find((resource) => resource.type === "protected_file").status = "planned";
    writeFileSync(inventoryPath, `${JSON.stringify(inventory, null, 2)}\n`);
    if (!fileExists) rmSync(config.worker_vars_file_ref, { force: false });
    const docker = new FakeDocker(config, storage);
    assert.throws(() => startFleet(configPath, { runner: docker.run }), /Worker vars file is not ready/);
    await assert.rejects(
      deployFleetWorker(configPath, {
        runner: docker.run,
        esbuildPath: process.execPath,
        ensureBucket: async () => { throw new Error("must not mutate the bucket"); },
      }),
      /Worker vars file is not ready/,
    );
    assert.equal(docker.containers.size, 0);
    assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
    assert.equal(existsSync(config.worker_vars_file_ref), false);
  }
});

test("cleanup refuses foreign labels and elevates independently detected residue to exit 4", () => {
  const first = fixture();
  const foreign = new FakeDocker(first.config, first.storage);
  foreign.containers.set(first.config.nodes[0].name, { labels: {}, running: true });
  assert.throws(() => cleanupFleet(first.configPath, { runner: foreign.run }), /refusing unowned container/);

  const second = fixture();
  const residue = new FakeDocker(second.config, second.storage);
  residue.forceResidue = true;
  assert.throws(
    () => cleanupFleet(second.configPath, { runner: residue.run }),
    (error) => error instanceof CleanupResidueError && error.exitCode === 4,
  );
});

test("janitor preview enforces age and exact owner while retaining partial inventories", () => {
  mkdirSync(TEST_ROOT, { recursive: true, mode: 0o700 });
  const old = fixture(new Date("2026-08-20T00:00:00Z"));
  const young = fixture(new Date("2026-08-21T11:59:30Z"));
  const partialId = `test-${randomUUID()}`;
  const partialRoot = join(TEST_ROOT, partialId);
  roots.add(partialRoot);
  mkdirSync(partialRoot, { mode: 0o700 });
  writeFileSync(join(partialRoot, "fleet-inventory.json"), `${JSON.stringify({ schema_version: "agentic-sandbox.celld-fleet-inventory/v1", run_id: partialId, owner: { repository: "someone/else", workflow: "unknown", run_id: partialId }, created_at: "2026-08-20T00:00:00Z" })}\n`, { mode: 0o600 });
  chmodSync(join(partialRoot, "fleet-inventory.json"), 0o600);
  const preview = janitorPreview(TEST_ROOT, { minimumAgeSeconds: 3600, now: new Date("2026-08-21T12:00:00Z") });
  assert.ok(preview.targets.some((target) => target.run_id === old.config.run_id));
  assert.ok(preview.retained.some((target) => target.run_id === young.config.run_id && target.reason === "minimum_age"));
  assert.ok(preview.retained.some((target) => target.run_id === partialId && target.reason === "ownership_mismatch"));
});
