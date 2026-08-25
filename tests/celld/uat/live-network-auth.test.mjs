import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  applyDirectionalPartition,
  applyListenerGuard,
  cleanupMtlsProxies,
  cleanupNetworkAuthInventories,
  cleanupProbeResources,
  createNetworkAuthInventory,
  directionalPartitionCommands,
  driverErrorDocument as networkAuthDriverErrorDocument,
  executeNetworkAuthDriver,
  healDirectionalPartition,
  mapBounded,
  listenerGuardCommands,
  mtlsNegativeIdentityFiles,
  mtlsProxyCreateArgs,
  networkAuthInventoryLocations,
  observeFleetNetworkNamespaces,
  planMtlsProxy,
  planDirectionalPartition,
  planListenerGuard,
  privateCelldRoute,
  prepareMtlsProxyCertificates,
  probeEnvironmentProxy,
  probeProxyBypass,
  probeMtlsTransportNegatives,
  readManagementProviderCounter,
  recoverNetworkAuthInventory,
  registerNetworkNamespace,
  removeListenerGuard,
  startMtlsProxy,
  validateNetworkAuthInventory,
  validateDirectionalRouteMatrices,
  validateTcpProbeResult,
  waitManagementCelldStatus,
  waitManagementProviderLedger,
  waitMtlsProxies,
} from "../../../scripts/celld-live-network-auth.mjs";

test("network/auth driver error document redacts unsafe fields", () => {
  const error = new Error("private endpoint token should not be logged");
  error.name = "Unsafe Error Name With Spaces";
  error.operation = "network-auth.run-authentication";
  error.scenarioId = "UAT-CELLD-012";
  error.code = "ERR_INVALID_ARG_TYPE";
  error.signal = "SIGTERM";
  const document = networkAuthDriverErrorDocument(error);
  assert.equal(document.name, `sha256:${createHash("sha256").update(error.name).digest("hex")}`);
  assert.equal(document.operation, "network-auth.run-authentication");
  assert.equal(document.scenario_id, "UAT-CELLD-012");
  assert.equal(document.node_code, "ERR_INVALID_ARG_TYPE");
  assert.equal(document.signal, "SIGTERM");
  assert.match(document.message_sha256, /^[0-9a-f]{64}$/);
  assert.equal(JSON.stringify(document).includes("private endpoint"), false);
});

