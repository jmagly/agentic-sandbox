import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  CREDENTIAL_PROVENANCE_PREREQUISITES,
  CREDENTIAL_PROVENANCE_PROFILE_SCHEMA,
  executeCredentialProvenanceDriver,
  validateCredentialProvenanceProfile,
} from "../../../scripts/celld-live-credential-provenance.mjs";
import { evaluateLiveObservation } from "../../../scripts/celld-uat-live-protocol.mjs";

const DRIVER_ID = "celld-live-credential-provenance";
const SCENARIO_ID = "UAT-CELLD-013";
const ASSERTIONS = new Set(["CELLD.013.NO_LEAK", "CELLD.013.SCOPE", "CELLD.013.PROVENANCE"]);

function fixture(enabled = true, campaignOverrides = {}) {
  const directory = mkdtempSync(join(tmpdir(), "celld-live-credential-provenance-test-"));
  const host = "synthetic-titan";
  const hostHash = createHash("sha256").update(host).digest("hex");
  const campaignPath = join(directory, "credential-provenance.json");
  const profilePath = join(directory, "profile.json");
  const campaign = { schema_version: CREDENTIAL_PROVENANCE_PROFILE_SCHEMA, run_id: "test-run", mode: "prerequisite-assessment", ...campaignOverrides };
  const profile = {
    schema_version: "agentic-sandbox.celld-live-profile/v1",
    profile_id: "test-profile",
    run_id: "test-run",
    expected_sandbox_git: "1".repeat(40),
    environment: { kind: "disposable-local", single_host: true, host_sha256: hostHash },
    authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
    drivers: { [DRIVER_ID]: { enabled, config_path: campaignPath } },
  };
  writeFileSync(campaignPath, `${JSON.stringify(campaign)}\n`, { mode: 0o600 });
  writeFileSync(profilePath, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
  chmodSync(campaignPath, 0o600);
  chmodSync(profilePath, 0o600);
  return { directory, host, hostHash, profilePath };
}

function execute(value, dependencies = {}) {
  return executeCredentialProvenanceDriver({ scenarioId: SCENARIO_ID, runId: "test-run", liveProfilePath: value.profilePath }, {
    gitCommit: () => "1".repeat(40),
    hostname: () => value.host,
    ...dependencies,
  });
}

test("credential provenance assessment profile is fixed and versioned", () => {
  assert.deepEqual(validateCredentialProvenanceProfile({ schema_version: CREDENTIAL_PROVENANCE_PROFILE_SCHEMA, run_id: "test-run", mode: "prerequisite-assessment" }), []);
  assert.match(validateCredentialProvenanceProfile({ schema_version: "agentic-sandbox.celld-live-credential-provenance/v2", run_id: "test-run", mode: "prerequisite-assessment" }).join("; "), /schema_version/);
  assert.match(validateCredentialProvenanceProfile({ schema_version: CREDENTIAL_PROVENANCE_PROFILE_SCHEMA, run_id: "test-run", mode: "prerequisite-assessment", access_key: "inline" }).join("; "), /access_key is not allowed/);
});

test("disabled credential provenance driver returns pre-mutation NOT_RUN evidence", () => {
  const value = fixture(false);
  try {
    const observation = executeCredentialProvenanceDriver({ scenarioId: SCENARIO_ID, runId: "test-run", liveProfilePath: value.profilePath });
    assert.equal(observation.mutation_started, false);
    assert.equal(observation.prerequisites[0].reason_code, "CELLD_LIVE_CREDENTIAL_PROVENANCE_DRIVER_DISABLED");
    assert.deepEqual(observation.assertions, []);
    assert.deepEqual(observation.artifacts, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("missing credential provenance prerequisites remain typed and pre-mutation", () => {
  const value = fixture();
  try {
    const observation = execute(value);
    assert.equal(observation.mutation_started, false);
    assert.deepEqual(observation.prerequisites, CREDENTIAL_PROVENANCE_PREREQUISITES);
    assert.deepEqual(observation.prerequisites.map((item) => item.id), [
      "CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION",
      "CELLD_STORAGE_QUALIFICATION",
      "CELLD_FLEET_QUALIFICATION",
      "CELLD_NETWORK_AUTH_QUALIFICATION",
      "CELLD_BROKER_INTEGRATED_FLEET",
      "CELLD_S3_CREDENTIAL_ROTATION_CONTROL",
      "CELLD_REQUEST_HMAC_ROTATION_CONTROL",
      "CELLD_MTLS_IDENTITY_ROTATION_CONTROL",
      "CELLD_PEER_SECRET_ROTATION_CONTROL",
      "CELLD_FIXTURE_ADMIN_LIFECYCLE_CONTROL",
      "CELLD_SECRET_SCAN_SURFACES",
      "CELLD_SIGNED_ARTIFACT_VERIFIER",
      "CELLD_SCOPED_OBJECT_STORE_IDENTITIES",
      "CELLD_SUPPORT_BUNDLE_EXPORTER",
    ]);
    assert.ok(observation.prerequisites.every((item) => item.status === "unavailable"));
    assert.deepEqual(observation.assertions, []);
    assert.deepEqual(observation.metrics, []);
    assert.deepEqual(observation.faults, []);
    assert.deepEqual(observation.artifacts, []);
    assert.equal(observation.cleanup.status, "not_required");
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("missing prerequisites cannot invoke trusted evaluators or produce PASS", () => {
  const value = fixture();
  try {
    const observation = execute(value);
    let evaluatorCalls = 0;
    const evaluators = Object.fromEntries([...ASSERTIONS].map((id) => [id, () => { evaluatorCalls += 1; return { passed: true }; }]));
    const result = evaluateLiveObservation(observation, {
      driverId: DRIVER_ID,
      runId: "test-run",
      scenarioId: SCENARIO_ID,
      assertionIds: ASSERTIONS,
      outputDir: value.directory,
      expectedProfileId: "test-profile",
      expectedGit: "1".repeat(40),
      expectedHostSha256: value.hostHash,
    }, evaluators);
    assert.equal(result.kind, "not_run");
    assert.equal(evaluatorCalls, 0);
    assert.deepEqual(result.assertions, []);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("an unsupported assessment profile fails before prerequisite probing", () => {
  const value = fixture(true, { schema_version: "agentic-sandbox.celld-live-credential-provenance/v2" });
  let prerequisiteCalls = 0;
  try {
    assert.throws(() => execute(value, { prerequisites: () => { prerequisiteCalls += 1; return CREDENTIAL_PROVENANCE_PREREQUISITES; } }), /schema_version/);
    assert.equal(prerequisiteCalls, 0);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});

test("declared readiness cannot cross the unimplemented mutation boundary", () => {
  const value = fixture();
  const available = CREDENTIAL_PROVENANCE_PREREQUISITES.map((item) => ({ ...item, status: "available", reason_code: `${item.id}_AVAILABLE` }));
  try {
    assert.throws(() => execute(value, { prerequisites: () => available }), /mutation campaign is not implemented/);
  } finally { rmSync(value.directory, { recursive: true, force: true }); }
});
