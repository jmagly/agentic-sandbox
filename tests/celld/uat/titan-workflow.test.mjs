import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(new URL("../../../.gitea/workflows/celld-qualification.yml", import.meta.url), "utf8");
const fleetWorkflow = readFileSync(new URL("../../../.gitea/workflows/celld-fleet-fixture.yml", import.meta.url), "utf8");
const mainCi = readFileSync(new URL("../../../.gitea/workflows/ci.yaml", import.meta.url), "utf8");
const gitignore = readFileSync(new URL("../../../.gitignore", import.meta.url), "utf8");

function exactWorkflowStep(document, name) {
  const header = `      - name: ${name}\n`;
  const start = document.indexOf(header);
  assert.notEqual(start, -1, `missing workflow step: ${name}`);
  assert.equal(document.indexOf(header, start + header.length), -1, `duplicate workflow step: ${name}`);

  const next = document.indexOf("\n      - name: ", start + header.length);
  return {
    body: document.slice(start, next === -1 ? document.length : next),
    start,
  };
}

test("ordinary CI runs credential-free Celld support on Titan", () => {
  assert.match(mainCi, /celld-deterministic:\n[\s\S]*?runs-on: titan/);
  assert.match(mainCi, /celld-deterministic:\n[\s\S]*?group: agentic-sandbox-celld-qualification-titan/);
  assert.match(mainCi, /CARGO_BUILD_JOBS: "8"/);
  assert.match(mainCi, /make test-celld-uat-structure/);
  assert.match(mainCi, /make test-celld/);
  assert.match(mainCi, /needs: \[lint, test, celld-deterministic\]/);
  assert.match(mainCi, /cargo clean --manifest-path management\/Cargo\.toml --target-dir "\$\{target\}"/);
});

test("Celld qualification is manual, Titan-only, and capacity-one", () => {
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /qualification_lane:[\s\S]*?default: complete[\s\S]*?- issue-764[\s\S]*?- issue-765[\s\S]*?- issue-767[\s\S]*?- issue-771/);
  assert.match(workflow, /runs-on: titan/);
  assert.doesNotMatch(workflow, /runs-on: rust/);
  assert.match(workflow, /group: agentic-sandbox-celld-qualification-titan/);
  assert.match(workflow, /group: agentic-sandbox-vm-e2e/);
  assert.match(workflow, /CARGO_BUILD_JOBS: "8"/);
  assert.match(workflow, /CARGO_INCREMENTAL: "0"/);
  assert.match(workflow, /name: Automated Celld UAT 003-015/);
  assert.match(workflow, /timeout-minutes: 480/);
  assert.match(workflow, /420m/);
  assert.match(workflow, /test ! -e "\$\{CARGO_TARGET_DIR\}"/);
  assert.doesNotMatch(workflow, /install -d[^\n]*CARGO_TARGET_DIR/);
});

test("Celld qualification generated evidence and scratch paths stay outside source cleanliness", () => {
  assert.match(workflow, /--output artifacts\/celld-titan-preflight\.json/);
  assert.match(workflow, /--output artifacts\/celld-qualification-readiness\.json/);
  assert.match(workflow, /CARGO_TARGET_DIR: \$\{\{ github\.workspace \}\}\/\.celld-target/);
  assert.match(workflow, /install -d -m 0700 "\$\{GITHUB_WORKSPACE\}\/\.ci-tmp"/);
  assert.match(workflow, /echo "TMPDIR=\/tmp" >> "\$\{GITHUB_ENV\}"[\s\S]*rm -rf -- "\$\{scratch\}"/);
  assert.match(workflow, /CARGO_TARGET_DIR="\$\{GITHUB_WORKSPACE\}\/tools\/celld-callback-relay\/target"/);
  assert.match(workflow, /export SOURCE_DATE_EPOCH=0/);
  assert.match(workflow, /export CARGO_INCREMENTAL=0/);
  assert.match(workflow, /--remap-path-prefix=\$\{GITHUB_WORKSPACE\}=\/workspace/);
  assert.match(workflow, /--remap-path-prefix=\$\{cargo_home\}=\/cargo-home/);
  const ignoredPaths = new Set(gitignore.split(/\r?\n/));
  for (const path of ["/artifacts/", "/.ci-tmp/", "/.celld-target/", "/tools/celld-callback-relay/target/"]) {
    assert.ok(ignoredPaths.has(path), `${path} must be ignored`);
  }
});