test("provider effects are read from the protected management ledger", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-provider-counter-test-"));
  try {
    const ledger = join(directory, "effect-ledger.sqlite");
    writeFileSync(ledger, "fixture", { mode: 0o600 });
    const calls = [];
    const value = readManagementProviderCounter({ fleet: { callback: { effect_ledger_file_ref: ledger } } }, {
      runner: (program, args, options) => {
        calls.push({ program, args, options });
        return "17\n";
      },
    });
    assert.equal(value, 17);
    assert.equal(calls[0].program, "sqlite3");
    assert.deepEqual(calls[0].args.slice(0, 4), ["-readonly", "-batch", "-noheader", ledger]);
    assert.match(calls[0].args[4], /SUM\(provider_dispatch_count\)/);
    chmodSync(ledger, 0o644);
    assert.throws(() => readManagementProviderCounter({ fleet: { callback: { effect_ledger_file_ref: ledger } } }, {
      runner: () => { throw new Error("insecure ledger must fail before sqlite"); },
    }), /not protected/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("provider ledger readiness waits for management startup to create the ledger", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-provider-ledger-wait-test-"));
  try {
    const ledger = join(directory, "effect-ledger.sqlite");
    let calls = 0;
    setTimeout(() => writeFileSync(ledger, "fixture", { mode: 0o600 }), 10);
    const value = await waitManagementProviderLedger({ fleet: { callback: { effect_ledger_file_ref: ledger } } }, {
      timeoutMs: 1_000,
      intervalMs: 1,
      runner: () => {
        calls += 1;
        return "23\n";
      },
    });
    assert.equal(value, 23);
    assert.equal(calls, 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("management Celld status readiness fails fast on invalid startup config", async () => {
  const configured = await waitManagementCelldStatus({}, {}, {
    timeoutMs: 1_000,
    intervalMs: 1,
    requester: async () => ({
      statusCode: 200,
      body: JSON.stringify({ status: { enabled: true, configured: true, unavailable_code: null }, configuration_error: null }),
    }),
  });
  assert.equal(configured.status.configured, true);

  await assert.rejects(
    () => waitManagementCelldStatus({}, {}, {
      timeoutMs: 1_000,
      intervalMs: 1,
      requester: async () => ({
        statusCode: 200,
        body: JSON.stringify({ status: { enabled: true, configured: false, unavailable_code: null }, configuration_error: "private startup detail" }),
      }),
    }),
    (error) => error.errorCode === "CELLD_MANAGEMENT_CELLD_CONFIG_INVALID" && /^[0-9a-f]{64}$/.test(error.evidenceSha256) && /^[0-9a-f]{64}$/.test(error.stderrSha256),
  );
});

test("bounded probe pool preserves order and never exceeds 32 in flight", async () => {
  let active = 0;
  let observed = 0;
  const statistics = {};
  const values = Array.from({ length: 160 }, (_value, index) => index);
  const results = await mapBounded(values, 32, async (value) => {
    active += 1;
    observed = Math.max(observed, active);
    await new Promise((resolvePromise) => setImmediate(resolvePromise));
    active -= 1;
    return value * 2;
  }, statistics);
  assert.deepEqual(results, values.map((value) => value * 2));
  assert.equal(observed, 32);
  assert.equal(statistics.max_in_flight, 32);
  await assert.rejects(() => mapBounded(values, 33, async (value) => value), /concurrency from 1 through 32/);
});

test("route probe evidence binds every raw attempt to its aggregate", () => {
  const observations = [
    { attempt: 0, started_at: "2026-08-23T08:00:00Z", ended_at: "2026-08-23T08:00:01Z", connected: false },
    { attempt: 1, started_at: "2026-08-23T08:00:00Z", ended_at: "2026-08-23T08:00:01Z", connected: true },
  ];
  assert.equal(validateTcpProbeResult({ attempts: 2, succeeded: 1, denied: 1, max_in_flight: 2, observations }, 2).denied, 1);
  assert.throws(() => validateTcpProbeResult({ attempts: 2, succeeded: 0, denied: 2, max_in_flight: 2, observations }, 2), /aggregate does not match/);
  assert.throws(() => validateTcpProbeResult({ attempts: 2, succeeded: 1, denied: 1, max_in_flight: 33, observations }, 2), /invalid bounded evidence/);
});

test("fleet namespace observation joins exact Docker ownership, inode, and private address", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  const fleet = { run_id: "titan-765", network: { name: "celld-private" }, nodes: [{ name: "celld-fleet-node-1" }, { name: "celld-fleet-node-2" }] };
  const observed = observeFleetNetworkNamespaces(inventory, fleet, {
    runner: (_program, args) => {
      const index = args.at(-1).endsWith("1") ? 1 : 2;
      return JSON.stringify([{
        Id: String(index).repeat(64),
        State: { Running: true, Pid: 3200 + index },
        Config: { Labels: { "dev.agentic-sandbox.run": "titan-765", "dev.agentic-sandbox.scope": "celld-qualification" } },
        NetworkSettings: { Networks: { "celld-private": { IPAddress: `172.30.0.${20 + index}` } } },
      }]);
    },
    namespaceInode: (pid) => 4026533000 + pid,
    now: new Date("2026-08-23T08:00:00Z"),
  });
  assert.deepEqual(observed.map((entry) => entry.address), ["172.30.0.21", "172.30.0.22"]);
  assert.deepEqual(observed.map((entry) => entry.container_id), ["1".repeat(64), "2".repeat(64)]);
  assert.deepEqual(inventory.namespaces.map((entry) => entry.inode), [4026536201, 4026536202]);
  assert.throws(() => observeFleetNetworkNamespaces(createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" }), fleet, {
    runner: () => JSON.stringify([{ State: { Running: true, Pid: 3 }, Config: { Labels: { "dev.agentic-sandbox.run": "foreign" } }, NetworkSettings: { Networks: { "celld-private": { IPAddress: "172.30.0.21" } } } }]),
    namespaceInode: () => 44,
  }), /not exact-run owned/);
});

test("directional partition planner binds typed nft identities to an exact namespace", () => {
  const inventory = createNetworkAuthInventory({
    runId: "titan-765",
    runRoot: "/dev/shm/celld-qualification/titan-765",
    host: "titan",
    now: new Date("2026-08-23T08:00:00Z"),
  });
  registerNetworkNamespace(inventory, {
    container: "celld-fleet-node-1",
    pid: 3210,
    inode: 4026533001,
    runLabel: "titan-765",
  }, new Date("2026-08-23T08:00:01Z"));
  const fault = planDirectionalPartition(inventory, {
    direction: "celld_to_store",
    sourceContainer: "celld-fleet-node-1",
    sourceNamespaceInode: 4026533001,
    destinationAddress: "172.30.0.10",
    destinationPort: 8334,
    faultId: "a".repeat(32),
  }, new Date("2026-08-23T08:00:02Z"));
  assert.equal(fault.boundary, "celld_store");
  assert.equal(fault.nft_family, "inet");
  assert.match(fault.nft_table, /^as_celld_[0-9a-f]{16}$/);
  assert.equal(fault.nft_chain, `p_${"a".repeat(16)}`);
  assert.equal(fault.nft_comment, `agentic-sandbox:celld-network:titan-765:${"a".repeat(32)}`);
  assert.deepEqual(validateNetworkAuthInventory(inventory, {
    runId: "titan-765",
    runRoot: "/dev/shm/celld-qualification/titan-765",
    hostSha256: inventory.host_sha256,
  }), []);
  assert.throws(() => planDirectionalPartition(inventory, {
    direction: "node_to_peer",
    sourceContainer: "celld-foreign-node-1",
    sourceNamespaceInode: 4026533999,
    destinationAddress: "172.30.0.11",
    destinationPort: 8081,
  }), /not exact-run inventory bound/);
  const inbound = planDirectionalPartition(inventory, {
    direction: "management_to_celld", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 4026533001,
    destinationAddress: "172.30.0.20", destinationPort: 8443, faultId: "1".repeat(32),
  });
  const inboundCommands = directionalPartitionCommands(inventory, inbound);
  assert.equal(inboundCommands.apply[1].includes("input"), true);
  assert.equal(inboundCommands.apply[1].includes("output"), false);
  const inboundDrop = inboundCommands.apply[2];
  assert.equal(inboundDrop[inboundDrop.indexOf("comment") + 1], `"${inbound.nft_comment}"`);
});

test("network inventory validation rejects substituted rule and namespace identities", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  planDirectionalPartition(inventory, {
    direction: "celld_to_management",
    sourceContainer: "celld-fleet-node-1",
    sourceNamespaceInode: 44,
    destinationAddress: "172.30.0.1",
    destinationPort: 18443,
    faultId: "b".repeat(32),
  });
  inventory.faults[0].nft_comment = `agentic-sandbox:celld-network:foreign:${"b".repeat(32)}`;
  assert.match(validateNetworkAuthInventory(inventory).join("; "), /fault is invalid/);
});

test("listener guard allows loopback and exact peers while dropping every bypass", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan", now: new Date("2026-08-23T08:00:00Z") });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 31, inode: 401, runLabel: "titan-765" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-2", pid: 32, inode: 402, runLabel: "titan-765" });
  const guard = planListenerGuard(inventory, {
    sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 401, sameFleetAddresses: ["172.30.0.21", "172.30.0.20"],
  }, new Date("2026-08-23T08:00:01Z"));
  assert.deepEqual(guard.same_fleet_addresses, ["172.30.0.20", "172.30.0.21"]);
  const commands = listenerGuardCommands(inventory, guard);
  const loopbackAccept = commands.apply[2];
  assert.equal(loopbackAccept[loopbackAccept.indexOf("iifname") + 1], "\"lo\"");
  assert.equal(loopbackAccept[loopbackAccept.indexOf("comment") + 1], `"${guard.nft_comment}"`);
  assert.equal(commands.apply.some((args) => args.includes("iifname") && args.includes("\"lo\"") && args.includes("accept")), true);
  assert.equal(commands.apply.filter((args) => args.includes("saddr") && args.includes("accept")).length, 2);
  for (const args of commands.apply.filter((args) => args.includes("comment"))) {
    assert.match(args[args.indexOf("comment") + 1], /^"agentic-sandbox:celld-listener:titan-765:[0-9a-f]{32}"$/);
  }
  assert.equal(commands.apply.at(-1).includes("drop"), true);
  const events = [];
  applyListenerGuard(inventory, guard, {
    persist: () => events.push(`persist:${guard.status}`), dockerRunner: () => "31|titan-765|celld-qualification", namespaceInode: () => 401,
    executor: (_program, args) => ({ status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }), now: new Date("2026-08-23T08:00:02Z"),
  });
  assert.deepEqual(events.slice(0, 2), ["persist:planned", "persist:applied"]);
  removeListenerGuard(inventory, guard, {
    persist: () => {}, dockerRunner: () => "31|titan-765|celld-qualification", namespaceInode: () => 401,
    executor: (_program, args) => ({ status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }), now: new Date("2026-08-23T08:00:03Z"),
  });
  assert.equal(guard.status, "removed");
  assert.equal(inventory.state, "clean");
  guard.same_fleet_addresses[0] = "172.30.0.99";
  assert.match(validateNetworkAuthInventory(inventory).join(";"), /listener guard is invalid/);
});

