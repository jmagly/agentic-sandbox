import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { OBSERVABILITY_BOUNDARIES, OBSERVABILITY_SURFACES } from "../../../scripts/celld-observability-controller.mjs";
import { executeObservabilityDriver, OBSERVABILITY_PREREQUISITES, OBSERVABILITY_READY_PREREQUISITES } from "../../../scripts/celld-live-observability.mjs";
import { SAFE_LIVE_EVALUATORS } from "../../../scripts/celld-live-evaluators.mjs";

function orchestrationConfig(repoRoot) {
  return { schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run", working_root: "/dev/shm/agentic-celld-orchestration/test-run", inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json", management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`, agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`, callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`, docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images", vm_storage_dir: "/build/agentic-sandbox/vms", agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount", libvirt_uri: "qemu:///system", management_grpc_port: 38120 };
}

function profile(configPath, hostHash, enabled = true, authorized = false) {
  return { schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run", expected_sandbox_git: "1".repeat(40), environment: { kind: "disposable-local", single_host: true, host_sha256: hostHash }, authorization: { destructive_faults: authorized, inventory_path: "/tmp/test-inventory.json", ...(authorized ? { exact_run_owner: "test-run" } : {}) }, drivers: { "celld-live-observability": { enabled, config_path: configPath } } };
}

function fixture(enabled = true, authorized = false) {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-observability-test-"));
  const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
  const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
  writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
  writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex"), enabled, authorized))}\n`, { mode: 0o600 });
  chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
  return { directory, host, profilePath };
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function campaignAdapter() {
  const hash = (value) => createHash("sha256").update(value).digest("hex");
  const baseline = "a".repeat(64);
  const active = new Set();
  const identity = (boundary) => {
    const index = OBSERVABILITY_BOUNDARIES.indexOf(boundary) + 1;
    return { fleet_id: "fleet-test", instance_id: `instance-${index}`, generation: index, operation_id: `operation-${index}`, trace_id: index.toString(16).padStart(32, "0"), celld_version: "v0.3.0", adapter_version: "2026.8.3", node_id: `node-${index % 3}` };
  };
  const timing = (boundary) => {
    const injected = 1_000_000 + OBSERVABILITY_BOUNDARIES.indexOf(boundary) * 1_000_000;
    const evaluation = 60_000, retry = 60_000;
    const delay = boundary === "divergence" ? 300_000 : boundary === "unknown_effect" ? retry * 2 : boundary === "stale_generation" ? evaluation : 1_000;
    return { injected, detected: injected + delay, healed: injected + delay + 1_000, resolved: injected + delay + 2_000, evaluation, retry };
  };
  return {
    captureBaseline: async () => ({ baseline_sha256: baseline }),
    persistIntent: async (intent) => ({ intent_sha256: hash(canonicalJson(intent)), persisted: true }),
    injectFault: async (boundary) => { active.add(boundary); },
    observeFault: async (boundary) => ({ boundary, injection_applied: true, injection_verified: true, injected_at_ms: timing(boundary).injected, identities: identity(boundary) }),
    collectSurface: async (boundary, surface) => ({ boundary, surface, classification: boundary, identities: identity(boundary) }),
    collectRepairPlan: async (boundary, surface) => ({ boundary, surface, representation: "plan", effect_claimed: false }),
    observeAlertDetection: async (boundary) => ({ boundary, detected_at_ms: timing(boundary).detected, evaluation_interval_ms: timing(boundary).evaluation, retry_interval_ms: timing(boundary).retry }),
    healFault: async (boundary) => { active.delete(boundary); },
    observeHeal: async (boundary) => ({ boundary, healed: true, heal_verified: true, healed_at_ms: timing(boundary).healed }),
    observeAlertResolution: async (boundary) => ({ boundary, resolved_at_ms: timing(boundary).resolved }),
    scanRedaction: async () => ({ surfaces_scanned: [...OBSERVABILITY_SURFACES], artifacts_scanned: 70, secret_findings: 0 }),
    verifyBaseline: async () => ({ baseline_sha256: baseline, restored: active.size === 0 }),
  };
}

test("disabled observability driver returns pre-mutation NOT_RUN", async () => {
  const value = fixture(false);
  try {
    const observation = await executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_OBSERVABILITY_DRIVER_DISABLED");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("UAT-014 reports every unavailable dependency before fault injection", async () => {
  const value = fixture();
  try {
    const observation = await executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host });
    assert.equal(observation.mutation_started, false);
    assert.deepEqual(observation.prerequisites, OBSERVABILITY_PREREQUISITES);
    assert.deepEqual(observation.assertions, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("observability prerequisites preserve separate authorization, rollout, and telemetry blockers", () => {
  assert.deepEqual(OBSERVABILITY_PREREQUISITES.map((item) => item.id), ["CELLD_CREDENTIAL_PROVENANCE_CAMPAIGN", "CELLD_ROLLOUT_QUALIFICATION", "CELLD_LIVE_TELEMETRY_STACK"]);
  assert.ok(OBSERVABILITY_PREREQUISITES.every((item) => item.status === "unavailable"));
});

test("ready prerequisites still require exact-run destructive authorization", async () => {
  const value = fixture();
  try {
    const observation = await executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => OBSERVABILITY_READY_PREREQUISITES });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("authorization cannot bypass an unavailable live telemetry adapter", async () => {
  const value = fixture(true, true);
  try {
    const observation = await executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => OBSERVABILITY_READY_PREREQUISITES });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_OBSERVABILITY_ADAPTER_UNAVAILABLE");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("injected controller emits artifact-backed measurements accepted by trusted evaluators", async () => {
  const value = fixture(true, true);
  try {
    const observation = await executeObservabilityDriver(
      { scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath, artifactDir: join(value.directory, "artifacts") },
      { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => OBSERVABILITY_READY_PREREQUISITES, observabilityAdapter: campaignAdapter() },
    );
    assert.equal(observation.mutation_started, true);
    assert.equal(observation.assertions.length, 3);
    assert.equal(observation.artifacts.length, 2);
    for (const assertion of observation.assertions) assert.equal(SAFE_LIVE_EVALUATORS[assertion.id](assertion.measurements).passed, true, assertion.id);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("prerequisite overrides must match the exact status contract", async () => {
  const value = fixture();
  try {
    await assert.rejects(
      executeObservabilityDriver({ scenarioId: "UAT-CELLD-014", runId: "test-run", liveProfilePath: value.profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => value.host, prerequisites: () => OBSERVABILITY_READY_PREREQUISITES.slice(1) }),
      /exact prerequisite inventory/,
    );
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});
