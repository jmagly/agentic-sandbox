import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  CleanupResidueError,
  cleanupFleet,
  diagnoseFleet,
  janitorPreview,
  prepareFleet,
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
    return JSON.stringify([{ Config: { Labels: container.labels }, State: { Running: container.running } }]);
  }

  run = (program, args) => {
    assert.equal(program, "docker");
    if (args[0] === "network" && args[1] === "inspect") {
      return JSON.stringify([{ Labels: { "com.docker.compose.project": this.storage.project, "dev.agentic-sandbox.scope": "celld-qualification" } }]);
    }
    if (args[0] === "image" && args[1] === "inspect") return "[]";
    if (args[0] === "inspect") return this.inspect(args[1]);
    if (args[0] === "create") {
      this.createCount += 1;
      const name = args[args.indexOf("--name") + 1];
      this.onCreate?.(name);
      if (this.failCreate === this.createCount) throw new Error("injected create failure");
      assert.ok(args.includes(this.config.pins.celld.image_ref));
      assert.ok(args.includes("AWS_SHARED_CREDENTIALS_FILE=/run/identity/credentials"));
      assert.ok(args.includes(`type=bind,src=${this.storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
      assert.ok(!args.some((value) => value.includes("/tls,dst=/run/tls")));
      assert.ok(!args.join(" ").includes(readFileSync(this.storage.secret_key_file_ref ?? join(this.storage.run_root, "secret-key"), "utf8").trim()));
      this.containers.set(name, { labels: this.labels(), running: false });
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
  const { config, inventoryPath } = fixture();
  assert.deepEqual(validateFleetConfig(config), []);
  assert.equal(config.scope, "single-host multi-node");
  assert.equal(config.nodes.length, 3);
  assert.equal(new Set(config.nodes.map((node) => node.name)).size, 3);
  assert.deepEqual(config.nodes.map((node) => node.role), ["active", "active", "reserve"]);
  assert.equal(config.pins.celld.manifest_digest, "sha256:8634eac20f69ffe99103d403b985c0afd43fd970badadd01435f297ba0df797a");
  assert.equal(config.pins.worker_digest, "sha256:89ad83f8bfb8be244560043f9ad3ea1a7cbbc6abcaaa444b8cf8d852263f3885");
  assert.equal(config.network.public_publish, "127.0.0.1::8080");
  const tlsExtensions = readFileSync(join(config.run_root, "tls/server-ext.cnf"), "utf8");
  assert.match(tlsExtensions, /DNS:s3gateway1/);
  assert.match(tlsExtensions, /DNS:s3gateway2/);
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.deepEqual(validateFleetInventory(inventory, config), []);
  assert.equal(inventory.resources.filter((resource) => resource.type === "directory" && resource.status === "created").length, 4);
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

test("each partial container creation boundary is crash-resumable and teardown is idempotent", () => {
  for (let failure = 1; failure <= 3; failure += 1) {
    const { config, storage, configPath } = fixture();
    const docker = new FakeDocker(config, storage);
    docker.failCreate = failure;
    assert.throws(() => startFleet(configPath, { runner: docker.run }), /injected create failure/);
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
    assert.ok(JSON.parse(readFileSync(join(config.run_root, "fleet-inventory.json"), "utf8")).resources.every((resource) => resource.status === "removed"));
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
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
