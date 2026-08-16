import assert from "node:assert/strict";
import { readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mkdtempSync } from "node:fs";
import test from "node:test";

import {
  DEFAULT_CATALOG,
  determineExitCode,
  redact,
  runSafeExecutor,
  runUat,
  selectScenarios,
  validateCatalog,
  writeOutputs,
} from "../../../scripts/run-celld-uat.mjs";
import { REQUIRED_AUTHORITY, validateAuthorityMatrix } from "../../../scripts/celld-uat-contract-check.mjs";

const catalog = JSON.parse(readFileSync(DEFAULT_CATALOG, "utf8"));

test("catalog contains 17 valid scenarios with stable issue coverage", () => {
  assert.deepEqual(validateCatalog(catalog), []);
  assert.equal(catalog.scenarios.length, 17);
  const issues = new Set(catalog.scenarios.flatMap((scenario) => scenario.issues));
  assert.deepEqual([...issues].sort(), ["#747", "#748", "#749", "#750", "#751", "#752", "#753", "#754"]);
  assert.equal(catalog.scenarios.filter((scenario) => scenario.trigger === "automated").length, 15);
  assert.equal(catalog.scenarios.filter((scenario) => scenario.trigger === "operator_soak").length, 1);
  assert.equal(catalog.scenarios.filter((scenario) => scenario.trigger === "operator_human").length, 1);
});

test("catalog validation rejects duplicate assertions and unallowlisted executors", () => {
  const invalid = structuredClone(catalog);
  invalid.scenarios[0].assertions[0].id = invalid.scenarios[1].assertions[0].id;
  invalid.scenarios[0].execution.executor_id = "arbitrary-shell";
  const errors = validateCatalog(invalid);
  assert.ok(errors.some((error) => error.includes("duplicated")));
  assert.ok(errors.some((error) => error.includes("not allowlisted")));

  const invalidSupporting = structuredClone(catalog);
  invalidSupporting.scenarios.find((scenario) => scenario.id === "UAT-CELLD-003").execution.supporting_executor_id = "arbitrary-shell";
  assert.ok(validateCatalog(invalidSupporting).some((error) => error.includes("supporting_executor_id is not allowlisted")));
});

test("authority matrix is complete and rejects ambiguous or incomplete ownership", () => {
  const matrix = JSON.parse(readFileSync(new URL("../../../docs/contracts/celld/authority-matrix-v1.json", import.meta.url), "utf8"));
  assert.deepEqual(validateAuthorityMatrix(matrix), []);
  assert.equal(matrix.fields.length, Object.keys(REQUIRED_AUTHORITY).length);
  const invalid = structuredClone(matrix);
  invalid.fields[0].forbidden_writers.pop();
  invalid.fields.push(structuredClone(invalid.fields[1]));
  const errors = validateAuthorityMatrix(invalid);
  assert.ok(errors.some((error) => error.includes("unique")));
  assert.ok(errors.some((error) => error.includes("exactly once")));
});

test("selectors intersect requested IDs and tags and reject unknown IDs", () => {
  const selected = selectScenarios(catalog.scenarios, { ids: ["UAT-CELLD-002"], tags: ["deterministic"] });
  assert.deepEqual(selected.map((scenario) => scenario.id), ["UAT-CELLD-002"]);
  assert.throws(() => selectScenarios(catalog.scenarios, { ids: ["UAT-CELLD-999"] }), /unknown scenario/);
  assert.deepEqual(
    selectScenarios(catalog.scenarios, { triggers: ["operator_soak"] }).map((scenario) => scenario.id),
    ["UAT-CELLD-016"],
  );
});

test("live prerequisites are recorded as NOT_RUN without invoking an executor", async () => {
  let invoked = false;
  const scenario = catalog.scenarios.find((candidate) => candidate.id === "UAT-CELLD-016");
  const result = await runUat(catalog, [scenario], {
    runId: "live-not-run",
    execute: async () => { invoked = true; throw new Error("must not execute"); },
  });
  assert.equal(invoked, false);
  assert.equal(result.records[0].status, "NOT_RUN");
  assert.ok(result.records[0].assertions.every((assertion) => assertion.status === "NOT_RUN"));
});

test("live scenarios run cached supporting checks without promoting live hard gates", async () => {
  let invocations = 0;
  const scenarios = catalog.scenarios.filter((candidate) => ["UAT-CELLD-003", "UAT-CELLD-004"].includes(candidate.id));
  const result = await runUat(catalog, scenarios, {
    runId: "live-supporting",
    execute: async () => {
      invocations += 1;
      return {
        kind: "pass",
        reason: "credential-free qualification checks passed",
        cleanup_status: "not_required",
        command: { argv_redacted: ["make", "test-celld"] },
      };
    },
  });
  assert.equal(invocations, 1);
  assert.deepEqual(result.records.map((record) => record.status), ["NOT_RUN", "NOT_RUN"]);
  assert.deepEqual(result.records.map((record) => record.supporting_evidence.status), ["PASS", "PASS"]);
  assert.deepEqual(result.records.map((record) => record.assertions[0].status), ["PASS", "PASS"]);
  assert.ok(result.records.every((record) => record.assertions.slice(1).every((assertion) => assertion.status === "NOT_RUN")));
});