test("listener guard apply failures expose exact safe command evidence", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan", now: new Date("2026-08-23T08:00:00Z") });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 31, inode: 401, runLabel: "titan-765" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-2", pid: 32, inode: 402, runLabel: "titan-765" });
  const guard = planListenerGuard(inventory, {
    sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 401, sameFleetAddresses: ["172.30.0.21", "172.30.0.20"],
  }, new Date("2026-08-23T08:00:01Z"));
  const stderr = "secret listener guard stderr";
  let failure;
  assert.throws(
    () => applyListenerGuard(inventory, guard, {
      persist: () => {},
      dockerRunner: () => "31|titan-765|celld-qualification",
      namespaceInode: () => 401,
      executor: (_program, args) => {
        if (args.includes("list") && args.includes("table")) return { status: 1, stdout: "", stderr: "not found" };
        if (args.includes("add") && args.includes("table")) return { status: 0, stdout: "", stderr: "" };
        if (args.includes("add") && args.includes("chain")) return { status: 2, stdout: "", stderr };
        return { status: 0, stdout: "", stderr: "" };
      },
      now: new Date("2026-08-23T08:00:02Z"),
    }),
    (error) => {
      failure = error;
      return true;
    },
  );
  assert.equal(failure.operation, "network-auth.apply-listener-guard.add-chain");
  assert.equal(failure.errorCode, "CELLD_LISTENER_GUARD_APPLY_FAILED");
  assert.equal(failure.exitStatus, 2);
  assert.equal(failure.stderrSha256, createHash("sha256").update(stderr).digest("hex"));
  const document = networkAuthDriverErrorDocument(failure);
  assert.equal(document.operation, "network-auth.apply-listener-guard.add-chain");
  assert.equal(document.error_code, "CELLD_LISTENER_GUARD_APPLY_FAILED");
  assert.equal(document.exit_status, 2);
  assert.equal(document.stderr_sha256, createHash("sha256").update(stderr).digest("hex"));
  assert.equal(JSON.stringify(document).includes(stderr), false);
});

test("directional matrices isolate only the selected route and fully heal", () => {
  const timestamp = "2026-08-23T08:00:00Z";
  const routes = ["management_to_celld", "celld_to_management", "celld_to_store", "node_to_peer"];
  const matrix = (blocked = null) => routes.map((route) => ({ route, reachable: route !== blocked, observed_at: timestamp }));
  assert.equal(validateDirectionalRouteMatrices("celld_to_store", matrix(), matrix("celld_to_store"), matrix()), true);
  assert.throws(() => validateDirectionalRouteMatrices("celld_to_store", matrix(), matrix("node_to_peer"), matrix()), /changed the wrong route/);
  assert.throws(() => validateDirectionalRouteMatrices("celld_to_store", matrix(), matrix("celld_to_store"), matrix("celld_to_store")), /did not heal/);
});

