import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, createHmac, randomUUID } from "node:crypto";
import { chmodSync, closeSync, constants, existsSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";

import {
  CleanupResidueError,
  cleanupFleet,
  deployFleetQualificationWorker,
  deployFleetWorker,
  diagnoseFleet,
  ensureFleetBucket,
  janitorPreview,
  openFleetWorkerAccess,
  prepareFleet,
  probeFleetWorker,
  startCallbackRelays,
  startFleet,
  stopFleetForWorkerDeployment,
  validateFleetConfig,
  validateFleetInventory,
  workerDeploymentProjectDigest,
} from "../../../scripts/celld-fleet-fixture.mjs";
import { prepareFixture } from "../../../scripts/celld-seaweedfs-fixture.mjs";

const TEST_ROOT = "/dev/shm/agentic-celld-fleet-tests";
const roots = new Set();
const REPO_ROOT = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "");
const FLEET_FIXTURE_CLI = join(REPO_ROOT, "scripts/celld-fleet-fixture.mjs");
const STARTUP_READINESS_DOCKER = new URL("./fixtures/fake-docker-startup-readiness.mjs", import.meta.url);
const PINNED_DIAGNOSIS_PATH = new URL("./fixtures/celld-v0.2.1-diagnose-output.json", import.meta.url).pathname;
const PINNED_DIAGNOSIS_OUTPUT = JSON.parse(readFileSync(
  PINNED_DIAGNOSIS_PATH,
  "utf8",
));

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function pinnedCelldDiagnosisOutput(config, {
  nodeIds = config.nodes.map((node) => node.node_id),
  signed = true,
  includeConditionalWrite = true,
  telemetry = {},
} = {}) {
  const lines = includeConditionalWrite ? [PINNED_DIAGNOSIS_OUTPUT.conditional_write_line] : [];
  for (const [index, nodeId] of nodeIds.entries()) {
    const address = config.nodes[index % config.nodes.length]?.advertise ?? `10.77.0.${index + 11}:8081`;
    let line = PINNED_DIAGNOSIS_OUTPUT.signed_direct_peer_line_template
      .replace("{{NODE_ID}}", nodeId)
      .replace("{{ADDRESS}}", address);
    for (const [field, value] of Object.entries(telemetry)) {
      line = line.replace(new RegExp(`${field}=[^\\s]+`), `${field}=${value}`);
    }
    if (!signed) line = line.replace(" (signed direct probe)", " (direct probe)");
    lines.push(line);
  }
  return lines.join("\n");
}

function signedDirectDiagnosisOutput(config, {
  includeAt = true,
  fieldEntries = () => [
    ["protocol", "2"],
    ["resident_cells", "0"],
    ["websockets", "0"],
    ["rss_bytes", "1048576"],
    ["in_use_bytes", "524288"],
    ["cpu_percent", "0.00"],
    ["fds", "12/1048576"],
    ["pressured", "false"],
    ["shed_cells", "0"],
    ["restoring", "0"],
    ["load_age_ms", "0"],
  ],
} = {}) {
  return [
    PINNED_DIAGNOSIS_OUTPUT.conditional_write_line,
    ...config.nodes.map((node, index) => {
      const fields = fieldEntries({ node, index })
        .map(([key, value]) => `${key}=${value}`)
        .join(" ");
      const address = includeAt ? `at ${node.advertise}` : node.advertise;
      return `ok peer ${node.node_id} ${address} (signed direct probe) ${fields}`;
    }),
  ].join("\n");
}

function tracingPersistenceFs(events, { failDirectoryFsync = null } = {}) {
  const descriptors = new Map();
  return {
    writeFileSync(path, value, options) {
      events.push({ operation: "write", path });
      return writeFileSync(path, value, options);
    },
    chmodSync(path, mode) {
      events.push({ operation: "chmod", path, mode });
      return chmodSync(path, mode);
    },
    openSync(path, flags, mode) {
      const descriptor = openSync(path, flags, mode);
      descriptors.set(descriptor, path);
      events.push({ operation: "open", path, flags });
      return descriptor;
    },
    fsyncSync(descriptor) {
      const path = descriptors.get(descriptor);
      events.push({ operation: "fsync", path });
      if (path === failDirectoryFsync) throw new Error("injected crash before diagnosis parent-directory fsync");
      return fsyncSync(descriptor);
    },
    closeSync(descriptor) {
      events.push({ operation: "close", path: descriptors.get(descriptor) });
      descriptors.delete(descriptor);
      return closeSync(descriptor);
    },
    renameSync(from, to) {
      events.push({ operation: "rename", from, to });
      return renameSync(from, to);
    },
    constants,
  };
}

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

function candidateFixture(now = new Date()) {
  const runId = `test-${randomUUID()}`;
  const root = join(TEST_ROOT, runId);
  roots.add(root);
  const storage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
  const config = prepareFleet({ storageConfigPath: join(root, "fixture.json"), now, celldChannel: "reviewed-candidate" });
  return { root, storage, config, configPath: join(root, "fleet.json"), inventoryPath: join(root, "fleet-inventory.json") };
}

class FakeDocker {
  constructor(config, storage) {
    this.config = config;
    this.storage = storage;
    this.containers = new Map();
    this.createCount = 0;
    this.failAfterCreate = null;
    this.forceResidue = false;
    this.onCreate = null;
    this.onRun = null;
    this.runOutput = "Current Version ID: 1111111111111111\ndeployment committed";
    this.failRunLeavesContainer = false;
    this.expectedFaultSignal = "disabled";
    this.failPortDiscovery = false;
  }

  labels() {
    return {
      "dev.agentic-sandbox.repository": "roctinam/agentic-sandbox",
      "dev.agentic-sandbox.workflow": "celld-qualification",
      "dev.agentic-sandbox.run": this.config.run_id,
      "dev.agentic-sandbox.scope": "celld-qualification",
    };
  }

  containerId(name) {
    return sha256(`${this.config.run_id}:${name}`);
  }

  containerAddress(name) {
    const index = this.config.nodes.findIndex((node) => node.name === name);
    return `172.29.0.${20 + Math.max(index, 0)}`;
  }

  inspect(name) {
    const container = this.containers.get(name);
    if (!container) throw new Error("not found");
    const id = this.containerId(name);
    const address = this.containerAddress(name);
    return JSON.stringify([{
      Id: id,
      Name: `/${name}`,
      Config: { Labels: container.labels, Image: container.image },
      HostConfig: { PortBindings: {} },
      NetworkSettings: {
        Ports: {},
        Networks: {
          [this.config.network.name]: {
            NetworkID: this.containerId(this.config.network.name),
            IPAddress: address,
            Aliases: [name],
          },
        },
      },
      State: { Running: container.running },
    }]);
  }