test("Celld qualification fails closed on exact-host capacity and provenance", () => {
  assert.match(workflow, /CELLD_QUALIFICATION_EXPECTED_HOST: titan/);
  assert.match(workflow, /CELLD_QUALIFICATION_EXPECTED_COMMIT: \$\{\{ github\.sha \}\}/);
  assert.match(workflow, /CELLD_QUALIFICATION_MIN_BUILD_FREE_GIB: "400"/);
  assert.match(workflow, /CELLD_QUALIFICATION_MAX_RETAINED_GIB: "10"/);
  assert.match(workflow, /scripts\/celld-titan-preflight\.mjs/);
  assert.match(workflow, /artifacts\/celld-titan-preflight\.json/);
  assert.match(workflow, /scripts\/celld-titan-postflight\.mjs/);
  assert.match(workflow, /artifacts\/celld-titan-postflight\.json/);
});

test("Celld qualification previews destructive cleanup and always verifies it", () => {
  const firstDryRun = workflow.indexOf("--dry-run");
  const firstMutation = workflow.indexOf("Reap stale disposable E2E resources");
  assert.ok(firstDryRun > 0 && firstDryRun < firstMutation);
  assert.match(workflow, /scripts\/reap-e2e-vms\.sh/g);
  assert.match(workflow, /Verify qualification VM cleanup/);
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /exit 4/);
  assert.ok(workflow.indexOf("Remove job-scoped compiler artifacts") < workflow.indexOf("Verify post-run resource and capacity baseline"));
});

test("Celld qualification uploads authoritative evidence without claiming operator UAT", () => {
  assert.match(workflow, /node scripts\/run-celld-uat\.mjs/);
  assert.match(workflow, /node scripts\/celld-qualification-lanes\.mjs/);
  assert.match(workflow, /selected_ids="\$\{CELLD_QUALIFICATION_SELECTED_IDS\}"/);
  assert.match(workflow, /CELLD_QUALIFICATION_EXPECTED_COUNT/);
  assert.match(workflow, /--id "\$\{selected_ids\}"/);
  assert.doesNotMatch(workflow, /--trigger automated/);
  assert.match(workflow, /actions\/upload-artifact@c6a366c94c3e0affe28c06c8df20a878f24da3cf/);
  assert.match(workflow, /sha256sum --check manifest\.sha256/);
  assert.match(workflow, /Report bounded qualification verdict/);
  assert.match(workflow, /stdout_tail: \.command\.stdout_tail/);
  assert.doesNotMatch(workflow, /Upload qualification evidence[\s\S]{0,100}continue-on-error: true/);
  assert.match(workflow, /tests\/celld\/uat\/results\/titan-/);
  assert.doesNotMatch(workflow, /make test-celld-soak/);
  assert.doesNotMatch(workflow, /make test-celld-human-uat/);
});

