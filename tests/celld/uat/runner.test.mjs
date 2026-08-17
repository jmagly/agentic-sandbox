import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
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
  verifyManifest,
  writeOutputs,
} from "../../../scripts/run-celld-uat.mjs";
import { REQUIRED_AUTHORITY, validateAuthorityMatrix } from "../../../scripts/celld-uat-contract-check.mjs";
import {
  LIVE_OBSERVATION_SCHEMA,
  LIVE_PROFILE_SCHEMA,
  evaluateLiveObservation,
  runSafeLiveDriver,
  validateLiveObservation,
  validateLiveProfile,
} from "../../../scripts/celld-uat-live-protocol.mjs";

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

  const invalidLive = structuredClone(catalog);
  const liveExecution = invalidLive.scenarios.find((scenario) => scenario.id === "UAT-CELLD-003").execution;
  liveExecution.live_drivers[0].id = "arbitrary-shell";
  liveExecution.live_drivers[0].program = "sh";
  const liveErrors = validateCatalog(invalidLive);
  assert.ok(liveErrors.some((error) => error.includes("live_drivers[0].id is not allowlisted")));
  assert.ok(liveErrors.some((error) => error.includes("live_drivers[0].program is not allowed")));

  const overlapping = structuredClone(catalog);
  overlapping.scenarios.find((scenario) => scenario.id === "UAT-CELLD-003").execution.live_drivers[0].covers_assertions.push("CELLD.003.DETERMINISTIC_LEDGER");
  assert.ok(validateCatalog(overlapping).some((error) => error.includes("overlaps assertion")));
});

function liveProfile(overrides = {}) {
  return {
    schema_version: LIVE_PROFILE_SCHEMA,
    profile_id: "titan-test",
    run_id: "live-test",
    expected_sandbox_git: "a".repeat(40),
    environment: { kind: "titan-single-host", single_host: true, host_sha256: "b".repeat(64) },
    authorization: { destructive_faults: false, inventory_path: "/tmp/celld-test-inventory.json" },
    drivers: { "celld-live-orchestration": { enabled: true, config_path: "/tmp/celld-orchestration.json" } },
    ...overrides,
  };
}

function liveObservation(assertionIds, overrides = {}) {
  return {
    schema_version: LIVE_OBSERVATION_SCHEMA,
    driver_id: "celld-live-orchestration",
    run_id: "live-test",
    scenario_id: "UAT-CELLD-003",
    started_at: "2026-08-17T00:00:00.000Z",
    ended_at: "2026-08-17T00:00:01.000Z",
    mutation_started: true,
    prerequisites: [{ id: "LIVE_FLEET", status: "available", reason_code: "READY" }],
    assertions: assertionIds.map((id) => ({ id, measurements: { count: 1 }, evidence_refs: [] })),
    identities: { profile_id: "titan-test", sandbox_git: "a".repeat(40), environment_host_sha256: "b".repeat(64), driver_version: "test-v1" },
    metrics: [],
    faults: [],
    artifacts: [],
    cleanup: { status: "passed", assertions: ["no residue"] },
    ...overrides,
  };
}

test("live profile is strict, allowlisted, and rejects inline secret-like values", () => {
  assert.deepEqual(validateLiveProfile(liveProfile()), []);
  const unknown = liveProfile({ drivers: { "arbitrary-shell": { enabled: true, config_path: "/tmp/config.json" } } });
  assert.ok(validateLiveProfile(unknown).some((error) => error.includes("unregistered driver")));
  const secret = liveProfile({ authorization: { destructive_faults: false, inventory_path: "/tmp/inventory-token=super-secret" } });
  assert.ok(validateLiveProfile(secret).some((error) => error.includes("secret-like")));
  const destructive = liveProfile({ authorization: { destructive_faults: true, inventory_path: "/tmp/inventory.json" } });
  assert.ok(validateLiveProfile(destructive).some((error) => error.includes("exact_run_owner")));
  const wrongOwner = liveProfile({ authorization: { destructive_faults: true, inventory_path: "/tmp/inventory.json", exact_run_owner: "another-run" } });
  assert.ok(validateLiveProfile(wrongOwner).some((error) => error.includes("must match profile.run_id")));
});

