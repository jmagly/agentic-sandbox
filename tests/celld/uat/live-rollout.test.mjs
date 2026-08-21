import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { executeRolloutDriver, qualifiedCelldArtifacts, selectDistinctRolloutPair } from "../../../scripts/celld-live-rollout.mjs";

function orchestrationConfig(repoRoot) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1", run_id: "test-run",
    working_root: "/dev/shm/agentic-celld-orchestration/test-run",
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
  const pair = [...one, { version: "0.2.2", commit: "2".repeat(40), manifest_digest: `sha256:${"b".repeat(64)}` }];
  assert.deepEqual(selectDistinctRolloutPair(pair), { previous: pair[0], candidate: pair[1] });
});

test("UAT-011 is typed NOT_RUN before mutation without a qualified old/new pair", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const observation = executeRolloutDriver({ scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => host });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_QUALIFIED_ROLLOUT_PAIR_UNAVAILABLE");
    assert.deepEqual(observation.assertions, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("adding a pair still returns pre-mutation NOT_RUN until the controller is reviewed", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-rollout-pair-test-"));
  try {
    const repoRoot = new URL("../../../", import.meta.url).pathname.replace(/\/$/, ""), host = "synthetic-titan";
    const configPath = join(directory, "orchestration.json"), profilePath = join(directory, "profile.json");
    writeFileSync(configPath, `${JSON.stringify(orchestrationConfig(repoRoot))}\n`, { mode: 0o600 });
    writeFileSync(profilePath, `${JSON.stringify(profile(configPath, createHash("sha256").update(host).digest("hex")))}\n`, { mode: 0o600 });
    chmodSync(configPath, 0o600); chmodSync(profilePath, 0o600);
    const qualifiedImages = () => ({ schema_version: "agentic-sandbox.celld-images/v1", platform: "linux/amd64", celld: [
      { version: "0.2.1", commit: "1".repeat(40), manifest_digest: `sha256:${"a".repeat(64)}` },
      { version: "0.2.2", commit: "2".repeat(40), manifest_digest: `sha256:${"b".repeat(64)}` },
    ] });
    const observation = executeRolloutDriver({ scenarioId: "UAT-CELLD-011", runId: "test-run", liveProfilePath: profilePath }, { gitCommit: () => "1".repeat(40), hostname: () => host, qualifiedImages });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_ROLLOUT_CONTROLLER_UNAVAILABLE");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
