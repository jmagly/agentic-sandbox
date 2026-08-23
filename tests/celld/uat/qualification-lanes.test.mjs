import assert from "node:assert/strict";
import test from "node:test";

import { QUALIFICATION_LANES, renderQualificationGithubEnv, resolveQualificationLane } from "../../../scripts/celld-qualification-lanes.mjs";

const allIds = Array.from({ length: 13 }, (_value, index) => `UAT-CELLD-${String(index + 3).padStart(3, "0")}`);

test("complete qualification remains the exact default UAT 003-015 selection", () => {
  const lane = resolveQualificationLane("complete");
  assert.deepEqual(lane.selectedIds, allIds);
  assert.equal(lane.runCatalog, true);
  assert.equal(lane.runMigration, true);
  assert.ok(Object.values(lane.drivers).every(Boolean));
});

test("Wave 3 issue lanes are strict, independent, and partial", () => {
  assert.deepEqual(resolveQualificationLane("issue-764").selectedIds, allIds.slice(0, 4));
  assert.deepEqual(resolveQualificationLane("issue-765").selectedIds, ["UAT-CELLD-010", "UAT-CELLD-012"]);
  assert.deepEqual(resolveQualificationLane("issue-767").selectedIds, ["UAT-CELLD-007", "UAT-CELLD-008", "UAT-CELLD-009"]);
  assert.deepEqual(resolveQualificationLane("issue-771").selectedIds, []);
  assert.equal(resolveQualificationLane("issue-771").runCatalog, false);
  assert.equal(resolveQualificationLane("issue-771").runMigration, true);
  for (const name of Object.keys(QUALIFICATION_LANES).filter((candidate) => candidate !== "complete")) {
    assert.notDeepEqual(resolveQualificationLane(name).selectedIds, allIds);
  }
});

test("issue lanes enable only the drivers they own", () => {
  assert.deepEqual(Object.entries(resolveQualificationLane("issue-764").drivers).filter(([, enabled]) => enabled).map(([name]) => name), ["orchestration"]);
  assert.deepEqual(Object.entries(resolveQualificationLane("issue-765").drivers).filter(([, enabled]) => enabled).map(([name]) => name), ["networkAuth", "storageTopology"]);
  assert.deepEqual(Object.entries(resolveQualificationLane("issue-767").drivers).filter(([, enabled]) => enabled).map(([name]) => name), ["worker"]);
  assert.deepEqual(Object.entries(resolveQualificationLane("issue-771").drivers).filter(([, enabled]) => enabled).map(([name]) => name), []);
});

test("lane resolver rejects unreviewed input and emits bounded GitHub environment records", () => {
  assert.throws(() => resolveQualificationLane("all; echo unsafe"), /unknown qualification lane/);
  const output = renderQualificationGithubEnv("issue-765");
  assert.match(output, /^CELLD_QUALIFICATION_SELECTED_IDS=UAT-CELLD-010,UAT-CELLD-012$/m);
  assert.match(output, /^CELLD_QUALIFICATION_EXPECTED_COUNT=2$/m);
  assert.match(output, /^CELLD_QUALIFICATION_RUN_CATALOG=true$/m);
  assert.match(output, /^CELLD_QUALIFICATION_RUN_MIGRATION=false$/m);
  assert.match(output, /^CELLD_QUALIFICATION_ENABLE_NETWORK_AUTH=true$/m);
  assert.doesNotMatch(output, /\r|%0A|::/);
});