test("mTLS proxy plans bind an exact node, private listener, and node-loopback plaintext target", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  }, new Date("2026-08-23T08:00:01Z"));
  assert.equal(proxy.name, "celld-fleet-node-1-mtls-proxy");
  assert.equal(proxy.listen_port, 8443);
  assert.equal(proxy.target_address, "127.0.0.1");
  assert.equal(proxy.target_port, 8081);
  assert.equal(proxy.management_client_cert_file_ref, "/dev/shm/celld-qualification/titan-765/network-tls/management-client.crt");
  assert.deepEqual(validateNetworkAuthInventory(inventory), []);
  const args = mtlsProxyCreateArgs(inventory, proxy, { binaryPath: "/repo/target/agentic-celld-mtls-proxy", uid: 1000, gid: 1000 });
  assert.deepEqual(args.slice(0, 3), ["create", "--name", proxy.name]);
  assert.equal(args.includes(`container:${proxy.node_container}`), true);
  assert.equal(args.includes("0.0.0.0:8443"), true);
  assert.equal(args.includes("127.0.0.1:8081"), true);
  assert.equal(args.includes("--client-cert"), true);
  assert.throws(() => planMtlsProxy(inventory, {
    nodeContainer: "celld-foreign-node", listenAddress: "172.30.0.21", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  }), /not exact-run inventory bound/);
  proxy.target_address = "0.0.0.0";
  assert.match(validateNetworkAuthInventory(inventory).join("; "), /proxy is invalid/);
});

test("mTLS proxy controller persists before creation and removes only the exact owned sidecar", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  const nodeContainerId = "a".repeat(64);
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", containerId: nodeContainerId, pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  const document = JSON.stringify([{
    Name: `/${proxy.name}`, State: { Running: true },
    Config: { Image: proxy.image_ref, Labels: { "dev.agentic-sandbox.run": "titan-765", "dev.agentic-sandbox.scope": "celld-qualification" } },
    HostConfig: { NetworkMode: `container:${nodeContainerId}` },
  }]);
  const events = [];
  let inspections = 0;
  startMtlsProxy(inventory, proxy, {
    binaryPath: "/repo/target/agentic-celld-mtls-proxy",
    verifyMaterial: () => {},
    persist: () => { events.push(`persist:${proxy.status}`); },
    executor: (_program, args) => {
      events.push(args.join(" "));
      if (args[0] === "container" && args[1] === "inspect") {
        inspections += 1;
        return inspections === 1 ? { status: 1, stdout: "", stderr: "missing" } : { status: 0, stdout: document, stderr: "" };
      }
      return { status: 0, stdout: "", stderr: "" };
    },
    uid: 1000, gid: 1000, now: () => new Date("2026-08-23T08:00:07Z"),
  });
  assert.equal(events[0], "persist:planned");
  assert.equal(events.findIndex((event) => event.startsWith("create --name")) > 0, true);
  assert.equal(proxy.status, "started");

  const cleanupCalls = [];
  let cleanupInspections = 0;
  const removed = cleanupMtlsProxies(inventory, {
    executor: (_program, args) => {
      cleanupCalls.push(args);
      if (args[0] === "container" && args[1] === "inspect") {
        cleanupInspections += 1;
        return cleanupInspections === 1 ? { status: 0, stdout: document, stderr: "" } : { status: 1, stdout: "", stderr: "missing" };
      }
      return { status: 0, stdout: "", stderr: "" };
    },
    persist: () => {}, now: () => new Date("2026-08-23T08:00:08Z"),
  });
  assert.deepEqual(removed, [proxy.name]);
  assert.equal(proxy.status, "removed");
  assert.equal(inventory.state, "clean");
  assert.deepEqual(cleanupCalls.find((args) => args[0] === "rm"), ["rm", "--force", "--volumes", proxy.name]);
});

test("mTLS proxy startup failures retain safe command evidence fields", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  const stderr = "secret docker create stderr";
  let failure;
  assert.throws(
    () => startMtlsProxy(inventory, proxy, {
      binaryPath: "/repo/target/agentic-celld-mtls-proxy",
      verifyMaterial: () => {},
      persist: () => {},
      executor: (_program, args) => {
        if (args[0] === "info") return { status: 0, stdout: "27.0.0", stderr: "" };
        if (args[0] === "container" && args[1] === "inspect") return { status: 1, stdout: "", stderr: "missing" };
        if (args[0] === "create") return { status: 125, stdout: "container-id", stderr };
        return { status: 0, stdout: "", stderr: "" };
      },
    }),
    (error) => {
      failure = error;
      return true;
    },
  );
  assert.equal(failure.operation, "network-auth.start-mtls-proxy.create-container");
  assert.equal(failure.errorCode, "CELLD_MTLS_PROXY_CREATE_FAILED");
  assert.equal(failure.exitStatus, 125);
  assert.equal(failure.stdoutSha256, createHash("sha256").update("container-id").digest("hex"));
  assert.equal(failure.stderrSha256, createHash("sha256").update(stderr).digest("hex"));
  const document = networkAuthDriverErrorDocument(failure);
  assert.equal(document.operation, "network-auth.start-mtls-proxy.create-container");
  assert.equal(document.error_code, "CELLD_MTLS_PROXY_CREATE_FAILED");
  assert.equal(document.exit_status, 125);
  assert.equal(JSON.stringify(document).includes(stderr), false);
});

