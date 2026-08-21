import assert from "node:assert/strict";
import test from "node:test";

import { evaluateTitanPostflight, main } from "../../../scripts/celld-titan-postflight.mjs";

function baseline() {
  const resources = { complete: true, errors: [], docker_containers: ["existing"], docker_networks: ["bridge", "host", "none"], docker_volumes: ["volume"], libvirt_domains: ["domain"], vm_root_entries: ["domain"], qualification_agentshare_entries: [] };
  return {
    evidence_schema: "agentic-sandbox.celld-titan-preflight/v1", status: "PASS", collected_at: "2026-08-21T00:00:00Z",
    host: { short_hostname: "titan" }, git: { commit: "a".repeat(40) },
    storage: { root: { free_bytes: 500 }, build: { free_bytes: 1000 } }, thresholds: { min_root_free_bytes: 100, min_build_free_bytes: 400 }, resource_baseline: resources,
  };
}

function current(preflight = baseline()) {
  return { evidence_schema: "agentic-sandbox.celld-titan-postflight-snapshot/v1", collected_at: "2026-08-21T01:00:00Z", host: { short_hostname: preflight.host.short_hostname }, git: { commit: preflight.git.commit }, storage: { root: { free_bytes: 495 }, build: { free_bytes: 990 } }, resource_baseline: structuredClone(preflight.resource_baseline) };
}

test("Titan postflight passes only when exact resources and capacity return to baseline", () => {
  const preflight = baseline(), result = evaluateTitanPostflight(preflight, current(preflight), 20);
  assert.equal(result.status, "PASS");
  assert.equal(result.reason_code, "titan.postflight_baseline_restored");
  assert.ok(result.checks.every((check) => check.status === "PASS"));
});

test("Titan postflight fails on leaked Docker, libvirt, VM-root, or agentshare resources", () => {
  const preflight = baseline(), snapshot = current(preflight);
  snapshot.resource_baseline.docker_containers.push("leaked-container");
  snapshot.resource_baseline.libvirt_domains.push("leaked-domain");
  snapshot.resource_baseline.vm_root_entries.push("leaked-domain");
  snapshot.resource_baseline.qualification_agentshare_entries.push("agentic-celld-qualification-123");
  const failures = evaluateTitanPostflight(preflight, snapshot, 20).checks.filter((check) => check.status === "FAIL").map((check) => check.id);
  assert.deepEqual(failures, ["resources.docker_containers", "resources.libvirt_domains", "resources.vm_root_entries", "resources.qualification_agentshare_entries"]);
});

test("Titan postflight fails when retained data exceeds the bounded allowance", () => {
  const preflight = baseline(), snapshot = current(preflight);
  snapshot.storage.root.free_bytes = 470;
  snapshot.storage.build.free_bytes = 970;
  const failures = evaluateTitanPostflight(preflight, snapshot, 20).checks.filter((check) => check.status === "FAIL").map((check) => check.id);
  assert.deepEqual(failures, ["storage.root.regression", "storage.build.regression"]);
});

test("Titan postflight emits a typed failure for incomplete resource inventory", () => {
  const preflight = baseline(), snapshot = current(preflight);
  snapshot.resource_baseline.complete = false;
  snapshot.resource_baseline.errors = ["docker_volumes"];
  const failures = evaluateTitanPostflight(preflight, snapshot, 20).checks.filter((check) => check.status === "FAIL").map((check) => check.id);
  assert.deepEqual(failures, ["resources.inventory_complete"]);
});

test("Titan postflight requires passing bound preflight evidence", () => {
  const preflight = baseline(); preflight.status = "FAIL";
  assert.throws(() => evaluateTitanPostflight(preflight, current(preflight), 20), /passing preflight evidence/);
});

test("Titan postflight CLI requires an explicit preflight evidence path", () => {
  assert.throws(() => main([], {}), /--preflight FILE/);
});
