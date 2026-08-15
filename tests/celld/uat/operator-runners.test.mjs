import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { evaluateHuman, evaluateSoak, main } from "../../../scripts/run-celld-operator-uat.mjs";
import { DEFAULT_CATALOG } from "../../../scripts/run-celld-uat.mjs";

const catalog = JSON.parse(readFileSync(DEFAULT_CATALOG, "utf8"));
const scenario = (id) => catalog.scenarios.find((candidate) => candidate.id === id);
const fixture = (name) => {
  const value = JSON.parse(readFileSync(new URL(`operator/${name}`, import.meta.url), "utf8"));
  value.template_only = false;
  value.identities.sandbox_git = "a".repeat(40);
  value.identities.aiwg_git = "b".repeat(40);
  value.environment = {
    os: "Ubuntu 24.04",
    kernel: "6.8.0",
    architecture: "x86_64",
    cpu: "20 vCPU",
    memory: "64 GiB",
    substrate: "qemu",
    node_ids: name.startsWith("soak") ? ["node-1", "node-2", "node-3"] : ["node-1"],
    network_profile: "private-overlay-v1",
    storage_provider: "qualified-s3",
    trust_domain: "celld-uat.internal",
  };
  value.artifacts[0].path = "metrics.json";
  value.artifacts[0].sha256 = "1".repeat(64);
  return value;
};

test("soak evaluator requires a real 24-hour window and computes every hard gate", () => {
  const valid = fixture("soak-input.example.json");
  const result = evaluateSoak(valid, scenario("UAT-CELLD-016"));
  assert.deepEqual(result.assertions.map((item) => item.status), ["PASS", "PASS", "PASS"]);
  assert.ok(result.durationMs >= 86_400_000);

  const short = structuredClone(valid);
  short.ended_at = "2026-08-15T12:30:00Z";
  assert.throws(() => evaluateSoak(short, scenario("UAT-CELLD-016")), /24 actual wall-clock hours/);

  const unsafe = structuredClone(valid);
  unsafe.metrics.duplicate_effects = 1;
  unsafe.metrics.cpu_headroom_percent = 29;
  assert.deepEqual(evaluateSoak(unsafe, scenario("UAT-CELLD-016")).assertions.map((item) => item.status), ["PASS", "FAIL", "FAIL"]);
});

test("human evaluator requires every role and derives workflow, satisfaction, and safety gates", () => {
  const valid = fixture("human-input.example.json");
  const result = evaluateHuman(valid, scenario("UAT-CELLD-017"));
  assert.deepEqual(result.assertions.map((item) => item.status), ["PASS", "PASS", "PASS"]);
  assert.equal(result.signoffs.length, 5);

  const incomplete = structuredClone(valid);
  incomplete.participants.pop();
  const missing = evaluateHuman(incomplete, scenario("UAT-CELLD-017"));
  assert.equal(missing.assertions[0].status, "FAIL");
  assert.deepEqual(missing.assertions[0].observed.missing_roles, ["release-manager"]);

  const unsafe = structuredClone(valid);
  unsafe.participants[0].critical_defects = 1;
  unsafe.participants[0].workflows.find((workflow) => workflow.name === "rollback").destructive_context_shown = false;
  const failed = evaluateHuman(unsafe, scenario("UAT-CELLD-017"));
  assert.deepEqual(failed.assertions.map((item) => item.status), ["PASS", "FAIL", "FAIL"]);
});

test("operator evaluators refuse secret-like evidence", () => {
  const soak = fixture("soak-input.example.json");
  soak.notes = "token=do-not-store-this";
  assert.throws(() => evaluateSoak(soak, scenario("UAT-CELLD-016")), /secret-like material/);
});

test("operator CLI verifies and stages artifacts into the signed result manifest", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-operator-uat-"));
  try {
    const input = fixture("soak-input.example.json");
    const artifact = "{\"measured\":true}\n";
    writeFileSync(join(directory, "metrics.json"), artifact);
    input.artifacts[0] = {
      path: "metrics.json",
      sha256: createHash("sha256").update(artifact).digest("hex"),
      mime_type: "application/json",
      redaction_profile: "celld-v1",
      contains_restricted_data: false,
    };
    writeFileSync(join(directory, "input.json"), JSON.stringify(input));
    const output = join(directory, "result");
    const originalLog = console.log;
    console.log = () => {};
    try {
      assert.equal(await main(["soak", "--input", join(directory, "input.json"), "--output-dir", output, "--run-id", "operator-cli-test"]), 0);
    } finally {
      console.log = originalLog;
    }
    assert.ok(readdirSync(join(output, "artifacts")).includes("01-metrics.json"));
    assert.match(readFileSync(join(output, "manifest.sha256"), "utf8"), /  artifacts\/01-metrics\.json\n/);
    assert.deepEqual(JSON.parse(readFileSync(join(output, "summary.json"), "utf8")).measured_lane_verdicts, input.lane_verdicts);

    input.cleanup.status = "failed";
    writeFileSync(join(directory, "failed-input.json"), JSON.stringify(input));
    const failedOutput = join(directory, "failed-result");
    console.log = () => {};
    try {
      assert.equal(await main(["soak", "--input", join(directory, "failed-input.json"), "--output-dir", failedOutput, "--run-id", "operator-cleanup-failed"]), 4);
    } finally {
      console.log = originalLog;
    }
    assert.equal(JSON.parse(readFileSync(join(failedOutput, "evidence.jsonl"), "utf8")).reason_code, "CLEANUP_FAILED");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
