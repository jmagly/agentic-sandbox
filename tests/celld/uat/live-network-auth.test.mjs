import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  cleanupProbeResources,
  executeNetworkAuthDriver,
} from "../../../scripts/celld-live-network-auth.mjs";

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
  assert.match(source, /const attemptsPerClass = 1_000/);
  assert.match(source, /attempt < 1_000/);
  assert.match(source, /docker\.io\/library\/node:20@sha256:[0-9a-f]{64}/);
  assert.doesNotMatch(source, /docker\.io\/library\/node:(?:latest|20)(?:["'])/);
});