test("mTLS proxy cleanup refuses a foreign same-name container", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  let removed = false;
  assert.throws(() => cleanupMtlsProxies(inventory, {
    executor: (_program, args) => {
      if (args[0] === "info") return { status: 0, stdout: "27", stderr: "" };
      if (args[0] === "container") return { status: 0, stdout: JSON.stringify([{ Name: `/${proxy.name}`, Config: { Image: proxy.image_ref, Labels: { "dev.agentic-sandbox.run": "foreign" } }, HostConfig: { NetworkMode: "container:celld-fleet-node-1" } }]), stderr: "" };
      removed = true;
      return { status: 0, stdout: "", stderr: "" };
    },
    persist: () => {},
  }), /cleanup left residue/);
  assert.equal(removed, false);
  assert.equal(proxy.status, "planned");
});

test("mTLS proxy cleanup closes a persisted plan whose container was never created", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  cleanupMtlsProxies(inventory, {
    executor: (_program, args) => ({ status: args[0] === "info" ? 0 : 1, stdout: "", stderr: "missing" }),
    persist: (document) => assert.deepEqual(validateNetworkAuthInventory(document), []),
    now: () => new Date("2026-08-23T08:00:09Z"),
  });
  assert.equal(proxy.status, "removed");
  assert.equal(proxy.created_at, undefined);
  assert.equal(inventory.state, "clean");
});

