import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { executeRolloutDriver, qualifiedCelldArtifacts, selectDistinctRolloutPair } from "../../../scripts/celld-live-rollout.mjs";
import { SAFE_LIVE_EVALUATORS } from "../../../scripts/celld-live-evaluators.mjs";
import { evaluateLiveObservation } from "../../../scripts/celld-uat-live-protocol.mjs";

function orchestrationConfig(repoRoot) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
    inventory_path: "/dev/shm/agentic-celld-orchestration/test-run/orchestration-inventory.json",
    management_binary_path: `${repoRoot}/management/target/release/agentic-mgmt`, agent_client_binary_path: `${repoRoot}/management/target/release/agent-client`,
    callback_relay_binary_path: `${repoRoot}/tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay`,
    docker_image_ref: `sha256:${"a".repeat(64)}`, base_images_dir: "/build/agentic-sandbox/base-images", vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount", libvirt_uri: "qemu:///system", management_grpc_port: 38120,
  };
}

function profile(configPath, hostHash) {
  return {
    schema_version: "agentic-sandbox.celld-live-profile/v1", profile_id: "test-profile", run_id: "test-run", expected_sandbox_git: "1".repeat(40),
    environment: { kind: "disposable-local", single_host: true, host_sha256: hostHash },
    authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
    drivers: { "celld-live-rollout": { enabled: true, config_path: configPath } },
  };
}

test("qualified rollout pair requires distinct reviewed versions and digests", () => {
  const one = qualifiedCelldArtifacts({ schema_version: "agentic-sandbox.celld-images/v1", platform: "linux/amd64", celld: { version: "0.2.1", commit: "1".repeat(40), manifest_digest: `sha256:${"a".repeat(64)}` } });
  assert.equal(selectDistinctRolloutPair(one), null);
  const pair = [...one, { version: "0.2.2", commit: "2".repeat(40), manifest_digest: `sha256:${"b".repeat(64)}`, compatible_from: ["0.2.1"] }];
  assert.deepEqual(selectDistinctRolloutPair(pair), { previous: pair[0], candidate: pair[1] });
  assert.equal(selectDistinctRolloutPair(pair.map((entry) => ({ ...entry, compatible_from: [] }))), null);
});