  run = (program, args, options = {}) => {
    assert.equal(program, "docker");
    assert.equal(options.env?.AWS_ACCESS_KEY_ID, undefined);
    assert.equal(options.env?.AWS_SECRET_ACCESS_KEY, undefined);
    if (args[0] === "network" && args[1] === "inspect") {
      const containers = {};
      for (const [name, container] of this.containers) {
        if (!container.running || name.endsWith("-callback-relay")) continue;
        const id = this.containerId(name);
        const address = this.containerAddress(name);
        containers[id] = { Name: name, IPv4Address: `${address}/16` };
      }
      return JSON.stringify([{
        Id: this.containerId(this.config.network.name),
        Name: this.config.network.name,
        Driver: "bridge",
        Scope: "local",
        Internal: true,
        Ingress: false,
        Labels: {
          "com.docker.compose.project": this.storage.project,
          "com.docker.compose.network": "storage-private",
          "dev.agentic-sandbox.run": this.storage.run_id,
          "dev.agentic-sandbox.scope": "celld-qualification",
        },
        IPAM: { Config: [{ Gateway: "172.29.0.1" }] },
        Containers: containers,
      }]);
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
      return this.runOutput;
    }
    if (args[0] === "create") {
      this.createCount += 1;
      const name = args[args.indexOf("--name") + 1];
      this.onCreate?.(name);
      if (name.endsWith("-callback-relay")) {
        assert.ok(args.includes(`container:${name.slice(0, -"-callback-relay".length)}`));
        assert.ok(args.includes("/usr/local/bin/agentic-celld-callback-relay"));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.relay_client_cert_file_ref},dst=/run/tls/client.crt,readonly`));
        assert.ok(args.includes(`type=bind,src=${this.config.callback.relay_client_key_file_ref},dst=/run/tls/client.key,readonly`));
        assert.ok(args.includes("172.29.0.1:8122"));
        assert.equal(args[args.indexOf("--fault-signal") + 1], this.expectedFaultSignal);
        assert.ok(!args.join(" ").includes(readFileSync(this.config.callback.relay_client_key_file_ref, "utf8").trim()));
        this.containers.set(name, { labels: this.labels(), image: this.config.pins.celld.image_ref, running: false });
        if (this.failAfterCreate === this.createCount) throw new Error("injected termination after create");
        return name;
      }
      assert.ok(args.includes(this.config.pins.celld.image_ref));
      assert.ok(!args.some((value) => value.startsWith("AWS_ACCESS_KEY_ID=")));
      assert.ok(!args.some((value) => value.startsWith("AWS_SECRET_ACCESS_KEY=")));
      assert.ok(!args.some((value) => value.startsWith("AWS_SHARED_CREDENTIALS_FILE=")));
      assert.ok(args.includes(`type=bind,src=${process.execPath},dst=/usr/local/bin/agentic-celld-credential-launcher,readonly`));
      assert.equal(args[args.indexOf("--entrypoint") + 1], "/usr/local/bin/agentic-celld-credential-launcher");
      assert.ok(args.includes(`type=bind,src=${this.storage.ca_file_ref},dst=/run/tls/ca.crt,readonly`));
      assert.ok(args.includes(`type=bind,src=${this.config.worker_vars_file_ref},dst=/run/worker/vars,readonly`));
      assert.ok(args.includes("CELLD_VARS_FILE=/run/worker/vars"));
      assert.ok(!args.some((value) => value.includes("/tls,dst=/run/tls")));
      assert.ok(!args.join(" ").includes(readFileSync(this.storage.secret_key_file_ref ?? join(this.storage.run_root, "secret-key"), "utf8").trim()));
      const workerAuthKey = /^CELL_AUTH_KEY=(.+)$/m.exec(readFileSync(this.config.worker_vars_file_ref, "utf8"))[1];
      assert.ok(!args.join(" ").includes(workerAuthKey));
      this.containers.set(name, { labels: this.labels(), image: this.config.pins.celld.image_ref, running: false });
      if (this.failAfterCreate === this.createCount) throw new Error("injected termination after create");
      return name;
    }
    if (args[0] === "start") {
      this.containers.get(args[1]).running = true;
      return args[1];
    }
    if (args[0] === "stop") {
      this.containers.get(args.at(-1)).running = false;
      return args.at(-1);
    }
    if (args[0] === "port") {
      if (this.failPortDiscovery) {
        const error = new Error("docker controller subprocess failed");
        error.name = "FleetControllerSubprocessError";
        error.program = "docker";
        error.operation = "docker_port";
        error.exitStatus = 1;
        error.signal = null;
        error.errorCode = null;
        error.timedOut = false;
        error.stdoutSha256 = sha256("");
        error.stderrSha256 = sha256("port is not published");
        throw error;
      }
      const index = this.config.nodes.findIndex((node) => node.name === args[1]);
      return `127.0.0.1:${18080 + index}`;
    }
    if (args[0] === "exec") {
      assert.equal(args[2], "/usr/local/bin/agentic-celld-credential-launcher");
      assert.equal(args[3], "diagnose");
      assert.equal(args.filter((value) => value === "--peer").length, 3);
      assert.deepEqual(
        args.filter((_value, index) => args[index - 1] === "--peer"),
        this.config.nodes.map((node) => node.node_id),
      );
      assert.ok(this.config.nodes.every((node) => !args.includes(node.advertise)));
      return pinnedCelldDiagnosisOutput(this.config);
    }
    if (args[0] === "rm") {
      this.containers.delete(args.at(-1));
      return args.at(-1);
    }
    if (args[0] === "ps") return this.forceResidue ? "unexpected-residue" : [...this.containers.keys()].join("\n");
    throw new Error(`unexpected fake Docker command: ${args.join(" ")}`);
  };
}

test("pinned Celld v0.2.1 diagnosis golden uses the authoritative signed-direct output", () => {
  assert.equal(PINNED_DIAGNOSIS_OUTPUT.schema_version, "agentic-sandbox.celld-pinned-diagnosis-output/v1");
  assert.equal(PINNED_DIAGNOSIS_OUTPUT.version, "0.2.1");
  assert.equal(PINNED_DIAGNOSIS_OUTPUT.commit, "ae8fac053d79f971bfcb996054bb43eb2f9b05da");
  assert.equal(
    PINNED_DIAGNOSIS_OUTPUT.conditional_write_line,
    "ok bucket conditional write (create, reject-create, update, reject-stale)",
  );
  assert.match(PINNED_DIAGNOSIS_OUTPUT.signed_direct_peer_line_template, /^ok peer \{\{NODE_ID\}\} at \{\{ADDRESS\}\} \(signed direct probe\) /);
  assert.doesNotMatch(JSON.stringify(PINNED_DIAGNOSIS_OUTPUT), /ok lease|ok signed peer/);
});

test("fleet preparation fixes three exact addressed nodes and one reserve", () => {
  const { config, storage, inventoryPath } = fixture();
  assert.deepEqual(validateFleetConfig(config), []);
  assert.equal(config.scope, "single-host multi-node");
  assert.equal(config.nodes.length, 3);
  assert.equal(new Set(config.nodes.map((node) => node.name)).size, 3);
  assert.deepEqual(config.nodes.map((node) => node.role), ["active", "active", "reserve"]);
  assert.deepEqual(config.operator_commands, ["prepare", "deploy", "start", "start-relays", "diagnose", "probe-worker", "cleanup", "janitor-preview", "janitor-reap"]);
  assert.equal(config.pins.celld_channel, "approved");
  assert.equal(config.pins.celld.manifest_digest, "sha256:8634eac20f69ffe99103d403b985c0afd43fd970badadd01435f297ba0df797a");
  assert.equal(config.pins.worker_digest, "sha256:ee79e3c52deaadd30fe9ab485d7e78d4a9f84447e483e9e4fa86efd2e357d000");
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
  inventory.credential_launcher_sha256 = "not-a-digest";
  assert.match(validateFleetInventory(inventory, config).join("; "), /credential launcher digest/);
  assert.equal(inventory.resources.filter((resource) => resource.type === "directory" && resource.status === "created").length, 4);
});

test("reviewed candidate selection is explicit and cannot masquerade as approved", () => {
  const { config } = candidateFixture();
  assert.deepEqual(validateFleetConfig(config), []);
  assert.equal(config.pins.celld_channel, "reviewed-candidate");
  assert.equal(config.pins.celld.version, "0.3.0");
  assert.equal(config.pins.celld.commit, "89e4ffc53a14ecb496d2ca5014ff9d19b0061ad9");
  assert.equal(config.pins.celld.index_digest, "sha256:f47d97c2980aa98aef1d9c42205a313442f48acb606c5987dbb9b32983a23aaf");
  assert.equal(config.pins.celld.manifest_digest, "sha256:e2983741d4733a537dcdb399671d3ce2f6968bfe4f15ce0a70c0279e10a930d1");
  config.pins.celld_channel = "approved";
  assert.match(validateFleetConfig(config).join("; "), /does not match its exact image channel/);
});

test("fleet bucket setup uses one protected gateway endpoint and closes both resources", async () => {
  const { config, storage } = fixture();
  const calls = [];
  const gatewayAccessFactory = async (value, options) => {
    assert.equal(value, storage);
    assert.deepEqual(options.services, ["s3gateway1"]);
    calls.push("gateway-open");
    return {
      endpoints: ["https://127.0.0.1:64152"],
      async close() { calls.push("gateway-close"); },
    };
  };
  const clientFactory = (profile) => {
    assert.equal(profile.endpoint, "https://127.0.0.1:64152");
    assert.deepEqual(profile.backend.gateway_endpoints, [profile.endpoint]);
    assert.equal(profile.identity_file_ref, storage.admin_identity_file_ref);
    calls.push("client-open");
    return {
      async createBucket() { calls.push("bucket-create"); return { status: 409 }; },
      async listPrefix() { calls.push("bucket-list"); return { status: 200 }; },
      close() { calls.push("client-close"); },
    };
  };
  assert.deepEqual(
    await ensureFleetBucket(config, storage, () => { throw new Error("runner must be passed through, not invoked by this fake"); }, { gatewayAccessFactory, clientFactory }),
    { created: false },
  );
  assert.deepEqual(calls, ["gateway-open", "client-open", "bucket-create", "bucket-list", "client-close", "gateway-close"]);
});

test("Worker deployment rejects an unsafe credential file before bucket mutation", async () => {
  const { config, storage, configPath } = fixture();
  const docker = new FakeDocker(config, storage);
  chmodSync(storage.identity_file_ref, 0o640);
  let bucketMutation = false;
  await assert.rejects(
    deployFleetWorker(configPath, {
      runner: docker.run,
      esbuildPath: process.execPath,
      credentialLauncherPath: process.execPath,
      ensureBucket: async () => { bucketMutation = true; return { created: true }; },
    }),
    /bucket credential must be a protected bounded regular file/,
  );
  assert.equal(bucketMutation, false);
  assert.equal(docker.containers.size, 0);
  chmodSync(storage.identity_file_ref, 0o600);
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
    assert.ok(args.includes(`type=bind,src=${process.execPath},dst=/usr/local/bin/agentic-celld-credential-launcher,readonly`));
    assert.equal(args[args.indexOf("--entrypoint") + 1], "/usr/local/bin/agentic-celld-credential-launcher");
    assert.ok(!args.some((value) => value.startsWith("AWS_ACCESS_KEY_ID=")));
    assert.ok(!args.some((value) => value.startsWith("AWS_SECRET_ACCESS_KEY=")));
    assert.ok(!args.some((value) => value.startsWith("AWS_SHARED_CREDENTIALS_FILE=")));
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
    credentialLauncherPath: process.execPath,
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
  assert.match(result.credential_launcher_sha256, /^[a-f0-9]{64}$/);
  assert.match(result.deployment_sha256, /^[0-9a-f]{64}$/);
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.resources.find((resource) => resource.id.endsWith("-worker-deploy")).status, "removed");
  assert.equal(inventory.actions.at(-1).status, "completed");
  assert.equal(inventory.worker_digest, config.pins.worker_digest);
  assert.equal(inventory.credential_launcher_sha256, result.credential_launcher_sha256);
  const replay = await deployFleetWorker(configPath, {
    runner: docker.run,
    esbuildPath: process.execPath,
    credentialLauncherPath: process.execPath,
    ensureBucket: async () => { throw new Error("completed deployment must not mutate the bucket"); },
  });
  assert.deepEqual(replay, result);
  assert.equal(JSON.parse(readFileSync(inventoryPath, "utf8")).actions.length, inventory.actions.length);
});

test("qualification Worker deployments reuse the protected launcher and retain immutable versions", async () => {
  const { root, config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  await deployFleetWorker(configPath, {
    runner: docker.run,
    esbuildPath: process.execPath,
    credentialLauncherPath: process.execPath,
    ensureBucket: async () => ({ created: true }),
  });
  const project = join(root, "qualification-candidate");
  mkdirSync(project, { mode: 0o700 });
  writeFileSync(join(project, "worker.mjs"), "export default { fetch() { return new Response('candidate'); } };\n", { mode: 0o600 });
  writeFileSync(join(project, "wrangler.json"), `${JSON.stringify({ name: "qualification-candidate", main: "worker.mjs", compatibility_date: "2026-08-14" })}\n`, { mode: 0o600 });
  const projectDigest = workerDeploymentProjectDigest(project);
  const invocations = [];
  let expectedProjectDigest = projectDigest;
  docker.onRun = (args) => {
    invocations.push(args);
    const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    assert.equal(inventory.actions.at(-1).kind, "celld_qualification_deploy");
    assert.equal(inventory.actions.at(-1).status, "planned");
    assert.equal(inventory.actions.at(-1).project_sha256, expectedProjectDigest);
  };
  docker.runOutput = "Current Version ID: 2222222222222222\ncandidate committed";
  const candidate = deployFleetQualificationWorker(configPath, {
    projectPath: project,
    projectDigest,
    deploymentKind: "qualification-candidate",
    runner: docker.run,
    esbuildPath: process.execPath,
    credentialLauncherPath: process.execPath,
  });
  assert.equal(candidate.version_id, "2222222222222222");
  assert.equal(candidate.project_sha256, projectDigest);
  const candidateArgs = invocations.at(-1);
  assert.ok(candidateArgs.includes(`type=bind,src=${storage.identity_file_ref},dst=/run/identity/credentials,readonly`));
  assert.ok(candidateArgs.includes(`type=bind,src=${process.execPath},dst=/usr/local/bin/agentic-celld-credential-launcher,readonly`));
  assert.equal(candidateArgs[candidateArgs.indexOf("--entrypoint") + 1], "/usr/local/bin/agentic-celld-credential-launcher");
  assert.ok(!candidateArgs.some((value) => value.startsWith("AWS_SHARED_CREDENTIALS_FILE=")));
  assert.ok(!candidateArgs.join(" ").includes(readFileSync(join(storage.run_root, "secret-key"), "utf8").trim()));

  const approvedRoot = join(new URL("../../../", import.meta.url).pathname, "runtimes/celld/instance-cell");
  const approvedDigest = workerDeploymentProjectDigest(approvedRoot, { deploymentKind: "approved-reference" });
  expectedProjectDigest = approvedDigest;
  docker.runOutput = "Current Version ID: 3333333333333333\napproved restored";
  const restored = deployFleetQualificationWorker(configPath, {
    projectPath: approvedRoot,
    projectDigest: approvedDigest,
    deploymentKind: "approved-reference",
    runner: docker.run,
    esbuildPath: process.execPath,
    credentialLauncherPath: process.execPath,
  });
  assert.equal(restored.version_id, "3333333333333333");
  assert.equal(restored.worker_digest, config.pins.worker_digest);
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.active_worker_deployment.version_id, restored.version_id);
  assert.equal(inventory.active_worker_deployment.deployment_kind, "approved-reference");
  assert.equal(inventory.resources.find((resource) => resource.id.endsWith("-worker-qualify")).status, "removed");
});

test("interrupted Worker deployment is recoverable by exact-owned cleanup", async () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.failRunLeavesContainer = true;
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, credentialLauncherPath: process.execPath, ensureBucket: async () => ({ created: true }) }),
    /injected deploy interruption/,
  );
  const interrupted = JSON.parse(readFileSync(inventoryPath, "utf8"));
  const deployer = interrupted.resources.find((resource) => resource.id.endsWith("-worker-deploy"));
  assert.equal(deployer.status, "planned");
  assert.equal(interrupted.actions.at(-1).status, "planned");
  docker.failRunLeavesContainer = false;
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, credentialLauncherPath: process.execPath, ensureBucket: async () => { throw new Error("must not retry bucket mutation"); } }),
    /outcome is unknown/,
  );
  docker.containers.get(deployer.id).labels = {};
  await assert.rejects(
    deployFleetWorker(configPath, { runner: docker.run, esbuildPath: process.execPath, credentialLauncherPath: process.execPath, ensureBucket: async () => { throw new Error("foreign ownership must fail before bucket mutation"); } }),
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
  const diagnosis = startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath });
  assert.equal(diagnosis.status, "READY");
  assert.equal(diagnosis.membership.running, 3);
  assert.equal(diagnosis.membership.reserve, 1);
  assert.equal(diagnosis.membership.probe, "passed");
  assert.match(diagnosis.membership.probe_sha256, /^[0-9a-f]{64}$/);
  assert.ok(diagnosis.nodes.every((node) => node.public_endpoint.startsWith("http://127.0.0.1:")));
  assert.equal(diagnoseFleet(configPath, { runner: docker.run }).status, "READY");
  const stopped = stopFleetForWorkerDeployment(configPath, { runner: docker.run });
  assert.equal(stopped.status, "STOPPED");
  assert.ok(config.nodes.every((node) => docker.containers.get(node.name).running === false));
  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  assert.equal(inventory.state, "stopped_for_worker_deployment");
  assert.equal(inventory.actions.filter((action) => action.kind === "docker_stop_for_worker_deployment" && action.status === "completed").length, 3);
});

test("fleet startup does not misclassify running nodes when Docker omits internal-network port publication", async () => {
  const { config, storage, configPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.failPortDiscovery = true;
  const diagnosis = startFleet(configPath, {
    runner: docker.run,
    credentialLauncherPath: process.execPath,
    readinessPolicy: { maxAttempts: 1, deadlineMs: 1_000, backoffMs: 0 },
  });

  assert.equal(diagnosis.status, "READY");
  assert.equal(diagnosis.membership.running, 3);
  assert.equal(diagnosis.membership.reserve, 1);
  assert.equal(diagnosis.membership.probe, "passed");
  assert.deepEqual(diagnosis.nodes.map((node) => node.public_endpoint), [null, null, null]);

  const events = [];
  const access = await openFleetWorkerAccess(configPath, {
    runner: docker.run,
    forwarderFactory: async (target, options) => {
      events.push({ event: "open", target, options });
      return {
        endpoint: "http://127.0.0.1:25001",
        async close() { events.push({ event: "close" }); },
      };
    },
  });

  assert.equal(access.endpoint, "http://127.0.0.1:25001");
  assert.equal(access.node, config.nodes[0].name);
  assert.deepEqual(events[0], { event: "open", target: { host: "172.29.0.20", port: 8080 }, options: { scheme: "http" } });
  await access.close();
  assert.deepEqual(events.at(-1), { event: "close" });
  assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
  assert.equal(docker.containers.size, 0);
});

test("fleet startup retries only the bounded repeatable diagnosis mutation", async (t) => {
  await t.test("partial exact probes converge without replaying lifecycle mutations", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    const secret = "transient-stderr-secret";
    const nodeIds = config.nodes.map((node) => node.node_id);
    const expectedNodeIdsSha256 = sha256(nodeIds.join("\n"));
    const attemptSnapshots = [];
    const calls = [];
    const waits = [];
    let attempts = 0;
    const runner = (program, args, options) => {
      calls.push([...args]);
      if (args[0] !== "exec") return docker.run(program, args, options);
      attempts += 1;
      const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
      attemptSnapshots.push(structuredClone(inventory.actions.at(-1)));
      if (attempts === 1) {
        return pinnedCelldDiagnosisOutput(config, { nodeIds: nodeIds.slice(0, 2) });
      }
      if (attempts === 2) {
        return [
          `bounded diagnostic digest input ${secret}`,
          pinnedCelldDiagnosisOutput(config, { nodeIds: nodeIds.slice(0, 2) }),
        ].join("\n");
      }
      return pinnedCelldDiagnosisOutput(config);
    };

    let diagnosis = null;
    let startError = null;
    try {
      diagnosis = startFleet(configPath, {
        runner,
        credentialLauncherPath: process.execPath,
        readinessPolicy: {
          maxAttempts: 3,
          deadlineMs: 1_000,
          backoffMs: 25,
          wait: (milliseconds) => { waits.push(milliseconds); },
        },
      });
    } catch (error) {
      startError = error;
    }
    const startupCalls = [...calls];
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleanup = cleanupFleet(configPath, { runner });

    assert.equal(cleanup.status, "PASS", "startup retry handling must not weaken exact cleanup");
    assert.equal(docker.containers.size, 0);
    assert.ifError(startError);
    assert.equal(diagnosis.status, "READY");
    assert.equal(diagnosis.membership.expected, 3);
    assert.equal(diagnosis.membership.running, 3);
    assert.equal(diagnosis.membership.probe, "passed");
    assert.equal(diagnosis.membership.attempts, 3);
    assert.equal(attempts, 3, "partial exact-node readiness and a transient probe failure must retry diagnosis only");
    assert.deepEqual(waits, [25, 25], "diagnosis retry must apply the configured bounded backoff");
    assert.equal(startupCalls.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "start").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "pull").length, 1);
    assert.equal(startupCalls.filter((args) => args[0] === "run").length, 0, "diagnosis retry must never redeploy the Worker");
    assert.equal(startupCalls.filter((args) => args[0] === "exec").length, 3);

    assert.equal(attemptSnapshots.length, 3);
    for (const [index, action] of attemptSnapshots.entries()) {
      assert.equal(action.kind, "celld_diagnose");
      assert.equal(action.status, "planned", "each repeatable diagnosis attempt must be durable before invocation");
      assert.equal(action.attempt, index + 1);
      assert.equal(action.max_attempts, 3);
      assert.equal(action.deadline_ms, 1_000);
      assert.equal(action.backoff_ms, 25);
      assert.equal(action.expected_node_ids_sha256, expectedNodeIdsSha256);
    }
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.deepEqual(diagnosisActions.map((action) => action.status), ["failed", "failed", "completed"]);
    assert.ok(diagnosisActions.every((action) => /^[0-9a-f]{64}$/.test(action.evidence_sha256)));
    assert.ok(diagnosisActions.every((action) => JSON.stringify(action).length <= 2_048));
    for (const action of diagnosisActions) {
      for (const rawField of ["stdout", "stderr", "error", "output"]) assert.equal(Object.hasOwn(action, rawField), false);
    }
    assert.ok(!JSON.stringify(startupInventory).includes(secret));
  });

  await t.test("the attempt bound fails closed with typed redacted evidence and remains exactly cleanable", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    const secret = "bounded-failure-stderr-secret";
    const calls = [];
    const waits = [];
    let attempts = 0;
    const runner = (program, args, options) => {
      calls.push([...args]);
      if (args[0] === "exec") {
        attempts += 1;
        return [
          `bounded diagnostic digest input ${secret}`,
          pinnedCelldDiagnosisOutput(config, { nodeIds: config.nodes.slice(0, 2).map((node) => node.node_id) }),
        ].join("\n");
      }
      return docker.run(program, args, options);
    };

    let startError = null;
    try {
      startFleet(configPath, {
        runner,
        credentialLauncherPath: process.execPath,
        readinessPolicy: {
          maxAttempts: 3,
          deadlineMs: 1_000,
          backoffMs: 10,
          wait: (milliseconds) => { waits.push(milliseconds); },
        },
      });
    } catch (error) {
      startError = error;
    }
    const startupCalls = [...calls];
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleanup = cleanupFleet(configPath, { runner });

    assert.equal(cleanup.status, "PASS", "bounded readiness failure must preserve exact cleanup behavior");
    assert.equal(docker.containers.size, 0);
    assert.equal(startupCalls.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "start").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "exec").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "run").length, 0);
    assert.equal(attempts, 3);
    assert.deepEqual(waits, [10, 10]);
    assert.equal(startError?.name, "FleetStartupReadinessError");
    assert.equal(startError?.exitCode, 3);
    assert.ok(!startError.message.includes(secret));
    assert.equal(startError.evidence?.schema_version, "agentic-sandbox.celld-fleet-diagnosis/v1");
    assert.equal(startError.evidence?.status, "NOT_READY");
    assert.equal(startError.evidence?.reason_code, "CELLD_FLEET_STARTUP_NOT_READY");
    assert.equal(startError.evidence?.membership?.expected, 3);
    assert.equal(startError.evidence?.membership?.running, 3);
    assert.equal(startError.evidence?.membership?.probe, "failed");
    assert.equal(startError.evidence?.failure?.attempts, 3);
    assert.match(startError.evidence?.failure?.evidence_sha256 ?? "", /^[0-9a-f]{64}$/);
    assert.ok(!JSON.stringify(startError.evidence).includes(secret));
    assert.ok(!JSON.stringify(startupInventory).includes(secret));
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.equal(diagnosisActions.length, 3);
    assert.ok(diagnosisActions.every((action) => action.status === "failed" && /^[0-9a-f]{64}$/.test(action.evidence_sha256)));
  });

  await t.test("the readiness deadline stops further diagnosis attempts before the larger attempt cap", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    const calls = [];
    const waits = [];
    let elapsedMs = 0;
    let attempts = 0;
    const runner = (program, args, options) => {
      calls.push([...args]);
      if (args[0] === "exec") {
        attempts += 1;
        return pinnedCelldDiagnosisOutput(config, { nodeIds: config.nodes.slice(0, 2).map((node) => node.node_id) });
      }
      return docker.run(program, args, options);
    };
    let startError = null;
    try {
      startFleet(configPath, {
        runner,
        credentialLauncherPath: process.execPath,
        readinessPolicy: {
          maxAttempts: 5,
          deadlineMs: 15,
          backoffMs: 10,
          clock: () => elapsedMs,
          wait: (milliseconds) => { waits.push(milliseconds); elapsedMs += milliseconds; },
        },
      });
    } catch (error) {
      startError = error;
    }
    const startupCalls = [...calls];
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleanup = cleanupFleet(configPath, { runner });

    assert.equal(cleanup.status, "PASS");
    assert.equal(docker.containers.size, 0);
    assert.equal(startError?.name, "FleetStartupReadinessError");
    assert.equal(startError?.exitCode, 3);
    assert.equal(attempts, 2, "the deadline must stop a third attempt even though maxAttempts permits five");
    assert.deepEqual(waits, [10]);
    assert.equal(startupCalls.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "start").length, 3);
    assert.equal(startupCalls.filter((args) => args[0] === "exec").length, 2);
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.equal(diagnosisActions.length, 2);
    assert.ok(diagnosisActions.every((action) => action.status === "failed" && action.deadline_ms === 15));
  });
});

test("fleet startup CLI emits a nonempty typed diagnosis artifact on bounded readiness failure", () => {
  const { root, config, configPath, inventoryPath } = fixture();
  const fakeBin = join(root, "fake-bin");
  const fakeDocker = join(fakeBin, "docker");
  const statePath = join(root, "fake-docker-state.json");
  const secret = "cli-stderr-secret-must-not-leak";
  mkdirSync(fakeBin, { mode: 0o700 });
  writeFileSync(fakeDocker, readFileSync(STARTUP_READINESS_DOCKER), { mode: 0o700 });
  chmodSync(fakeDocker, 0o700);
  const env = {
    ...process.env,
    PATH: `${fakeBin}:${process.env.PATH ?? ""}`,
    CELLD_FAKE_DOCKER_STATE: statePath,
    CELLD_FAKE_FLEET_CONFIG: configPath,
    CELLD_FAKE_PINNED_DIAGNOSIS: PINNED_DIAGNOSIS_PATH,
    CELLD_FAKE_DIAGNOSIS_SECRET: secret,
  };

  const started = spawnSync(process.execPath, [
    FLEET_FIXTURE_CLI,
    "start",
    "--config", configPath,
    "--credential-launcher", process.execPath,
  ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });
  const cleaned = spawnSync(process.execPath, [
    FLEET_FIXTURE_CLI,
    "cleanup",
    "--config", configPath,
  ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });
  const state = JSON.parse(readFileSync(statePath, "utf8"));

  assert.equal(cleaned.status, 0, cleaned.stderr);
  assert.equal(Object.keys(state.containers).length, 0, "typed startup failure must remain exactly cleanable");
  assert.equal(started.status, 3);
  assert.ok(started.stdout.trim().length > 0, "stdout redirection must never leave a zero-byte diagnosis artifact");
  const evidence = JSON.parse(started.stdout);
  assert.equal(evidence.schema_version, "agentic-sandbox.celld-fleet-diagnosis/v1");
  assert.equal(evidence.run_id, config.run_id);
  assert.equal(evidence.status, "NOT_READY");
  assert.equal(evidence.reason_code, "CELLD_FLEET_STARTUP_NOT_READY");
  assert.equal(evidence.membership.expected, 3);
  assert.equal(evidence.membership.running, 3);
  assert.equal(evidence.membership.probe, "failed");
  assert.match(evidence.failure.evidence_sha256, /^[0-9a-f]{64}$/);
  assert.ok(state.diagnosis_attempts >= 2 && state.diagnosis_attempts <= 5, "CLI diagnosis attempts must remain bounded");
  assert.ok(!started.stdout.includes(secret));
  assert.ok(!started.stderr.includes(secret));
  assert.ok(!JSON.stringify(JSON.parse(readFileSync(inventoryPath, "utf8"))).includes(secret));
  assert.equal(state.commands.filter((args) => args[0] === "create").length, 3);
  assert.equal(state.commands.filter((args) => args[0] === "start").length, 3);
  assert.equal(state.commands.filter((args) => args[0] === "exec").length, state.diagnosis_attempts);
});

test("arbitrary non-transient subprocess stderr cannot opt itself into startup retries", () => {
  const { root, config, configPath, inventoryPath } = fixture();
  const fakeBin = join(root, "fake-bin-nontransient");
  const fakeDocker = join(fakeBin, "docker");
  const statePath = join(root, "fake-docker-nontransient-state.json");
  const secret = "nontransient-stderr-secret-must-not-leak";
  mkdirSync(fakeBin, { mode: 0o700 });
  writeFileSync(fakeDocker, readFileSync(STARTUP_READINESS_DOCKER), { mode: 0o700 });
  chmodSync(fakeDocker, 0o700);
  const env = {
    ...process.env,
    PATH: `${fakeBin}:${process.env.PATH ?? ""}`,
    CELLD_FAKE_DOCKER_STATE: statePath,
    CELLD_FAKE_FLEET_CONFIG: configPath,
    CELLD_FAKE_PINNED_DIAGNOSIS: PINNED_DIAGNOSIS_PATH,
    CELLD_FAKE_DIAGNOSIS_SECRET: secret,
    CELLD_FAKE_DIAGNOSIS_MODE: "nontransient",
  };

  const started = spawnSync(process.execPath, [
    FLEET_FIXTURE_CLI,
    "start",
    "--config", configPath,
    "--credential-launcher", process.execPath,
  ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });
  const startupState = JSON.parse(readFileSync(statePath, "utf8"));
  const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  const cleaned = spawnSync(process.execPath, [
    FLEET_FIXTURE_CLI,
    "cleanup",
    "--config", configPath,
  ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });

  assert.equal(cleaned.status, 0, cleaned.stderr);
  assert.equal(started.status, 3);
  assert.equal(startupState.diagnosis_attempts, 1, "exit 42 is a typed non-transient command failure even when stderr says lease, peer, and startup");
  const evidence = JSON.parse(started.stdout);
  assert.equal(evidence.status, "NOT_READY");
  assert.equal(evidence.retryable, false);
  assert.equal(evidence.failure.reason_code, "CELLD_DIAGNOSIS_NONRETRYABLE_FAILURE");
  assert.match(evidence.failure.evidence_sha256, /^[0-9a-f]{64}$/);
  assert.ok(!started.stdout.includes(secret));
  assert.ok(!started.stderr.includes(secret));
  assert.ok(!JSON.stringify(startupInventory).includes(secret));
  const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
  assert.equal(diagnosisActions.length, 1);
  assert.equal(diagnosisActions[0].status, "failed");
  assert.equal(diagnosisActions[0].reason_code, "CELLD_DIAGNOSIS_NONRETRYABLE_FAILURE");
});

test("real pinned Celld subprocess outcomes authorize only exact diagnosis retries", async (t) => {
  async function runCliMode(mode, label) {
    const { root, config, configPath, inventoryPath } = fixture();
    const fakeBin = join(root, `fake-bin-${label}`);
    const fakeDocker = join(fakeBin, "docker");
    const statePath = join(root, `fake-docker-${label}-state.json`);
    const secret = `${label}-stderr-must-not-control-retry`;
    mkdirSync(fakeBin, { mode: 0o700 });
    writeFileSync(fakeDocker, readFileSync(STARTUP_READINESS_DOCKER), { mode: 0o700 });
    chmodSync(fakeDocker, 0o700);
    const env = {
      ...process.env,
      PATH: `${fakeBin}:${process.env.PATH ?? ""}`,
      CELLD_FAKE_DOCKER_STATE: statePath,
      CELLD_FAKE_FLEET_CONFIG: configPath,
      CELLD_FAKE_PINNED_DIAGNOSIS: PINNED_DIAGNOSIS_PATH,
      CELLD_FAKE_DIAGNOSIS_SECRET: secret,
      CELLD_FAKE_DIAGNOSIS_MODE: mode,
    };
    const started = spawnSync(process.execPath, [
      FLEET_FIXTURE_CLI,
      "start",
      "--config", configPath,
      "--credential-launcher", process.execPath,
    ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });
    const startupState = JSON.parse(readFileSync(statePath, "utf8"));
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleaned = spawnSync(process.execPath, [
      FLEET_FIXTURE_CLI,
      "cleanup",
      "--config", configPath,
    ], { cwd: REPO_ROOT, env, encoding: "utf8", timeout: 10_000 });
    assert.equal(cleaned.status, 0, cleaned.stderr);
    assert.ok(!started.stdout.includes(secret));
    assert.ok(!started.stderr.includes(secret));
    assert.ok(!JSON.stringify(startupInventory).includes(secret));
    return { config, started, startupState, startupInventory, secret };
  }

  await t.test("exit 1 with zero then partial exact signed peers converges from stdout alone", async () => {
    const { started, startupState, startupInventory } = await runCliMode("incomplete-converge", "incomplete-converge");
    assert.equal(started.status, 0, started.stderr);
    const evidence = JSON.parse(started.stdout);
    assert.equal(evidence.status, "READY");
    assert.equal(evidence.membership.attempts, 3);
    assert.equal(startupState.diagnosis_attempts, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "pull").length, 1);
    assert.equal(startupState.commands.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "start").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "exec").length, 3);
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.deepEqual(diagnosisActions.map((action) => action.status), ["failed", "failed", "completed"]);
  });

  await t.test("transient Docker port publication lag falls back to exact private node inspection without trusting stderr", async () => {
    const { started, startupState, startupInventory, secret } = await runCliMode("port-race-once", "port-race-once");
    assert.equal(started.status, 0, started.stderr);
    const evidence = JSON.parse(started.stdout);
    assert.equal(evidence.status, "READY");
    assert.equal(evidence.membership.attempts, 1);
    assert.equal(startupState.port_attempts, 3);
    assert.equal(startupState.diagnosis_attempts, 1);
    assert.equal(startupState.commands.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "start").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "exec").length, 1);
    assert.equal(startupState.commands.filter((args) => args[0] === "port").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "network" && args[1] === "inspect").length, 2);
    assert.ok(!started.stdout.includes(secret));
    assert.ok(!started.stderr.includes(secret));
    assert.ok(!JSON.stringify(startupInventory).includes(secret));
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.deepEqual(diagnosisActions.map((action) => action.status), ["completed"]);
  });

  await t.test("exit 1 with exact-looking stderr and no controller stdout never retries", async () => {
    const { started, startupState, startupInventory } = await runCliMode("stderr-spoof", "stderr-spoof");
    assert.equal(started.status, 3);
    assert.equal(startupState.diagnosis_attempts, 1);
    const evidence = JSON.parse(started.stdout);
    assert.equal(evidence.status, "NOT_READY");
    assert.equal(evidence.retryable, false);
    assert.equal(evidence.failure.reason_code, "CELLD_DIAGNOSIS_NONRETRYABLE_FAILURE");
    assert.equal(startupInventory.actions.filter((action) => action.kind === "celld_diagnose").length, 1);
  });

  await t.test("an unrelated Docker exit 75 cannot impersonate the exact diagnosis command", async () => {
    const { started, startupState } = await runCliMode("unrelated-exit75", "unrelated-exit75");
    assert.equal(started.status, 3);
    assert.equal(startupState.diagnosis_attempts, 0);
    assert.equal(startupState.commands.filter((args) => args[0] === "exec").length, 0);
    assert.equal(startupState.commands.filter((args) => args[0] === "port").length, 1);
    assert.equal(startupState.commands.filter((args) => args[0] === "create").length, 3);
    assert.equal(startupState.commands.filter((args) => args[0] === "start").length, 3);
  });
});

test("fleet startup deadline governs every diagnosis subprocess and terminal", async (t) => {
  await t.test("inspect and port work consume the total deadline and block later invocation", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    const deadlineMs = 10;
    const diagnosisCalls = [];
    let elapsedMs = 0;
    let starts = 0;
    const runner = (program, args, options = {}) => {
      if (args[0] === "start") {
        const result = docker.run(program, args, options);
        starts += 1;
        return result;
      }
      if (starts === 3 && ["inspect", "port", "exec"].includes(args[0])) {
        diagnosisCalls.push({ command: args[0], invokedAtMs: elapsedMs, timeout: options.timeout });
        const result = docker.run(program, args, options);
        elapsedMs += 3;
        return result;
      }
      return docker.run(program, args, options);
    };
    let startError = null;
    try {
      startFleet(configPath, {
        runner,
        credentialLauncherPath: process.execPath,
        readinessPolicy: {
          maxAttempts: 4,
          deadlineMs,
          backoffMs: 0,
          clock: () => elapsedMs,
          wait: () => {},
        },
      });
    } catch (error) {
      startError = error;
    }
    const startupCalls = structuredClone(diagnosisCalls);
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleanup = cleanupFleet(configPath, { runner });

    assert.equal(cleanup.status, "PASS");
    assert.equal(startError?.name, "FleetStartupReadinessError");
    assert.equal(startError?.evidence?.status, "NOT_READY");
    assert.match(startError?.evidence?.failure?.evidence_sha256 ?? "", /^[0-9a-f]{64}$/);
    assert.ok(startupCalls.length > 0);
    assert.ok(startupCalls.every((call) => call.invokedAtMs < deadlineMs), "no inspect, port, or exec subprocess may begin at or after the total deadline");
    assert.ok(startupCalls.every((call) => Number.isSafeInteger(call.timeout)
      && call.timeout > 0
      && call.timeout <= deadlineMs - call.invokedAtMs), "each diagnosis subprocess receives only its remaining total-deadline budget");
    assert.equal(startupCalls.filter((call) => call.command === "exec").length, 0, "the conditional-write and peer probe must not start after inspect/port exhaust the deadline");
    assert.ok(startupInventory.actions.filter((action) => action.kind === "celld_diagnose").every((action) => action.status !== "completed"));
  });

  await t.test("a probe returning after its deadline is durable failure evidence, never READY", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    const deadlineMs = 10;
    let elapsedMs = 0;
    let starts = 0;
    let execTimeout = null;
    const runner = (program, args, options = {}) => {
      if (args[0] === "start") {
        const result = docker.run(program, args, options);
        starts += 1;
        return result;
      }
      if (starts === 3 && args[0] === "exec") {
        execTimeout = options.timeout;
        const result = docker.run(program, args, options);
        elapsedMs = deadlineMs + 1;
        return result;
      }
      return docker.run(program, args, options);
    };
    let startError = null;
    try {
      startFleet(configPath, {
        runner,
        credentialLauncherPath: process.execPath,
        readinessPolicy: {
          maxAttempts: 2,
          deadlineMs,
          backoffMs: 0,
          clock: () => elapsedMs,
          wait: () => {},
        },
      });
    } catch (error) {
      startError = error;
    }
    const startupInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
    const cleanup = cleanupFleet(configPath, { runner });

    assert.equal(cleanup.status, "PASS");
    assert.equal(execTimeout, deadlineMs);
    assert.equal(startError?.name, "FleetStartupReadinessError");
    assert.equal(startError?.evidence?.status, "NOT_READY");
    assert.equal(startError?.evidence?.failure?.reason_code, "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED");
    const diagnosisActions = startupInventory.actions.filter((action) => action.kind === "celld_diagnose");
    assert.equal(diagnosisActions.length, 1);
    assert.equal(diagnosisActions[0].status, "failed", "late successful bytes cannot terminalize the diagnosis action as completed");
    assert.equal(diagnosisActions[0].reason_code, "CELLD_DIAGNOSIS_DEADLINE_EXCEEDED");
  });
});

test("fleet readiness accepts only the exact pinned signed-direct expected-node set", async (t) => {
  const vectors = [
    {
      name: "all three exact expected signed-direct IDs",
      output: (config) => pinnedCelldDiagnosisOutput(config),
      status: "READY",
    },
    {
      name: "all three signed-direct IDs with upstream unknown telemetry",
      output: (config) => pinnedCelldDiagnosisOutput(config, {
        telemetry: {
          rss_bytes: "unknown",
          in_use_bytes: "unknown",
          load_age_ms: "unknown",
        },
      }),
      status: "READY",
    },
    {
      name: "all three signed-direct IDs with live variable telemetry formatting",
      output: (config) => pinnedCelldDiagnosisOutput(config, {
        telemetry: {
          resident_cells: "unknown",
          websockets: "unknown",
          rss_bytes: "unknown",
          in_use_bytes: "unknown",
          cpu_percent: "0",
          fds: "unknown/unknown",
          pressured: "unknown",
          shed_cells: "unknown",
          restoring: "unknown",
          load_age_ms: "unknown",
        },
      }),
      status: "READY",
    },
    {
      name: "all three signed-direct IDs with reordered and added upstream telemetry",
      output: (config) => signedDirectDiagnosisOutput(config, {
        fieldEntries: () => [
          ["upstream_metric", "not_available"],
          ["load_age_ms", "unknown"],
          ["protocol", "2"],
          ["pressured", "unknown"],
          ["fds", "unknown/unknown"],
          ["cpu_percent", "0"],
          ["in_use_bytes", "unknown"],
          ["rss_bytes", "unknown"],
          ["websockets", "unknown"],
          ["resident_cells", "unknown"],
          ["restoring", "unknown"],
          ["shed_cells", "unknown"],
        ],
      }),
      status: "READY",
    },
    {
      name: "all three signed-direct IDs with bare host-port address form",
      output: (config) => signedDirectDiagnosisOutput(config, { includeAt: false }),
      status: "READY",
    },
    {
      name: "signed-direct IDs with malformed telemetry still fail closed",
      output: (config) => pinnedCelldDiagnosisOutput(config, {
        telemetry: {
          cpu_percent: "NaN",
        },
      }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED",
    },
    {
      name: "signed-direct IDs with unsupported old protocol still fail closed",
      output: (config) => signedDirectDiagnosisOutput(config, {
        fieldEntries: () => [
          ["protocol", "1"],
          ["resident_cells", "0"],
        ],
      }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED",
    },
    {
      name: "signed-direct IDs with duplicate telemetry still fail closed",
      output: (config) => signedDirectDiagnosisOutput(config, {
        fieldEntries: () => [
          ["protocol", "2"],
          ["resident_cells", "0"],
          ["resident_cells", "1"],
        ],
      }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED",
    },
    {
      name: "signed-direct IDs with malformed extra telemetry still fail closed",
      output: (config) => [
        PINNED_DIAGNOSIS_OUTPUT.conditional_write_line,
        ...config.nodes.map((node) => `ok peer ${node.node_id} at ${node.advertise} (signed direct probe) protocol=2 extra=bad;`),
      ].join("\n"),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED",
    },
    {
      name: "one signed-direct foreign ID",
      output: (config) => pinnedCelldDiagnosisOutput(config, {
        nodeIds: [...config.nodes.slice(0, 2).map((node) => node.node_id), "foreign-node-session"],
      }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_NODE_IDENTITY_INVALID",
    },
    {
      name: "duplicate signed-direct expected ID",
      output: (config) => pinnedCelldDiagnosisOutput(config, {
        nodeIds: [config.nodes[0].node_id, config.nodes[1].node_id, config.nodes[1].node_id],
      }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_NODE_IDENTITY_INVALID",
    },
    {
      name: "expected IDs without the signed-direct marker",
      output: (config) => pinnedCelldDiagnosisOutput(config, { signed: false }),
      status: "NOT_READY",
      reasonCode: "CELLD_DIAGNOSIS_SIGNED_PEER_PROOF_REQUIRED",
    },
  ];
  for (const vector of vectors) {
    await t.test(vector.name, () => {
      const { config, storage, configPath } = fixture();
      const docker = new FakeDocker(config, storage);
      for (const node of config.nodes) {
        docker.containers.set(node.name, { labels: docker.labels(), image: config.pins.celld.image_ref, running: true });
      }
      const runner = (program, args, options) => args[0] === "exec"
        ? vector.output(config)
        : docker.run(program, args, options);
      const diagnosis = diagnoseFleet(configPath, { runner });
      const cleanup = cleanupFleet(configPath, { runner: docker.run });

      assert.equal(cleanup.status, "PASS");
      assert.equal(diagnosis.status, vector.status);
      if (vector.status === "READY") {
        assert.equal(diagnosis.membership.probe, "passed");
        assert.match(diagnosis.membership.probe_sha256, /^[0-9a-f]{64}$/);
        return;
      }
      assert.equal(diagnosis.retryable, false, "wrong, duplicate, and unsigned output is invalid rather than transient");
      assert.equal(diagnosis.membership.probe, "failed");
      assert.equal(diagnosis.failure.reason_code, vector.reasonCode);
      assert.match(diagnosis.failure.evidence_sha256, /^[0-9a-f]{64}$/);
    });
  }
});

test("diagnosis plan persistence crosses file and directory durability barriers before invocation", async (t) => {
  await t.test("temp fsync, atomic rename, and parent fsync precede the conditional-write probe", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    assert.equal(startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath }).status, "READY");
    const events = [];
    const fsOperations = tracingPersistenceFs(events);
    const runner = (program, args, options) => {
      if (args[0] === "exec") events.push({ operation: "invoke", command: "docker exec" });
      return docker.run(program, args, options);
    };
    const diagnosis = diagnoseFleet(configPath, { runner, fsOperations });
    const cleanup = cleanupFleet(configPath, { runner: docker.run });

    assert.equal(cleanup.status, "PASS");
    assert.equal(diagnosis.status, "READY");
    const invokeIndex = events.findIndex((event) => event.operation === "invoke");
    const renameIndex = events.findIndex((event, index) => index < invokeIndex && event.operation === "rename" && event.to === inventoryPath);
    assert.ok(renameIndex >= 0, "the planned diagnosis record is atomically renamed before invocation");
    const temporary = events[renameIndex].from;
    const temporaryFsyncIndex = events.findIndex((event, index) => index < renameIndex && event.operation === "fsync" && event.path === temporary);
    const parentFsyncIndex = events.findIndex((event, index) => index > renameIndex && index < invokeIndex
      && event.operation === "fsync" && event.path === dirname(inventoryPath));
    assert.ok(temporaryFsyncIndex >= 0, "the complete temporary inventory file is fsynced before rename");
    assert.ok(parentFsyncIndex >= 0, "the inventory parent directory is fsynced after rename and before diagnosis invocation");
  });

  await t.test("a crash at the post-rename directory barrier prevents diagnosis invocation", () => {
    const { config, storage, configPath, inventoryPath } = fixture();
    const docker = new FakeDocker(config, storage);
    assert.equal(startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath }).status, "READY");
    const events = [];
    const fsOperations = tracingPersistenceFs(events, { failDirectoryFsync: dirname(inventoryPath) });
    let execInvocations = 0;
    let diagnosisError = null;
    const runner = (program, args, options) => {
      if (args[0] === "exec") execInvocations += 1;
      return docker.run(program, args, options);
    };
    try {
      diagnoseFleet(configPath, { runner, fsOperations });
    } catch (error) {
      diagnosisError = error;
    }
    const cleanup = cleanupFleet(configPath, { runner: docker.run });

    assert.equal(cleanup.status, "PASS");
    assert.match(diagnosisError?.message ?? "", /injected crash before diagnosis parent-directory fsync/);
    assert.equal(execInvocations, 0, "the repeatable conditional-write mutation cannot run before the plan rename is directory-durable");
    const renameIndex = events.findIndex((event) => event.operation === "rename" && event.to === inventoryPath);
    const failedBarrierIndex = events.findIndex((event) => event.operation === "fsync" && event.path === dirname(inventoryPath));
    assert.ok(renameIndex >= 0 && failedBarrierIndex > renameIndex, "the injected crash is exactly after rename and at the parent durability barrier");
  });

});

test("cleanup reclaims an exact same-run unique pre-rename diagnosis crash residue", () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  const staleTemporary = `${inventoryPath}.new-99999999-deadbeefdeadbeef`;
  const sameRunInventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  writeFileSync(staleTemporary, `${JSON.stringify(sameRunInventory, null, 2)}\n`, { mode: 0o600, flag: "wx" });

  const cleanup = cleanupFleet(configPath, { runner: docker.run });

  assert.equal(cleanup.status, "PASS");
  assert.equal(existsSync(staleTemporary), false, "a protected same-run abandoned atomic-write file must not survive cleanup unreported");
  assert.equal(existsSync(inventoryPath), true, "crash-residue cleanup must preserve the authoritative inventory");
});

test("callback relays share only node loopback and authenticate management with protected mTLS material", () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath });
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

test("callback relay response-loss injection is explicit and remains disabled by default", () => {
  const { config, storage, configPath } = fixture();
  const docker = new FakeDocker(config, storage);
  docker.expectedFaultSignal = "enabled";
  startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath });
  const result = startCallbackRelays(configPath, {
    runner: docker.run,
    relayBinaryPath: process.execPath,
    enableFaultSignal: true,
  });
  assert.ok(result.relays.every((relay) => relay.response_loss_signal === "SIGUSR1"));
  assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
});

test("deployed Worker probe proves signed readiness and denials without retaining auth material", async () => {
  const { config, storage, configPath, inventoryPath } = fixture();
  const docker = new FakeDocker(config, storage);
  startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath });
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

test("each node post-creation termination boundary is crash-resumable and teardown is idempotent", () => {
  for (let failure = 1; failure <= 3; failure += 1) {
    const { config, storage, configPath } = fixture();
    const docker = new FakeDocker(config, storage);
    docker.failAfterCreate = failure;
    assert.throws(() => startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath }), /injected termination after create/);
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
    assert.equal(existsSync(config.worker_vars_file_ref), false);
    assert.ok(JSON.parse(readFileSync(join(config.run_root, "fleet-inventory.json"), "utf8")).resources.every((resource) => resource.status === "removed"));
    assert.deepEqual(cleanupFleet(configPath, { runner: docker.run }), { status: "PASS", run_id: config.run_id, scope: "single-host multi-node", removed_containers: 3, residue: [] });
  }
});

test("each protected preparation creation boundary is crash-resumable and teardown is idempotent", () => {
  for (let failure = 1; failure <= 6; failure += 1) {
    const runId = `test-${randomUUID()}`;
    const root = join(TEST_ROOT, runId);
    roots.add(root);
    const storage = prepareFixture({ fixtureProfile: "titan-single-host-storage", runId, root });
    let creations = 0;
    assert.throws(
      () => prepareFleet({
        storageConfigPath: join(root, "fixture.json"),
        afterCreate: () => {
          creations += 1;
          if (creations === failure) throw new Error("injected termination after protected creation");
        },
      }),
      /injected termination after protected creation/,
    );
    const configPath = join(root, "fleet.json");
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    const docker = new FakeDocker(config, storage);
    if (failure === 1) assert.throws(() => startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath }), /Worker vars file is absent/);
    assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
    assert.ok(JSON.parse(readFileSync(join(root, "fleet-inventory.json"), "utf8")).resources.every((resource) => resource.status === "removed"));
    assert.equal(cleanupFleet(configPath, { runner: docker.run }).status, "PASS");
  }
});

test("each callback-relay post-creation termination boundary is crash-resumable and teardown is idempotent", () => {
  for (let failure = 4; failure <= 6; failure += 1) {
    const { config, storage, configPath } = fixture();
    const docker = new FakeDocker(config, storage);
    startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath });
    docker.failAfterCreate = failure;
    assert.throws(
      () => startCallbackRelays(configPath, { runner: docker.run, relayBinaryPath: process.execPath }),
      /injected termination after create/,
    );
    assert.equal(cleanupFleet(configPath, { runner: docker.run }).removed_containers, failure);
    assert.equal(docker.containers.size, 0);
    assert.equal(cleanupFleet(configPath, { runner: docker.run }).removed_containers, failure);
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
    assert.throws(() => startFleet(configPath, { runner: docker.run, credentialLauncherPath: process.execPath }), /Worker vars file is not ready/);
    await assert.rejects(
      deployFleetWorker(configPath, {
        runner: docker.run,
        esbuildPath: process.execPath,
        credentialLauncherPath: process.execPath,
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
