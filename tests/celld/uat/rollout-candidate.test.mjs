import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

import {
  CELLD_ROLLOUT_CANDIDATES_PATH,
  loadReviewedRolloutCandidates,
  validateRolloutCandidates,
} from "../../../scripts/celld-rollout-candidate.mjs";

test("reviewed rollout candidate inventory matches the exact research record", () => {
  const document = JSON.parse(readFileSync(CELLD_ROLLOUT_CANDIDATES_PATH, "utf8"));
  assert.deepEqual(validateRolloutCandidates(document), []);
  const [candidate] = loadReviewedRolloutCandidates();
  assert.equal(candidate.version, "0.3.0");
  assert.equal(candidate.qualification_status, "reviewed_unqualified");
  assert.deepEqual(candidate.compatible_from_versions, ["0.2.1"]);
});

test("candidate validation rejects metadata mutation and alternate inventory paths", () => {
  const document = JSON.parse(readFileSync(CELLD_ROLLOUT_CANDIDATES_PATH, "utf8"));
  document.candidates[0].qualification_status = "qualified";
  assert.match(validateRolloutCandidates(document).join("; "), /does not match the exact approved research record/);

  const directory = mkdtempSync(join(tmpdir(), "celld-rollout-candidate-test-"));
  try {
    const alternate = join(directory, "candidates.json");
    writeFileSync(alternate, `${JSON.stringify(document)}\n`, { mode: 0o600 });
    assert.throws(() => loadReviewedRolloutCandidates(alternate), /not the reviewed repository path/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("candidate checker emits only a bounded non-promoting summary", () => {
  const script = new URL("../../../scripts/celld-rollout-candidate.mjs", import.meta.url).pathname;
  const result = spawnSync(process.execPath, [script, "check", "--input", CELLD_ROLLOUT_CANDIDATES_PATH], { encoding: "utf8", shell: false });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), {
    status: "PASS",
    candidates: 1,
    versions: ["0.3.0"],
    qualification_statuses: ["reviewed_unqualified"],
  });
});