test("UAT-011 is typed NOT_RUN before mutation while the reviewed candidate is unqualified", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const observation = await executeRolloutDriver({ scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => host });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_ROLLOUT_CANDIDATE_UNQUALIFIED");
    assert.deepEqual(observation.assertions, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("UAT-011 distinguishes absence of any reviewed candidate", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-absent-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const observation = await executeRolloutDriver(
      { scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath },
      { gitCommit: () => "1".repeat(40), hostname: () => host, reviewedCandidates: () => [] },
    );
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_QUALIFIED_ROLLOUT_PAIR_UNAVAILABLE");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("adding a pair requires destructive authorization before an adapter", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-pair-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const qualifiedImages = () => ({ schema_version: "agentic-sandbox.celld-images/v1", platform: "linux/amd64", celld: [
      { version: "0.2.1", commit: "1".repeat(40), manifest_digest: `sha256:${"a".repeat(64)}` },
      { version: "0.2.2", commit: "2".repeat(40), manifest_digest: `sha256:${"b".repeat(64)}`, compatible_from: ["0.2.1"] },
    ] });
    const observation = await executeRolloutDriver({ scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => host, qualifiedImages });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_DESTRUCTIVE_AUTHORIZATION_REQUIRED");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("an authorized qualified pair remains NOT_RUN without the reviewed live adapter", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-adapter-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    const authorized = profile(configPath, createHash("sha256").update(host).digest("hex"));
    authorized.authorization = { destructive_faults: true, inventory_path: "/tmp/test-inventory.json", exact_run_owner: "test-run" };
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(authorized)}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const qualifiedImages = () => ({ schema_version: "agentic-sandbox.celld-images/v1", platform: "linux/amd64", celld: [
      { version: "0.2.1", commit: "1".repeat(40), manifest_digest: `sha256:${"a".repeat(64)}` },
      { version: "0.2.2", commit: "2".repeat(40), manifest_digest: `sha256:${"b".repeat(64)}`, compatible_from: ["0.2.1"] },
    ] });
    const observation = await executeRolloutDriver({ scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => host, qualifiedImages });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_ROLLOUT_ADAPTER_UNAVAILABLE");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("the injected controller emits artifact-backed measurements accepted by trusted evaluators", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-evidence-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json"), outputDir = join(directory, "results"), artifactDir = join(outputDir, "artifacts");
    const authorized = profile(configPath, createHash("sha256").update(host).digest("hex"));
    authorized.authorization = { destructive_faults: true, inventory_path: "/tmp/test-inventory.json", exact_run_owner: "test-run" };
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(authorized)}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const oldDigest = `sha256:${"a".repeat(64)}`, newDigest = `sha256:${"b".repeat(64)}`;
    const nodes = new Map([
      ["celld-node-1", { name: "celld-node-1", role: "active", running: true, ready: true, member: true, manifest_digest: oldDigest }],
      ["celld-node-2", { name: "celld-node-2", role: "active", running: true, ready: true, member: true, manifest_digest: oldDigest }],
      ["celld-node-3", { name: "celld-node-3", role: "reserve", running: true, ready: true, member: true, manifest_digest: oldDigest }],
    ]);
    let signaled = false;
    const rolloutAdapter = {
      persistIntent: async () => {},
      observeFleet: async () => ({ nodes: [...nodes.values()].map((node) => ({ ...node })) }),
      observeInventory: async () => ({ nodes: [...nodes.values()].map(({ name, role, manifest_digest }) => ({ name, role, manifest_digest })) }),
      drainNode: async (name) => Object.assign(nodes.get(name), { ready: false, member: false }),
      replaceNode: async (name, artifact) => Object.assign(nodes.get(name), { running: true, ready: true, member: true, manifest_digest: artifact.manifest_digest }),
      killNode: async (name) => Object.assign(nodes.get(name), { running: false, ready: false, member: false }),
      restoreNode: async (name, artifact) => Object.assign(nodes.get(name), { running: true, ready: true, member: true, manifest_digest: artifact.manifest_digest }),
      injectRollbackSignal: async () => { signaled = true; },
      observeSafety: async () => ({ acknowledged_intent_ids: ["intent-1"], rebuilt_intent_ids: ["intent-1"], effects: [], current_generations: {}, reconcile_samples_ms: [250], error_rate: signaled ? 0.02 : 0, baseline_coordination_p99_ms: 100, coordination_p99_ms: 100 }),
    };
    const qualifiedImages = () => ({ schema_version: "agentic-sandbox.celld-images/v1", platform: "linux/amd64", celld: [
      { version: "0.2.1", commit: "1".repeat(40), manifest_digest: oldDigest },
      { version: "0.2.2", commit: "2".repeat(40), manifest_digest: newDigest, compatible_from: ["0.2.1"] },
    ] });
    const rolloutPlan = { max_unavailable: 1, reserve: 1, drain_timeout_ms: 120_000, nodes: [...nodes.values()].map(({ name, role, manifest_digest }) => ({ name, role, manifest_digest })) };
    const observation = await executeRolloutDriver(
      { scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath, artifactDir },
      { gitCommit: () => "1".repeat(40), hostname: () => host, qualifiedImages, rolloutPlan, rolloutAdapter },
    );
    const evaluated = evaluateLiveObservation(observation, {
      driverId: "celld-live-rollout", scenarioId: "UAT-CELLD-011", runId: "test-run",
      assertionIds: new Set(["CELLD.011.BUDGET", "CELLD.011.SAFETY", "CELLD.011.REFUSAL"]),
      outputDir, expectedGit: "1".repeat(40), expectedHostSha256: authorized.environment.host_sha256, expectedProfileId: "test-profile",
    }, SAFE_LIVE_EVALUATORS);
    assert.equal(evaluated.kind, "evaluated");
    assert.ok(evaluated.assertions.every((assertion) => assertion.status === "PASS"));
    assert.equal(observation.artifacts.length, 2);
    assert.equal(observation.cleanup.status, "passed");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
