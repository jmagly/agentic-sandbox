import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  QUALIFICATION_DEPENDENCY_SOURCES,
  QUALIFICATION_DRIVER_IDS,
  evaluateQualificationReadiness,
} from "../../../scripts/celld-qualification-readiness.mjs";

const catalog = JSON.parse(readFileSync(new URL("./scenarios.json", import.meta.url), "utf8"));
const workflowSource = readFileSync(new URL("../../../.gitea/workflows/celld-qualification.yml", import.meta.url), "utf8");
const git = "1".repeat(40);

function evaluate(overrides = {}) {
  return evaluateQualificationReadiness({ catalog: structuredClone(catalog), laneName: "complete", expectedGit: git, actualGit: git, workflowSource, ...overrides });
}

function profile(disabled = null) {
  return {
    schema_version: "agentic-sandbox.celld-live-profile/v1",
    profile_id: "test-profile",
    run_id: "test-run",
    expected_sandbox_git: git,
    environment: { kind: "disposable-local", single_host: true, host_sha256: "2".repeat(64) },
    authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
    drivers: Object.fromEntries(Object.values(QUALIFICATION_DRIVER_IDS).map((id) => [id, { enabled: id !== disabled, config_path: "/tmp/protected-config.json" }])),
  };
}

test("complete readiness binds 13 scenarios, 44 owners, every formula, eight drivers, and nine dependencies", () => {
  const result = evaluate();
  assert.equal(result.ready, true, JSON.stringify(result.checks.filter((entry) => entry.status === "FAIL")));
  assert.equal(result.scenarios.length, 13);
  assert.equal(result.assertions.length, 44);
  assert.equal(result.assertions.filter((record) => record.formula === "trusted" || record.formula === "candidate_withheld").length, 30);
  assert.equal(result.drivers.length, 8);
  assert.equal(result.dependencies.length, 9);
  assert.ok(result.dependencies.every((dependency) => dependency.exact_git === git && dependency.acceptable));
});

test("withheld credential formulas are visible but cannot disappear from readiness", () => {
  const result = evaluate();
  assert.deepEqual(result.assertions.filter((record) => record.formula === "candidate_withheld").map((record) => record.assertion_id).sort(), ["CELLD.013.NO_LEAK", "CELLD.013.PROVENANCE", "CELLD.013.SCOPE"]);
  assert.equal(result.checks.find((entry) => entry.id === "assertions.live_formulas").status, "PASS");
});

test("a missing exact driver source fails the readiness gate", () => {
  const missing = "scripts/celld-observability-controller.mjs";
  const result = evaluate({ fileStatus: (path) => path === missing ? { exists: false, regular_file: false, symlink: false } : { exists: true, regular_file: true, symlink: false } });
  assert.equal(result.ready, false);
  assert.equal(result.checks.find((entry) => entry.id === "dependencies.exact_head_sources").status, "FAIL");
});

test("duplicate assertion ownership fails even when the catalog assertion remains unique", () => {
  const changed = structuredClone(catalog);
  const scenario = changed.scenarios.find((entry) => entry.id === "UAT-CELLD-003");
  scenario.execution.live_drivers[0].covers_assertions.push(scenario.execution.supporting_covers_assertions[0]);
  const result = evaluate({ catalog: changed });
  assert.equal(result.ready, false);
  assert.equal(result.checks.find((entry) => entry.id === "assertions.one_owner").status, "FAIL");
});

test("generated complete profile must enable every required driver", () => {
  const result = evaluate({ profile: profile("celld-live-credential-provenance") });
  assert.equal(result.ready, false);
  assert.equal(result.checks.find((entry) => entry.id === "profile.required_drivers_enabled").status, "FAIL");
  assert.equal(evaluate({ profile: profile() }).ready, true);
});

test("readiness fails on a different checkout or missing end janitor", () => {
  assert.equal(evaluate({ actualGit: "3".repeat(40) }).checks.find((entry) => entry.id === "git.exact_head").status, "FAIL");
  const changedWorkflow = workflowSource.replace("Preview final disposable E2E cleanup", "Final cleanup preview removed");
  const result = evaluate({ workflowSource: changedWorkflow });
  assert.equal(result.ready, false);
  assert.equal(result.checks.find((entry) => entry.id === "workflow.end_janitor_preview").status, "FAIL");
});

test("readiness rejects a dirty checkout and a weakened shell ceiling", () => {
  assert.equal(evaluate({ gitClean: false }).checks.find((entry) => entry.id === "git.clean").status, "FAIL");
  const changedWorkflow = workflowSource.replace("420m \\", "419m \\");
  const result = evaluate({ workflowSource: changedWorkflow });
  assert.equal(result.ready, false);
  assert.equal(result.checks.find((entry) => entry.id === "workflow.shell_timeout").status, "FAIL");
});

test("dependency source map is exact and excludes soak and human UAT", () => {
  assert.deepEqual(Object.keys(QUALIFICATION_DEPENDENCY_SOURCES), ["#763", "#764", "#765", "#766", "#767", "#768", "#769", "#770", "#771"]);
  assert.equal(JSON.stringify(QUALIFICATION_DEPENDENCY_SOURCES).includes("016"), false);
  assert.equal(JSON.stringify(QUALIFICATION_DEPENDENCY_SOURCES).includes("017"), false);
});
