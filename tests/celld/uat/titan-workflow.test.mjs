import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(new URL("../../../.gitea/workflows/celld-qualification.yml", import.meta.url), "utf8");

test("Celld qualification is manual, Titan-only, and capacity-one", () => {
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /runs-on: titan/);
  assert.doesNotMatch(workflow, /runs-on: rust/);
  assert.match(workflow, /group: agentic-sandbox-celld-qualification-titan/);
  assert.match(workflow, /group: agentic-sandbox-vm-e2e/);
  assert.match(workflow, /CARGO_BUILD_JOBS: "8"/);
  assert.match(workflow, /CARGO_INCREMENTAL: "0"/);
  assert.match(workflow, /test ! -e "\$\{CARGO_TARGET_DIR\}"/);
  assert.doesNotMatch(workflow, /install -d[^\n]*CARGO_TARGET_DIR/);
});

test("Celld qualification fails closed on exact-host capacity and provenance", () => {
  assert.match(workflow, /CELLD_QUALIFICATION_EXPECTED_HOST: titan/);
  assert.match(workflow, /CELLD_QUALIFICATION_EXPECTED_COMMIT: \$\{\{ github\.sha \}\}/);
  assert.match(workflow, /CELLD_QUALIFICATION_MIN_BUILD_FREE_GIB: "400"/);
  assert.match(workflow, /scripts\/celld-titan-preflight\.mjs/);
  assert.match(workflow, /artifacts\/celld-titan-preflight\.json/);
});

test("Celld qualification previews destructive cleanup and always verifies it", () => {
  const firstDryRun = workflow.indexOf("--dry-run");
  const firstMutation = workflow.indexOf("Reap stale disposable E2E resources");
  assert.ok(firstDryRun > 0 && firstDryRun < firstMutation);
  assert.match(workflow, /scripts\/reap-e2e-vms\.sh/g);
  assert.match(workflow, /Verify qualification VM cleanup/);
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /exit 4/);
});

test("Celld qualification uploads authoritative evidence without claiming operator UAT", () => {
  assert.match(workflow, /node scripts\/run-celld-uat\.mjs/);
  assert.match(workflow, /--trigger automated/);
  assert.match(workflow, /actions\/upload-artifact@c6a366c94c3e0affe28c06c8df20a878f24da3cf/);
  assert.match(workflow, /sha256sum --check manifest\.sha256/);
  assert.match(workflow, /Report bounded qualification verdict/);
  assert.match(workflow, /stdout_tail: \.command\.stdout_tail/);
  assert.doesNotMatch(workflow, /Upload qualification evidence[\s\S]{0,100}continue-on-error: true/);
  assert.match(workflow, /tests\/celld\/uat\/results\/titan-/);
  assert.doesNotMatch(workflow, /make test-celld-soak/);
  assert.doesNotMatch(workflow, /make test-celld-human-uat/);
});