test("mTLS certificate preparation persists first and pins client/server purposes and node IP SANs", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  for (const [index, address] of ["172.30.0.20", "172.30.0.21"].entries()) {
    const node = `celld-fleet-node-${index + 1}`;
    registerNetworkNamespace(inventory, { container: node, pid: index + 3, inode: index + 44, runLabel: "titan-765" });
    planMtlsProxy(inventory, { nodeContainer: node, listenAddress: address, binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}` });
  }
  const files = new Map();
  const events = [];
  const result = prepareMtlsProxyCertificates(inventory, {
    persist: () => events.push("persist"), rootAvailable: () => true,
    verifySources: () => events.push("verify-sources"), createDirectory: (path) => events.push(`mkdir:${path}`),
    writeProtected: (path, value) => { files.set(path, Buffer.from(value)); events.push(`write:${path}`); },
    readProtected: (path) => files.get(path), protect: (path) => events.push(`protect:${path}`),
    runner: (program, args) => {
      events.push([program, ...args]);
      const output = args[args.indexOf("-out") + 1];
      if (output) files.set(output, Buffer.from(args[0] === "genpkey" ? "PRIVATE-KEY\n" : args[0] === "req" ? "CSR\n" : "CERTIFICATE\n"));
      return "";
    },
  });
  assert.equal(events[0], "verify-sources");
  assert.equal(events[1], "persist");
  assert.equal(result.servers.length, 2);
  assert.deepEqual(Object.keys(result.negative_client_identities), ["wrong_cn", "cross_fleet_certificate", "expired_certificate"]);
  assert.equal(files.get(result.management_client_identity_file_ref).toString(), "CERTIFICATE\nPRIVATE-KEY\n");
  assert.equal(files.get(result.negative_client_identities.cross_fleet_certificate.identity_file_ref).toString(), "CERTIFICATE\nPRIVATE-KEY\n");
  assert.equal([...files.values()].some((value) => value.toString().includes("subjectAltName=IP:172.30.0.20")), true);
  const commands = events.filter(Array.isArray).map((args) => args.join(" "));
  assert.equal(commands.some((command) => command.includes("verify -purpose sslclient")), true);
  assert.equal(commands.filter((command) => command.includes("verify -purpose sslserver")).length, 2);
  assert.equal(commands.filter((command) => command.includes("-checkend 3600")).length, 5);
  assert.equal(commands.some((command) => command.includes("/CN=agentic-celld-management")), true);
  assert.equal(commands.some((command) => command.includes("/CN=agentic-celld-wrong-cn")), true);
  assert.equal(commands.some((command) => command.includes("/CN=agentic-celld-cross-fleet")), true);
  assert.equal(commands.some((command) => command.includes("/CN=agentic-celld-expired") && command.includes("req -new")), true);
  assert.equal(commands.some((command) => command.includes("expired_certificate.csr") && command.includes("-days 0")), true);
  assert.deepEqual(mtlsNegativeIdentityFiles(inventory), result.negative_client_identities);
});

test("mTLS proxy readiness requires every exact started sidecar", async () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  proxy.status = "started";
  proxy.created_at = "2026-08-23T08:00:07Z";
  proxy.started_at = "2026-08-23T08:00:08Z";
  proxy.updated_at = proxy.started_at;
  let probes = 0;
  const result = await waitMtlsProxies(inventory, {
    attempts: 3, intervalMs: 0, delay: async () => {}, probe: async () => { probes += 1; return probes === 2; },
  });
  assert.deepEqual(result, { status: "READY", proxies: 1 });
  assert.equal(probes, 2);
  await assert.rejects(() => waitMtlsProxies(inventory, { attempts: 2, intervalMs: 0, delay: async () => {}, probe: async () => false }), /readiness failed/);
});

test("authentication route loads the exact protected mTLS proxy identity", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  proxy.status = "started";
  proxy.created_at = "2026-08-23T08:00:07Z";
  proxy.started_at = "2026-08-23T08:00:08Z";
  proxy.updated_at = proxy.started_at;
  const reads = [];
  const route = privateCelldRoute(inventory, {
    inspect: () => ({ isFile: () => true, isSymbolicLink: () => false, mode: 0o100600 }),
    readProtected: (path) => { reads.push(path); return path.endsWith("ca.crt") ? "private-ca" : "client-identity"; },
  });
  assert.equal(route.endpoint, "https://172.30.0.20:8443");
  assert.equal(route.tls.ca.toString(), "private-ca");
  assert.equal(route.tls.identity.toString(), "client-identity");
  assert.deepEqual(reads, [proxy.ca_file_ref, proxy.management_client_identity_file_ref]);
});

test("mTLS negative identities must fail before any Celld HTTP response", async () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  proxy.status = "started";
  proxy.created_at = "2026-08-23T08:00:07Z";
  proxy.started_at = "2026-08-23T08:00:08Z";
  proxy.updated_at = proxy.started_at;
  const seen = [];
  const routeProvider = () => ({ endpoint: "https://172.30.0.20:8443", tls: { ca: Buffer.from("private-ca"), identity: Buffer.from("management") } });
  const attempts = await probeMtlsTransportNegatives(inventory, {
    now: () => new Date("2026-08-23T08:01:00Z"),
    readIdentity: (path) => Buffer.from(path),
    routeProvider,
    requester: async (_endpoint, _path, options) => { seen.push(options.tls); throw new Error("transport denied"); },
  });
  assert.deepEqual(attempts.map((attempt) => attempt.class), ["wrong_san", "wrong_cn", "public_root", "expired_certificate", "cross_fleet_certificate"]);
  assert.equal(attempts.every((attempt) => attempt.outcome === "denied" && attempt.status === null && attempt.code === "transport.denied"), true);
  assert.equal(seen[0].servername, "wrong.invalid");
  assert.equal(seen[2].ca, null);
  await assert.rejects(() => probeMtlsTransportNegatives(inventory, {
    readIdentity: (path) => Buffer.from(path), routeProvider, requester: async () => ({ status: 401 }),
  }), /reached Celld HTTP/);
});

test("plaintext proxy bypass must be unreachable from the management host", async () => {
  const attempt = await probeProxyBypass("172.30.0.20", {
    now: () => new Date("2026-08-23T08:01:00Z"), probe: async (address, port) => { assert.equal(address, "172.30.0.20"); assert.equal(port, 8081); return false; },
  });
  assert.equal(attempt.class, "proxy_bypass");
  assert.equal(attempt.outcome, "denied");
  await assert.rejects(() => probeProxyBypass("172.30.0.20", { probe: async () => true }), /bypass is reachable/);
});

test("environment proxy variables cannot intercept the explicit private route", async () => {
  const environment = { HTTPS_PROXY: "http://old.invalid:1" };
  let closed = false;
  const route = { endpoint: "https://172.30.0.20:8443", tls: { ca: Buffer.from("private-ca"), identity: Buffer.from("management") } };
  const attempt = await probeEnvironmentProxy(route, { keyId: "run-key", key: "k".repeat(43) }, {
    environment,
    now: () => new Date("2026-08-23T08:01:00Z"),
    openTrap: async () => ({ url: "http://127.0.0.1:41234", connections: () => 0, close: async () => { closed = true; } }),
    requester: async (_endpoint, _path, options) => {
      assert.equal(environment.HTTPS_PROXY, "http://127.0.0.1:41234");
      assert.equal(environment.NO_PROXY, "");
      assert.equal(options.tls, route.tls);
      return { status: 404, code: "cell.missing" };
    },
  });
  assert.equal(attempt.class, "environment_proxy");
  assert.equal(attempt.outcome, "denied");
  assert.equal(closed, true);
  assert.deepEqual(environment, { HTTPS_PROXY: "http://old.invalid:1" });
});

test("partition controller persists before mutation and heals only its exact nft table", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan", now: new Date("2026-08-23T08:00:00Z") });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 4026533001, runLabel: "titan-765" }, new Date("2026-08-23T08:00:01Z"));
  const fault = planDirectionalPartition(inventory, {
    direction: "node_to_peer", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 4026533001,
    destinationAddress: "172.30.0.11", destinationPort: 8081, faultId: "c".repeat(32),
  }, new Date("2026-08-23T08:00:02Z"));
  const calls = [];
  const persist = () => { calls.push(["persist", fault.status]); };
  const dockerRunner = () => "3210|titan-765|celld-qualification";
  const executor = (program, args) => {
    calls.push([program, ...args]);
    return { status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" };
  };
  applyDirectionalPartition(inventory, fault, { executor, persist, dockerRunner, namespaceInode: () => 4026533001, now: new Date("2026-08-23T08:00:03Z") });
  assert.deepEqual(calls[0], ["persist", "planned"]);
  assert.equal(fault.status, "applied");
  const commands = directionalPartitionCommands(inventory, fault);
  assert.deepEqual(commands.heal.slice(-3), ["table", "inet", `as_celld_${"c".repeat(16)}`]);
  assert.equal(commands.apply[2].includes("drop"), true);
  assert.equal(commands.apply[2].at(-1), `"agentic-sandbox:celld-network:titan-765:${"c".repeat(32)}"`);

  calls.length = 0;
  healDirectionalPartition(inventory, fault, {
    executor: (program, args) => { calls.push([program, ...args]); return { status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }; },
    persist, dockerRunner, namespaceInode: () => 4026533001, now: new Date("2026-08-23T08:00:04Z"),
  });
  assert.equal(fault.status, "healed");
  assert.equal(inventory.state, "clean");
  assert.equal(calls.length, 3);
  assert.deepEqual(calls[0].slice(-3), ["table", "inet", `as_celld_${"c".repeat(16)}`]);
  assert.equal(calls[1].includes("list"), true);
  assert.deepEqual(calls[2], ["persist", "healed"]);
});

test("partially applied partition remains planned and exact cleanup is recoverable", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 99, runLabel: "titan-765" });
  const fault = planDirectionalPartition(inventory, {
    direction: "celld_to_management", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 99,
    destinationAddress: "172.30.0.1", destinationPort: 8122, faultId: "d".repeat(32),
  });
  let calls = 0;
  let failure;
  assert.throws(() => applyDirectionalPartition(inventory, fault, {
    executor: (_program, args) => {
      if (args.includes("list")) return { status: 1, stdout: "", stderr: "missing" };
      calls += 1;
      return { status: calls === 2 ? 1 : 0, stdout: "", stderr: "injected" };
    },
    persist: () => {}, dockerRunner: () => "3210|titan-765|celld-qualification", namespaceInode: () => 99,
  }), (error) => {
    failure = error;
    return /partition apply command failed/.test(error.message);
  });
  assert.equal(failure.operation, "network-auth.apply-directional-partition.add-chain");
  assert.equal(failure.errorCode, "CELLD_DIRECTIONAL_PARTITION_APPLY_FAILED");
  assert.equal(failure.exitStatus, 1);
  assert.equal(failure.stderrSha256, createHash("sha256").update("injected").digest("hex"));
  assert.equal(fault.status, "planned");
  healDirectionalPartition(inventory, fault, {
    executor: (_program, args) => ({ status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }), persist: () => {},
    dockerRunner: () => "3210|titan-765|celld-qualification", namespaceInode: () => 99,
  });
  assert.equal(fault.status, "healed");
});

test("persisted recovery heals every exact table in reverse plan order", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 99, runLabel: "titan-765" });
  for (const [destinationAddress, faultId] of [["172.30.0.10", "e".repeat(32)], ["172.30.0.11", "f".repeat(32)]]) {
    planDirectionalPartition(inventory, {
      direction: "node_to_peer", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 99,
      destinationAddress, destinationPort: 8081, faultId,
    });
  }
  const deleted = [];
  const persisted = [];
  const result = recoverNetworkAuthInventory(inventory, {
    executor: (_program, args) => { if (args.includes("delete")) deleted.push(args.at(-1)); return { status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }; },
    persist: (document) => { persisted.push(document.state); },
    dockerRunner: () => "3210|titan-765|celld-qualification",
    namespaceInode: () => 99,
    now: () => new Date("2026-08-23T08:00:05Z"),
  });
  assert.deepEqual(deleted, [`as_celld_${"f".repeat(16)}`, `as_celld_${"e".repeat(16)}`]);
  assert.equal(inventory.state, "clean");
  assert.equal(persisted.at(-1), "clean");
  assert.equal(result.status, "PASS");
  assert.deepEqual(result.healed_fault_ids, ["e".repeat(32), "f".repeat(32)]);
});

test("persisted recovery reports residue without touching an unowned nftables table", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 99, runLabel: "titan-765" });
  const fault = planDirectionalPartition(inventory, {
    direction: "celld_to_store", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 99,
    destinationAddress: "172.30.0.10", destinationPort: 8334, faultId: "9".repeat(32),
  });
  const calls = [];
  assert.throws(() => recoverNetworkAuthInventory(inventory, {
    executor: (_program, args) => {
      calls.push(args);
      return { status: args.includes("delete") ? 1 : 0, stdout: args.at(-1) === "tables" ? `table inet ${fault.nft_table}` : "", stderr: "injected" };
    },
    persist: () => {}, dockerRunner: () => "3210|titan-765|celld-qualification",
    namespaceInode: () => 99, now: () => new Date("2026-08-23T08:00:06Z"),
  }), /cleanup left residue/);
  assert.equal(inventory.state, "cleanup_residue");
  assert.equal(fault.status, "planned");
  assert.equal(calls[0].includes(fault.nft_table), true);
  assert.equal(calls[1].at(-1), "tables");
  assert.equal(calls.some((args) => args.includes("flush")), false);
});

test("crash cleanup reads only the two fixed issue-lane inventory paths", () => {
  const repoRoot = process.cwd();
  const config = {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1",
    run_id: "titan-765",
    working_root: "/dev/shm/agentic-celld-orchestration/titan-765",
    inventory_path: "/dev/shm/agentic-celld-orchestration/titan-765/orchestration-inventory.json",
    management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`,
    agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`,
    callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`,
    qemu_cleanup_helper_path: "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper",
    qemu_cleanup_helper_sha256: "e".repeat(64),
    docker_image_ref: `sha256:${"a".repeat(64)}`,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: "/var/tmp/agentic-celld-qualification-765/mount",
    libvirt_uri: "qemu:///system",
    management_grpc_port: 38120,
  };
  const locations = networkAuthInventoryLocations(config);
  assert.deepEqual(locations.map((entry) => entry.scenario_id), ["UAT-CELLD-010", "UAT-CELLD-012"]);
  assert.equal(locations.every((entry) => entry.inventory_path === `${entry.run_root}/network-auth-inventory.json`), true);
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: locations[0].run_root, host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 99, runLabel: "titan-765" });
  planDirectionalPartition(inventory, {
    direction: "node_to_peer", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 99,
    destinationAddress: "172.30.0.11", destinationPort: 8081, faultId: "8".repeat(32),
  });
  const inspected = [];
  const result = cleanupNetworkAuthInventories(config, {
    exists: (path) => { inspected.push(path); return path === locations[0].inventory_path; },
    readInventory: (path) => { assert.equal(path, locations[0].inventory_path); return inventory; },
    host: "titan", executor: (_program, args) => ({ status: args.at(-1) === "tables" ? 0 : args.includes("list") ? 1 : 0, stdout: "", stderr: "" }), persist: () => {},
    dockerRunner: () => "3210|titan-765|celld-qualification", namespaceInode: () => 99,
  });
  assert.deepEqual(inspected, locations.map((entry) => entry.inventory_path));
  assert.equal(result.inventories.length, 1);
  assert.equal(result.inventories[0].scenario_id, "UAT-CELLD-010");
  assert.equal(inventory.state, "clean");
});