test("Celld qualification builds and enables the fixed live orchestration driver", () => {
  assert.match(workflow, /cargo build --locked --release --manifest-path management\/Cargo\.toml/);
  assert.match(workflow, /cargo build --locked --release --manifest-path agent-rs\/Cargo\.toml/);
  assert.match(workflow, /x86_64-unknown-linux-musl/);
  assert.match(workflow, /images\/container\/Dockerfile\.base/);
  assert.match(workflow, /CELLD_ORCHESTRATION_DOCKER_IMAGE_REF/);
  assert.match(workflow, /celld-live-orchestration\.mjs prepare/);
  assert.match(workflow, /credential_provenance_config="\$\{fixture_root\}\/credential-provenance\.json"/);
  assert.doesNotMatch(workflow, /credential_provenance_config="\$\{orchestration_root\}\/credential-provenance\.json"/);
  assert.match(workflow, /orchestration_inventory="\$\{orchestration_root\}\/orchestration-inventory\.json"/);
  assert.match(workflow, /--arg inventory "\$\{orchestration_inventory\}"/);
  assert.match(workflow, /"celld-live-orchestration": \{enabled: \$orchestration_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /"celld-live-worker": \{enabled: \$worker_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /"celld-live-network-auth": \{enabled: \$network_auth_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /--arg credential_provenance_config "\$\{credential_provenance_config\}"/);
  assert.match(workflow, /"celld-live-credential-provenance": \{enabled: \$credential_provenance_enabled, config_path: \$credential_provenance_config\}/);
  assert.doesNotMatch(workflow, /env\.CELLD_CREDENTIAL_PROVENANCE_CONFIG/);
  assert.match(workflow, /agentic-sandbox\.celld-live-credential-provenance\/v1/);
  assert.match(workflow, /"celld-live-rollout": \{enabled: \$rollout_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /"celld-live-observability": \{enabled: \$observability_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /"celld-live-recovery": \{enabled: \$recovery_enabled, config_path: \$orchestration_config\}/);
  assert.match(workflow, /celld-live-worker\.mjs cleanup/);
  assert.match(workflow, /celld-live-network-auth\.mjs cleanup/);
  assert.match(workflow, /celld-live-orchestration\.mjs cleanup/);
});

test("Celld qualification recovers before UAT and attempts every scoped cleaner while preserving residue exit 4", () => {
  const recovery = workflow.indexOf("celld-live-orchestration.mjs recover");
  const catalog = workflow.indexOf("Execute automated Celld qualification catalog");
  assert.ok(recovery > 0 && recovery < catalog, "startup recovery must precede the UAT catalog");

  const cleanup = exactWorkflowStep(workflow, "Reap exact orchestration controller state");
  assert.match(cleanup.body, /cleanup_rc=0/);
  assert.match(cleanup.body, /node scripts\/celld-live-worker\.mjs cleanup[^\n]*\|\| cleanup_rc=4/);
  assert.match(cleanup.body, /node scripts\/celld-live-network-auth\.mjs cleanup[^\n]*\|\| cleanup_rc=4/);
  assert.match(cleanup.body, /node scripts\/celld-live-orchestration\.mjs cleanup[^\n]*\|\| cleanup_rc=4/);
  assert.match(cleanup.body, /rmdir \/dev\/shm\/agentic-celld-orchestration 2>\/dev\/null \|\| true/);
  assert.match(cleanup.body, /exit "\$\{cleanup_rc\}"/);
});

test("Celld qualification discovers a retained same-run inventory before preparation and never adopts another run", () => {
  const discovery = exactWorkflowStep(workflow, "Discover and recover retained same-run orchestration inventory");
  const prepare = exactWorkflowStep(workflow, "Prepare pinned storage candidate for UAT-010");
  const catalog = exactWorkflowStep(workflow, "Execute automated Celld qualification catalog");
  assert.ok(discovery.start < prepare.start && prepare.start < catalog.start);
  assert.match(discovery.body, /celld-live-orchestration\.mjs recover-retained/);
  assert.match(discovery.body, /--run-id "titan-\$\{GITHUB_RUN_ID\}"/);
  assert.match(discovery.body, /--orchestration-root \/dev\/shm\/agentic-celld-orchestration/);
  assert.match(discovery.body, /--exact-run-owner "titan-\$\{GITHUB_RUN_ID\}"/);
  assert.doesNotMatch(discovery.body, /find [^\n]*-exec|for [^\n]*agentic-celld-orchestration\/\*/);
});

test("Celld qualification rehearses an exact two-store offline migration before UAT", () => {
  const prepare = workflow.indexOf("Prepare pinned storage candidate for UAT-010");
  const migration = workflow.indexOf("Qualify destination and rehearse verified offline object-store migration");
  const catalog = workflow.indexOf("Execute automated Celld qualification catalog");
  assert.ok(prepare > 0 && prepare < migration && migration < catalog);
  assert.match(workflow, /migration_destination_root="\/dev\/shm\/agentic-celld-migration\/\$\{migration_destination_run_id\}"/);
  assert.match(workflow, /node scripts\/celld-live-offline-migration\.mjs/);
  assert.match(workflow, /CELLD_QUALIFICATION_RUN_MIGRATION/);
  assert.match(workflow, /--source-config "\$\{CELLD_STORAGE_FIXTURE_CONFIG\}"/);
  assert.match(workflow, /--destination-run-id "\$\{CELLD_MIGRATION_DESTINATION_RUN_ID\}"/);
  assert.match(workflow, /celld-offline-migration-destination-qualification\.jsonl/);
  assert.match(workflow, /sha256sum --check artifacts\/celld-offline-migration-manifest\.sha256/);
  assert.match(workflow, /Reap exact offline-migration fixtures\n\s+if: always\(\)/);
  assert.equal((workflow.match(/\.retain_namespaces == true/g) ?? []).length, 2);
  assert.match(workflow, /retaining stopped, write-denied migration namespaces for operator recovery/);
  assert.ok(workflow.indexOf("Reap exact offline-migration fixtures") < workflow.indexOf("Reap exact storage fixture"));
  assert.match(workflow, /artifacts\/celld-offline-migration\*/);
});

test("Wave 3 issue selections cannot be presented as the complete qualification", () => {
  const resolver = exactWorkflowStep(workflow, "Resolve strict qualification lane");
  const prepare = exactWorkflowStep(workflow, "Prepare pinned storage candidate for UAT-010");
  assert.ok(resolver.start < prepare.start);
  assert.match(resolver.body, /--format github-env >> "\$\{GITHUB_ENV\}"/);
  assert.match(workflow, /CELLD_QUALIFICATION_RUN_CATALOG/);
  assert.match(workflow, /Skipping UAT catalog for the offline-migration-only lane/);
  assert.doesNotMatch(workflow, /qualification_lane:[\s\S]{0,500}?issue-766/);
});

test("destructive qualification needs both workflow opt-in and exact run ownership", () => {
  assert.match(workflow, /CELLD_QUALIFICATION_ALLOW_DESTRUCTIVE_FAULTS: \$\{\{ inputs\.allow_destructive_faults \}\}/);
  assert.match(workflow, /--argjson destructive_faults "\$\{destructive_faults\}"/);
  assert.match(workflow, /authorization: \{destructive_faults: \$destructive_faults, inventory_path: \$inventory, exact_run_owner: \$run_id\}/);
});

test("three-node fleet fixture is manual, exact-pinned, janitored, and cleanup-fail-closed", () => {
  const preflight = exactWorkflowStep(fleetWorkflow, "Fail-closed Titan capacity and provenance preflight");
  const focusedGate = exactWorkflowStep(fleetWorkflow, "Validate focused fleet lifecycle contracts");
  const firstMutation = exactWorkflowStep(fleetWorkflow, "Prepare pinned storage and fleet inventories");

  assert.match(fleetWorkflow, /on:\n  workflow_dispatch:/);
  assert.match(fleetWorkflow, /celld_channel:[\s\S]*?default: approved[\s\S]*?- reviewed-candidate/);
  assert.match(fleetWorkflow, /CELLD_FLEET_CHANNEL: \$\{\{ inputs\.celld_channel \}\}/);
  assert.match(fleetWorkflow, /runs-on: titan/);
  assert.match(fleetWorkflow, /concurrency:\n  # [\s\S]*?group: agentic-sandbox-celld-qualification-titan/);
  assert.match(fleetWorkflow, /fleet-fixture:\n[\s\S]*?concurrency:\n      group: agentic-sandbox-vm-e2e/);
  assert.ok(preflight.start < focusedGate.start && focusedGate.start < firstMutation.start);
  assert.equal(preflight.body, [
    "      - name: Fail-closed Titan capacity and provenance preflight",
    "        run: |",
    "          set -euo pipefail",
    "          node scripts/celld-titan-preflight.mjs \\",
    "            --output artifacts/celld-fleet-titan-preflight.json",
    "",
  ].join("\n"));
  assert.equal(focusedGate.body, [
    "      - name: Validate focused fleet lifecycle contracts",
    "        run: |",
    "          set -euo pipefail",
    "          node --test --test-concurrency=10 \\",
    "            tests/celld/uat/fleet-fixture.test.mjs \\",
    "            tests/celld/uat/seaweedfs-fixture.test.mjs \\",
    "            tests/celld/uat/titan-preflight.test.mjs \\",
    "            tests/celld/uat/titan-workflow.test.mjs",
    "          cargo test --locked \\",
    "            --manifest-path tools/celld-callback-relay/Cargo.toml",
    "          node scripts/celld-uat-contract-check.mjs",
    "",
  ].join("\n"));
  assert.match(fleetWorkflow, /celld-seaweedfs-fixture\.mjs start/);
  assert.match(fleetWorkflow, /celld-rollout-candidate\.mjs check/);
  assert.match(fleetWorkflow, /gh attestation verify/);
  assert.match(fleetWorkflow, /--bundle-from-oci/);
  assert.match(fleetWorkflow, /docker buildx imagetools inspect/);
  assert.match(fleetWorkflow, /qualification_status: "reviewed_unqualified"/);
  assert.match(fleetWorkflow, /source_commit: \$commit/);
  assert.match(fleetWorkflow, /runner_environment: "github-hosted"/);
  assert.match(fleetWorkflow, /--celld-channel "\$\{CELLD_FLEET_CHANNEL\}"/);
  assert.match(fleetWorkflow, /npm ci --no-audit --no-fund --prefix runtimes\/celld\/instance-cell/);
  assert.match(fleetWorkflow, /--bins/);
  assert.match(fleetWorkflow, /agentic-celld-credential-launcher/);
  assert.equal((fleetWorkflow.match(/--credential-launcher "\$\{CELLD_CREDENTIAL_LAUNCHER_BINARY\}"/g) ?? []).length, 2);
  assert.match(fleetWorkflow, /CELLD_CREDENTIAL_LAUNCHER_SHA256/);
  assert.match(fleetWorkflow, /\.credential_launcher_sha256 == \$expected_launcher/);
  assert.match(fleetWorkflow, /celld-fleet-fixture\.mjs deploy/);
  assert.match(fleetWorkflow, /artifacts\/celld-worker-deployment\.json/);
  assert.match(fleetWorkflow, /\.worker_digest == \$expected_worker/);
  assert.match(fleetWorkflow, /\.celld_manifest_digest == \$expected_celld/);
  assert.match(fleetWorkflow, /celld-fleet-fixture\.mjs start/);
  assert.match(fleetWorkflow, /celld-fleet-fixture\.mjs cleanup/);
  assert.match(fleetWorkflow, /celld-storage-startup-diagnostics\.json/);
  assert.match(fleetWorkflow, /celld-seaweedfs-fixture\.mjs cleanup/);
  assert.equal((fleetWorkflow.match(/celld-fleet-fixture\.mjs janitor-preview/g) ?? []).length, 2);
  assert.match(fleetWorkflow, /\.scope == "single-host multi-node"/);
  assert.match(fleetWorkflow, /\.membership\.running == 3/);
  assert.match(fleetWorkflow, /\.membership\.probe == "passed"/);
  assert.match(fleetWorkflow, /\.pins\.celld_channel == \$channel/);
  assert.match(fleetWorkflow, /artifacts\/celld-rollout-candidate-provenance\.json/);
  assert.match(fleetWorkflow, /exit 4/);
  assert.doesNotMatch(fleetWorkflow, /docker (?:system|image|network|volume) prune/);
  assert.doesNotMatch(fleetWorkflow, /test-celld-(?:soak|human-uat)/);
});