test("live observations cannot self-declare verdicts or cross assertion assignments", () => {
  const context = { driverId: "celld-live-orchestration", runId: "live-test", scenarioId: "UAT-CELLD-003", assertionIds: new Set(["CELLD.003.ONE_EFFECT"]) };
  assert.deepEqual(validateLiveObservation(liveObservation(["CELLD.003.ONE_EFFECT"]), context), []);
  const selfDeclared = liveObservation(["CELLD.003.ONE_EFFECT"], { status: "PASS" });
  assert.ok(validateLiveObservation(selfDeclared, context).some((error) => error.includes("status is not allowed")));
  const crossed = liveObservation(["CELLD.003.COLLISION"]);
  assert.ok(validateLiveObservation(crossed, context).some((error) => error.includes("unassigned assertion")));
});

test("trusted evaluators derive pass and fail while missing evidence is ERROR", () => {
  const outputDir = mkdtempSync(join(tmpdir(), "celld-live-evaluate-"));
  const context = { driverId: "celld-live-orchestration", runId: "live-test", scenarioId: "UAT-CELLD-003", assertionIds: new Set(["CELLD.003.ONE_EFFECT"]), outputDir };
  try {
    const pass = evaluateLiveObservation(liveObservation(["CELLD.003.ONE_EFFECT"]), context, {
      "CELLD.003.ONE_EFFECT": (measurements) => ({ passed: measurements.count === 1, observed: measurements.count }),
    });
    assert.equal(pass.kind, "evaluated");
    assert.equal(pass.assertions[0].status, "PASS");

    const fail = evaluateLiveObservation(liveObservation(["CELLD.003.ONE_EFFECT"], { assertions: [{ id: "CELLD.003.ONE_EFFECT", measurements: { count: 2 }, evidence_refs: [] }] }), context, {
      "CELLD.003.ONE_EFFECT": (measurements) => ({ passed: measurements.count === 1, observed: measurements.count }),
    });
    assert.equal(fail.assertions[0].status, "FAIL");

    assert.equal(evaluateLiveObservation(liveObservation([]), context, {}).kind, "error");
    assert.equal(evaluateLiveObservation(liveObservation(["CELLD.003.ONE_EFFECT"]), context, {}).kind, "error");
  } finally { rmSync(outputDir, { recursive: true, force: true }); }
});

test("pre-mutation prerequisite absence is NOT_RUN but absence after mutation is ERROR", () => {
  const outputDir = mkdtempSync(join(tmpdir(), "celld-live-prerequisite-"));
  const context = { driverId: "celld-live-orchestration", runId: "live-test", scenarioId: "UAT-CELLD-003", assertionIds: new Set(["CELLD.003.ONE_EFFECT"]), outputDir };
  try {
    const unavailable = liveObservation([], { mutation_started: false, prerequisites: [{ id: "LIVE_FLEET", status: "unavailable", reason_code: "FLEET_MISSING" }] });
    assert.equal(evaluateLiveObservation(unavailable, context, {}).kind, "not_run");
    unavailable.mutation_started = true;
    assert.equal(evaluateLiveObservation(unavailable, context, {}).kind, "error");
  } finally { rmSync(outputDir, { recursive: true, force: true }); }
});

test("artifact ingestion rejects missing or tampered evidence", () => {
  const outputDir = mkdtempSync(join(tmpdir(), "celld-live-artifact-"));
  const artifactDir = join(outputDir, "artifacts");
  const context = { driverId: "celld-live-orchestration", runId: "live-test", scenarioId: "UAT-CELLD-003", assertionIds: new Set(["CELLD.003.ONE_EFFECT"]), outputDir };
  try {
    mkdirSync(artifactDir);
    const bytes = Buffer.from("bounded observation\n");
    writeFileSync(join(artifactDir, "counts.json"), bytes);
    const artifact = { path: "artifacts/counts.json", mime_type: "application/json", sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length, contains_restricted_data: false };
    const observation = liveObservation(["CELLD.003.ONE_EFFECT"], { artifacts: [artifact], assertions: [{ id: "CELLD.003.ONE_EFFECT", measurements: { count: 1 }, evidence_refs: [artifact.path] }] });
    assert.equal(evaluateLiveObservation(observation, context, { "CELLD.003.ONE_EFFECT": () => ({ passed: true }) }).kind, "evaluated");
    writeFileSync(join(artifactDir, "counts.json"), "tampered\n");
    assert.equal(evaluateLiveObservation(observation, context, { "CELLD.003.ONE_EFFECT": () => ({ passed: true }) }).kind, "error");
  } finally { rmSync(outputDir, { recursive: true, force: true }); }
});