test("disabled network/auth qualification returns pre-mutation NOT_RUN evidence", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-network-auth-test-"));
  try {
    const profilePath = join(directory, "profile.json");
    const profile = {
      schema_version: "agentic-sandbox.celld-live-profile/v1",
      profile_id: "test-profile",
      run_id: "test-run",
      expected_sandbox_git: "1".repeat(40),
      environment: { kind: "disposable-local", single_host: true, host_sha256: "2".repeat(64) },
      authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
      drivers: { "celld-live-network-auth": { enabled: false, config_path: "/tmp/orchestration.json" } },
    };
    writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
    chmodSync(profilePath, 0o600);
    const observation = await executeNetworkAuthDriver({ scenarioId: "UAT-CELLD-010", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_NETWORK_AUTH_DRIVER_DISABLED");
    assert.equal(observation.cleanup.status, "not_required");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("probe cleanup removes only exact-run labeled resources in dependency order", () => {
  const calls = [];
  const runId = "titan-123";
  const suffix = "782e8aeeba2cf0d1";
  const network = `celld-probe-${suffix}`;
  const container = `${network}-client`;
  const labels = { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification", "dev.agentic-sandbox.probe-role": "isolation" };
  const runner = (program, args) => {
    calls.push([program, ...args]);
    if (args[0] === "info") return "27.0.0";
    if (args[0] === "container" && args[1] === "inspect") return JSON.stringify([{ Config: { Labels: labels } }]);
    if (args[0] === "network" && args[1] === "inspect") return JSON.stringify([{ Labels: labels }]);
    return "";
  };
  const result = cleanupProbeResources(runId, { runner, roles: ["isolation"] });
  assert.deepEqual(result, { status: "PASS", run_id: runId, removed: [container, network], residue: [] });
  assert.deepEqual(calls.at(-2), ["docker", "network", "inspect", network]);
  assert.deepEqual(calls.at(-1), ["docker", "network", "rm", network]);
});

test("probe cleanup refuses a foreign Docker label before deletion", () => {
  let mutated = false;
  const runner = (_program, args) => {
    if (args[0] === "info") return "27.0.0";
    if (args[1] === "inspect") return JSON.stringify([{ Config: { Labels: { "dev.agentic-sandbox.run": "foreign" } } }]);
    mutated = true;
    return "";
  };
  assert.throws(() => cleanupProbeResources("titan-123", { runner, roles: ["isolation"] }), /refusing unowned probe resource/);
  assert.equal(mutated, false);
});

test("network/auth source fixes the qualified sample sizes and pins the probe image", () => {
  const source = readFileSync(new URL("../../../scripts/celld-live-network-auth.mjs", import.meta.url), "utf8");
  const proxy = readFileSync(new URL("../../../tools/celld-callback-relay/src/bin/agentic-celld-mtls-proxy.rs", import.meta.url), "utf8");
  assert.match(source, /const attemptsPerClass = 1_000/);
  assert.match(source, /const PROBE_CONCURRENCY = 32/);
  assert.match(source, /mapBounded\(Array\.from\(\{ length: 1_000 \}/);
  assert.match(source, /String\(PROBE_CONCURRENCY\)/);
  assert.match(source, /readManagementProviderCounter/);
  assert.match(source, /waitManagementProviderLedger/);
  assert.match(source, /waitManagementCelldStatus/);
  assert.match(source, /network-auth\.wait-management-celld-status/);
  assert.match(source, /CELLD_MANAGEMENT_CELLD_CONFIG_INVALID/);
  assert.match(source, /\/api\/v2\/celld\/status/);
  assert.match(source, /network-auth\.wait-provider-ledger/);
  assert.match(source, /const route = privateCelldRoute\(runtime\.networkInventory\)/);
  assert.match(source, /role: kind === "public_route" \? "public" : "cross-fleet"/);
  assert.doesNotMatch(source, /provider_effects:\s*0/);
  assert.match(source, /docker\.io\/library\/node:20@sha256:[0-9a-f]{64}/);
  assert.doesNotMatch(source, /docker\.io\/library\/node:(?:latest|20)(?:["'])/);
  assert.match(source, /import \{ openStorageGatewayAccess \} from "\.\/celld-storage-gateway-access\.mjs"/);
  assert.match(source, /openStorageGatewayAccess\(runtime\.storage, \{ services: \["s3gateway1"\] \}\)/);
  assert.doesNotMatch(source, /compose[^\n]*\["port",\s*"s3gateway/);
  assert.match(proxy, /--client-cert/);
  assert.match(proxy, /peer_certificates/);
  assert.match(proxy, /--target must be a fixed node-loopback socket/);
  assert.match(proxy, /exact run identity/);
});