test("deterministic evidence passes only covered assertions and redacts command data", async () => {
  const scenario = structuredClone(catalog.scenarios.find((candidate) => candidate.id === "UAT-CELLD-001"));
  scenario.execution.covers_assertions = ["CELLD.001.DISABLED_UNIT"];
  const result = await runUat(catalog, [scenario], {
    runId: "partial",
    execute: async () => ({
      kind: "pass",
      reason: "token=super-secret passed",
      cleanup_status: "not_required",
      command: { argv_redacted: [redact("token=super-secret")], stdout_preview: redact("Authorization: Bearer abc.def.ghi") },
    }),
  });
  const record = result.records[0];
  assert.equal(record.status, "NOT_RUN");
  assert.deepEqual(record.assertions.map((assertion) => assertion.status), ["PASS", "NOT_RUN", "NOT_RUN"]);
  assert.equal(record.command.argv_redacted[0], "token=[REDACTED]");
  assert.ok(!record.command.stdout_preview.includes("abc.def.ghi"));
  assert.ok(!JSON.stringify(record).includes("super-secret"));
});

test("security UAT records deterministic controls without promoting live denial gates", async () => {
  const scenario = catalog.scenarios.find((candidate) => candidate.id === "UAT-CELLD-012");
  const result = await runUat(catalog, [scenario], {
    runId: "security-partial",
    execute: async () => ({
      kind: "pass",
      reason: "deterministic security controls passed",
      cleanup_status: "not_required",
      command: { argv_redacted: ["make", "test-celld"] },
    }),
  });
  const record = result.records[0];
  assert.equal(record.status, "NOT_RUN");
  assert.deepEqual(record.assertions.map((assertion) => assertion.status), ["PASS", "PASS", "NOT_RUN", "PASS"]);
});

test("executor evidence stores bounded redacted output heads and tails", () => {
  const output = `${"A".repeat(5_000)}\ntoken=super-secret\n`;
  const result = runSafeExecutor({
    program: process.execPath,
    args: ["-e", `process.stdout.write(${JSON.stringify(output)})`],
    timeout_ms: 5_000,
  });
  assert.equal(result.kind, "pass");
  assert.equal(result.command.stdout_preview.length, 4_096);
  assert.ok(result.command.stdout_tail.length <= 4_096);
  assert.match(result.command.stdout_tail, /token=\[REDACTED\]/);
  assert.ok(!JSON.stringify(result.command).includes("super-secret"));
});

test("exit codes distinguish pass, fail, not-run, invalid evidence, and cleanup failure", () => {
  const record = (status, cleanup = "not_required") => ({ status, cleanup: { status: cleanup } });
  assert.equal(determineExitCode({ records: [record("PASS")] }), 0);
  assert.equal(determineExitCode({ records: [record("FAIL")] }), 1);
  assert.equal(determineExitCode({ records: [record("NOT_RUN")] }), 2);
  assert.equal(determineExitCode({ validationErrors: ["bad"] }), 3);
  assert.equal(determineExitCode({ records: [record("ERROR", "failed")] }), 4);
});

test("output writer emits parseable evidence, JUnit, report, and matching hashes", () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-uat-runner-"));
  try {
    const scenario = catalog.scenarios[0];
    const records = [{
      evidence_schema: catalog.evidence_schema,
      run_id: "output-test",
      scenario_id: scenario.id,
      title: "XML <safe>",
      issues: scenario.issues,
      lane: scenario.lane,
      status: "NOT_RUN",
      duration_ms: 0,
      assertions: scenario.assertions.map((assertion) => ({ ...assertion, status: "NOT_RUN" })),
      cleanup: { status: "not_required" },
    }];
    const written = writeOutputs(directory, catalog, "output-test", records);
    assert.deepEqual(written.files.sort(), ["evidence.jsonl", "junit.xml", "manifest.sha256", "report.md", "summary.json"]);
    assert.deepEqual(readdirSync(directory).sort(), written.files.sort());
    assert.equal(JSON.parse(readFileSync(join(directory, "summary.json"), "utf8")).counts.NOT_RUN, 1);
    assert.equal(JSON.parse(readFileSync(join(directory, "summary.json"), "utf8")).supporting_checks.selected_scenarios, 0);
    assert.equal(JSON.parse(readFileSync(join(directory, "evidence.jsonl"), "utf8")).scenario_id, scenario.id);
    assert.match(readFileSync(join(directory, "junit.xml"), "utf8"), /<skipped/);
    const manifest = readFileSync(join(directory, "manifest.sha256"), "utf8");
    for (const name of ["summary.json", "evidence.jsonl", "junit.xml", "report.md"]) assert.match(manifest, new RegExp(`  ${name}\\n`));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