test("live driver process timeout is an ERROR rather than NOT_RUN or FAIL", () => {
  const result = runSafeLiveDriver({ program: process.execPath, args: ["-e", "setTimeout(() => {}, 10000)"], timeout_ms: 20 }, {
    driverId: "celld-live-orchestration", scenarioId: "UAT-CELLD-003", runId: "live-test", profilePath: "/tmp/profile.json", outputDir: "/tmp/output", repoRoot: process.cwd(),
  });
  assert.equal(result.kind, "error");
  assert.equal(result.cleanup_status, "failed");
});

test("an absent registered driver is a pre-mutation NOT_RUN", () => {
  const result = runSafeLiveDriver({ program: process.execPath, args: ["scripts/does-not-exist.mjs"], timeout_ms: 20 }, {
    driverId: "celld-live-orchestration", scenarioId: "UAT-CELLD-003", runId: "live-test", profilePath: "/tmp/profile.json", outputDir: "/tmp/output", repoRoot: process.cwd(),
  });
  assert.equal(result.kind, "not_run");
  assert.match(result.reason, /not installed/);
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

test("runner derives live assertion verdicts from observations and trusted evaluators", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-runner-"));
  try {
    const scenario = catalog.scenarios.find((candidate) => candidate.id === "UAT-CELLD-003");
    const result = await runUat(catalog, [scenario], {
      runId: "live-test",
      outputDir: directory,
      liveProfile: liveProfile(),
      liveProfilePath: "/tmp/live-profile.json",
      execute: async () => ({ kind: "pass", reason: "support passed", cleanup_status: "not_required", command: {} }),
      executeLive: async () => ({ kind: "observation", observation: liveObservation(["CELLD.003.ONE_EFFECT", "CELLD.003.COLLISION"]), command: { shell: false } }),
      evaluators: {
        "CELLD.003.ONE_EFFECT": (measurements) => ({ passed: measurements.count === 1 }),
        "CELLD.003.COLLISION": (measurements) => ({ passed: measurements.count === 1 }),
      },
    });
    assert.equal(result.records[0].status, "PASS");
    assert.deepEqual(result.records[0].assertions.map((assertion) => assertion.status), ["PASS", "PASS", "PASS"]);
    assert.equal(result.records[0].live_driver_evidence[0].status, "PASS");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("cleanup failure outranks live assertion results and returns exit 4", async () => {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-cleanup-"));
  try {
    const scenario = catalog.scenarios.find((candidate) => candidate.id === "UAT-CELLD-003");
    const observation = liveObservation(["CELLD.003.ONE_EFFECT", "CELLD.003.COLLISION"], { cleanup: { status: "failed", assertions: ["residue remains"] } });
    const result = await runUat(catalog, [scenario], {
      runId: "live-test", outputDir: directory, liveProfile: liveProfile(), liveProfilePath: "/tmp/live-profile.json",
      execute: async () => ({ kind: "pass", reason: "support passed", cleanup_status: "not_required", command: {} }),
      executeLive: async () => ({ kind: "observation", observation, command: { shell: false } }),
      evaluators: {
        "CELLD.003.ONE_EFFECT": () => ({ passed: true }),
        "CELLD.003.COLLISION": () => ({ passed: false }),
      },
    });
    assert.equal(result.records[0].status, "ERROR");
    assert.equal(result.records[0].reason_code, "CLEANUP_FAILED");
    assert.equal(determineExitCode({ records: result.records }), 4);
    assert.deepEqual(result.records[0].assertions.map((assertion) => assertion.status), ["PASS", "PASS", "FAIL"]);
  } finally { rmSync(directory, { recursive: true, force: true }); }
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
  assert.deepEqual(record.assertions.map((assertion) => assertion.status), ["PASS", "PASS", "NOT_RUN", "NOT_RUN"]);
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
    const summary = JSON.parse(readFileSync(join(directory, "summary.json"), "utf8"));
    assert.equal(summary.counts.NOT_RUN, 1);
    assert.equal(summary.supporting_checks.selected_scenarios, 0);
    assert.equal(summary.selection.label, "partial-selection");
    assert.equal(JSON.parse(readFileSync(join(directory, "evidence.jsonl"), "utf8")).scenario_id, scenario.id);
    assert.match(readFileSync(join(directory, "junit.xml"), "utf8"), /<skipped/);
    const manifest = readFileSync(join(directory, "manifest.sha256"), "utf8");
    for (const name of ["summary.json", "evidence.jsonl", "junit.xml", "report.md"]) assert.match(manifest, new RegExp(`  ${name}\\n`));
    assert.deepEqual(verifyManifest(directory, manifest), []);
    writeFileSync(join(directory, "report.md"), "tampered\n");
    assert.ok(verifyManifest(directory, manifest).some((error) => error.includes("report.md")));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
