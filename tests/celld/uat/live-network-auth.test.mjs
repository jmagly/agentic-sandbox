import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  applyDirectionalPartition,
  cleanupMtlsProxies,
  cleanupNetworkAuthInventories,
  cleanupProbeResources,
  createNetworkAuthInventory,
  directionalPartitionCommands,
  executeNetworkAuthDriver,
  healDirectionalPartition,
  mapBounded,
  mtlsNegativeIdentityFiles,
  mtlsProxyCreateArgs,
  networkAuthInventoryLocations,
  observeFleetNetworkNamespaces,
  planMtlsProxy,
  planDirectionalPartition,
  privateCelldRoute,
  prepareMtlsProxyCertificates,
  readManagementProviderCounter,
  recoverNetworkAuthInventory,
  registerNetworkNamespace,
  startMtlsProxy,
  validateNetworkAuthInventory,
  waitMtlsProxies,
} from "../../../scripts/celld-live-network-auth.mjs";

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

test("fleet namespace observation joins exact Docker ownership, inode, and private address", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  const fleet = { run_id: "titan-765", network: { name: "celld-private" }, nodes: [{ name: "celld-fleet-node-1" }, { name: "celld-fleet-node-2" }] };
  const observed = observeFleetNetworkNamespaces(inventory, fleet, {
    runner: (_program, args) => {
      const index = args.at(-1).endsWith("1") ? 1 : 2;
      return JSON.stringify([{
        State: { Running: true, Pid: 3200 + index },
        Config: { Labels: { "dev.agentic-sandbox.run": "titan-765", "dev.agentic-sandbox.scope": "celld-qualification" } },
        NetworkSettings: { Networks: { "celld-private": { IPAddress: `172.30.0.${20 + index}` } } },
      }]);
    },
    namespaceInode: (pid) => 4026533000 + pid,
    now: new Date("2026-08-23T08:00:00Z"),
  });
  assert.deepEqual(observed.map((entry) => entry.address), ["172.30.0.21", "172.30.0.22"]);
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
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3, inode: 44, runLabel: "titan-765" });
  const proxy = planMtlsProxy(inventory, {
    nodeContainer: "celld-fleet-node-1", listenAddress: "172.30.0.20", binarySha256: "7".repeat(64), imageRef: `sha256:${"6".repeat(64)}`,
  });
  const document = JSON.stringify([{
    Name: `/${proxy.name}`, State: { Running: true },
    Config: { Image: proxy.image_ref, Labels: { "dev.agentic-sandbox.run": "titan-765", "dev.agentic-sandbox.scope": "celld-qualification" } },
    HostConfig: { NetworkMode: "container:celld-fleet-node-1" },
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
    return { status: args.includes("list") ? 1 : 0, stdout: "", stderr: "" };
  };
  applyDirectionalPartition(inventory, fault, { executor, persist, dockerRunner, namespaceInode: () => 4026533001, now: new Date("2026-08-23T08:00:03Z") });
  assert.deepEqual(calls[0], ["persist", "planned"]);
  assert.equal(fault.status, "applied");
  const commands = directionalPartitionCommands(inventory, fault);
  assert.deepEqual(commands.heal.slice(-3), ["table", "inet", `as_celld_${"c".repeat(16)}`]);
  assert.equal(commands.apply[2].includes("drop"), true);
  assert.equal(commands.apply[2].at(-1), `agentic-sandbox:celld-network:titan-765:${"c".repeat(32)}`);

  calls.length = 0;
  healDirectionalPartition(inventory, fault, {
    executor: (program, args) => { calls.push([program, ...args]); return { status: 0, stdout: "", stderr: "" }; },
    persist, dockerRunner, namespaceInode: () => 4026533001, now: new Date("2026-08-23T08:00:04Z"),
  });
  assert.equal(fault.status, "healed");
  assert.equal(inventory.state, "clean");
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[0].slice(-3), ["table", "inet", `as_celld_${"c".repeat(16)}`]);
  assert.deepEqual(calls[1], ["persist", "healed"]);
});

test("partially applied partition remains planned and exact cleanup is recoverable", () => {
  const inventory = createNetworkAuthInventory({ runId: "titan-765", runRoot: "/dev/shm/celld-qualification/titan-765", host: "titan" });
  registerNetworkNamespace(inventory, { container: "celld-fleet-node-1", pid: 3210, inode: 99, runLabel: "titan-765" });
  const fault = planDirectionalPartition(inventory, {
    direction: "celld_to_management", sourceContainer: "celld-fleet-node-1", sourceNamespaceInode: 99,
    destinationAddress: "172.30.0.1", destinationPort: 8122, faultId: "d".repeat(32),
  });
  let calls = 0;
  assert.throws(() => applyDirectionalPartition(inventory, fault, {
    executor: (_program, args) => {
      if (args.includes("list")) return { status: 1, stdout: "", stderr: "missing" };
      calls += 1;
      return { status: calls === 2 ? 1 : 0, stdout: "", stderr: "injected" };
    },
    persist: () => {}, dockerRunner: () => "3210|titan-765|celld-qualification", namespaceInode: () => 99,
  }), /partition apply failed/);
  assert.equal(fault.status, "planned");
  healDirectionalPartition(inventory, fault, {
    executor: () => ({ status: 0, stdout: "", stderr: "" }), persist: () => {},
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
    executor: (_program, args) => { deleted.push(args.at(-1)); return { status: 0, stdout: "", stderr: "" }; },
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
      return { status: args.includes("delete") ? 1 : 0, stdout: "", stderr: "injected" };
    },
    persist: () => {}, dockerRunner: () => "3210|titan-765|celld-qualification",
    namespaceInode: () => 99, now: () => new Date("2026-08-23T08:00:06Z"),
  }), /cleanup left residue/);
  assert.equal(inventory.state, "cleanup_residue");
  assert.equal(fault.status, "planned");
  assert.equal(calls.every((args) => args.includes(fault.nft_table)), true);
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
    host: "titan", executor: () => ({ status: 0, stdout: "", stderr: "" }), persist: () => {},
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
  const labels = { "dev.agentic-sandbox.run": runId, "dev.agentic-sandbox.scope": "celld-qualification" };
  const runner = (program, args) => {
    calls.push([program, ...args]);
    if (args[0] === "info") return "27.0.0";
    if (args[0] === "container" && args[1] === "inspect") return JSON.stringify([{ Config: { Labels: labels } }]);
    if (args[0] === "network" && args[1] === "inspect") return JSON.stringify([{ Labels: labels }]);
    return "";
  };
  const result = cleanupProbeResources(runId, { runner });
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
  assert.throws(() => cleanupProbeResources("titan-123", { runner }), /refusing unowned probe resource/);
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
  assert.match(source, /const route = privateCelldRoute\(runtime\.networkInventory\)/);
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
