import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, closeSync, existsSync, fsyncSync, linkSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, renameSync, rmdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { basename, dirname, join } from "node:path";
import test from "node:test";

import {
  acquireOrchestrationInventoryLifecycle,
  canonicalOrchestrationJson,
  commitOrchestrationInventory,
  createOrchestrationInventoryV2,
  finishOrchestrationMutation,
  loadProtectedOrchestrationInventory,
  planOrchestrationMutation,
  releaseOrchestrationInventoryLifecycle,
  validateOrchestrationInventoryDocument,
} from "../../../scripts/celld-orchestration-inventory.mjs";
import * as liveOrchestration from "../../../scripts/celld-live-orchestration.mjs";

const REPO_ROOT = new URL("../../../", import.meta.url).pathname.replace(/\/$/, "");
const DRIVER = join(REPO_ROOT, "scripts/celld-live-orchestration.mjs");
const CAS_WRITER = join(REPO_ROOT, "tests/celld/uat/fixtures/orchestration-cas-writer.mjs");
const CLEANUP_WORKER = join(REPO_ROOT, "tests/celld/uat/fixtures/orchestration-cleanup-worker.mjs");
const PENDING_LOCK_HOLDER = join(REPO_ROOT, "tests/celld/uat/fixtures/orchestration-pending-lock-holder.mjs");
const PROVIDER_IDENTITY = "c".repeat(64);
const PROVIDER_CONFIGURATION = "d".repeat(64);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function fullConfig(runId, root) {
  return {
    schema_version: "agentic-sandbox.celld-live-orchestration/v1",
    run_id: runId,
    working_root: root,
    inventory_path: join(root, "orchestration-inventory.json"),
    management_binary_path: join(REPO_ROOT, "management/target/release/agentic-mgmt"),
    agent_client_binary_path: join(REPO_ROOT, "management/target/release/agent-client"),
    callback_relay_binary_path: join(REPO_ROOT, "tools/celld-callback-relay/target/x86_64-unknown-linux-musl/release/agentic-celld-callback-relay"),
    qemu_cleanup_helper_path: "/usr/libexec/agentic-sandbox/agentic-celld-qemu-cleanup-helper",
    qemu_cleanup_helper_sha256: "e".repeat(64),
    docker_image_ref: `sha256:${"a".repeat(64)}`,
    base_images_dir: "/build/agentic-sandbox/base-images",
    vm_storage_dir: "/build/agentic-sandbox/vms",
    agentshare_root: "/var/tmp/agentic-celld-qualification-123/mount",
    libvirt_uri: "qemu:///system",
    management_grpc_port: 38120,
  };
}

function exactRunFixture(prefix) {
  const parent = "/dev/shm/agentic-celld-orchestration";
  const removeParent = !existsSync(parent);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  const root = mkdtempSync(join(parent, `${prefix}-`));
  const runId = basename(root);
  const config = fullConfig(runId, root);
  const inventory = createOrchestrationInventoryV2({
    runId,
    workingRoot: root,
    hostSha256: sha256(hostname()),
    now: new Date("2026-08-23T00:00:00.000Z"),
  });
  return {
    root,
    runId,
    config,
    inventory,
    cleanup() {
      rmSync(root, { recursive: true, force: true });
      if (removeParent) rmdirSync(parent);
    },
  };
}

function providerSubject({ instanceId = "123e4567-e89b-42d3-a456-426614174000", name = "celld-owned-provider", substrate = "docker", action = "provision", operationId = `operation-${action}`, generation = 1 } = {}) {
  return {
    instance_id: instanceId,
    name,
    substrate,
    operation_id: operationId,
    generation,
    action,
    request_sha256: "a".repeat(64),
  };
}

function rehashLastEntry(inventory) {
  const entry = inventory.journal.at(-1);
  const { entry_sha256: _discarded, ...hashed } = entry;
  entry.entry_sha256 = sha256(canonicalOrchestrationJson(hashed));
  inventory.journal_head_sha256 = entry.entry_sha256;
}

function addObservedProvider(inventory, { scenarioId = "UAT-CELLD-003", subject = providerSubject(), mutationId } = {}) {
  const planned = planOrchestrationMutation(inventory, {
    mutation: "provider_action",
    scenarioId,
    subjectType: "provider_resource",
    subject,
    ...(mutationId ? { mutationId } : {}),
  }, new Date("2026-08-23T00:00:01.000Z"));
  const providerBindings = subject.substrate === "docker" ? {
    observedProviderId: "1".repeat(64),
    observedOwnershipBindingSha256: "2".repeat(64),
    observedManagedNetworkId: "3".repeat(64),
    observedManagedNetworkIdentitySha256: "4".repeat(64),
    observedManagedNetworkConfigurationSha256: "5".repeat(64),
  } : {
    observedProviderId: "11111111-2222-4333-8444-555555555555",
    observedOwnershipBindingSha256: "2".repeat(64),
    observedProviderStorageIdentitySha256: "3".repeat(64),
    observedStoragePath: `/build/agentic-sandbox/vms/${subject.name}`,
    observedStorageDevice: "8",
    observedStorageInode: "9",
    observedStorageUid: String(typeof process.getuid === "function" ? process.getuid() : 1000),
    observedStorageGid: String(typeof process.getgid === "function" ? process.getgid() : 1000),
  };
  const completed = finishOrchestrationMutation(inventory, planned.entry, {
    outcome: "effect_observed",
    observedIdentitySha256: PROVIDER_IDENTITY,
    observedConfigurationSha256: PROVIDER_CONFIGURATION,
    ...providerBindings,
  }, new Date("2026-08-23T00:00:02.000Z"));
  return { subject, resource: completed.materialized };
}

function addObservedQemuProvider(inventory, storagePath, {
  instanceId = "123e4567-e89b-42d3-a456-426614174000",
  name = "celld-owned-provider",
  providerId = "11111111-2222-4333-8444-555555555555",
} = {}) {
  const storage = lstatSync(storagePath);
  const subject = providerSubject({ instanceId, name, substrate: "qemu" });
  const planned = planOrchestrationMutation(inventory, {
    mutation: "provider_action",
    scenarioId: "UAT-CELLD-003",
    subjectType: "provider_resource",
    subject,
  }, new Date("2026-08-23T00:00:01.000Z"));
  const completed = finishOrchestrationMutation(inventory, planned.entry, {
    outcome: "effect_observed",
    observedProviderId: providerId,
    observedIdentitySha256: sha256(`qemu:${providerId}`),
    observedConfigurationSha256: "6".repeat(64),
    observedOwnershipBindingSha256: "7".repeat(64),
    observedProviderStorageIdentitySha256: sha256(canonicalOrchestrationJson({
      path: storagePath,
      device: String(storage.dev),
      inode: String(storage.ino),
      uid: storage.uid,
      gid: storage.gid,
    })),
    observedStoragePath: storagePath,
    observedStorageDevice: String(storage.dev),
    observedStorageInode: String(storage.ino),
    observedStorageUid: String(storage.uid),
    observedStorageGid: String(storage.gid),
  }, new Date("2026-08-23T00:00:02.000Z"));
  return { subject, resource: completed.materialized };
}

function addPendingCleanup(inventory, resource) {
  return planOrchestrationMutation(inventory, {
    mutation: "provider_cleanup",
    scenarioId: resource.scenario_id,
    subjectType: "provider_resource",
    subject: providerSubject({
      instanceId: resource.instance_id,
      name: resource.name,
      substrate: resource.substrate,
      action: "cleanup",
      operationId: `pending-cleanup-${resource.instance_id}`,
    }),
  }, new Date("2026-08-23T00:00:03.000Z"));
}

function appendLegacyNestedCleanup(inventory, resource) {
  const recordedAt = "2026-08-23T00:00:01.000Z";
  const entry = {
    sequence: inventory.last_sequence + 1,
    mutation_id: "123e4567-e89b-42d3-a456-426614174222",
    event: "planned",
    mutation: "provider_cleanup",
    scenario_id: resource.scenario_id,
    subject_type: "provider_resource",
    subject: providerSubject({
      instanceId: resource.instance_id,
      name: resource.name,
      substrate: resource.substrate,
      action: "cleanup",
      operationId: "legacy-nested-cleanup",
    }),
    recorded_at: recordedAt,
    previous_entry_sha256: inventory.journal_head_sha256,
  };
  entry.entry_sha256 = sha256(canonicalOrchestrationJson(entry));
  inventory.journal.push(entry);
  inventory.last_sequence = entry.sequence;
  inventory.journal_head_sha256 = entry.entry_sha256;
  inventory.incomplete_mutation_ids.push(entry.mutation_id);
  inventory.updated_at = recordedAt;
  inventory.state = "active";
  resource.status = "cleanup_pending";
  resource.last_sequence = entry.sequence;
  resource.updated_at = recordedAt;
  return entry;
}

function appendLegacyUnboundFaultPlan(inventory) {
  const recordedAt = "2026-08-23T00:00:01.000Z";
  const subject = { fault_id: "e".repeat(32), kind: "callback_relay_pause", target: "celld-owned-relay" };
  const entry = {
    sequence: inventory.last_sequence + 1,
    mutation_id: "123e4567-e89b-42d3-a456-426614174333",
    event: "planned",
    mutation: "fault_apply",
    scenario_id: "UAT-CELLD-006",
    subject_type: "fault",
    subject,
    recorded_at: recordedAt,
    previous_entry_sha256: inventory.journal_head_sha256,
  };
  entry.entry_sha256 = sha256(canonicalOrchestrationJson(entry));
  inventory.journal.push(entry);
  inventory.last_sequence = entry.sequence;
  inventory.journal_head_sha256 = entry.entry_sha256;
  inventory.incomplete_mutation_ids.push(entry.mutation_id);
  inventory.updated_at = recordedAt;
  inventory.state = "active";
  inventory.faults.push({
    id: subject.fault_id,
    scenario_id: entry.scenario_id,
    kind: subject.kind,
    target: subject.target,
    status: "planned",
    last_sequence: entry.sequence,
    planned_at: recordedAt,
    updated_at: recordedAt,
  });
  return entry;
}

function runtimeFor(fixture, resource) {
  return {
    scenarioId: resource.scenario_id,
    runId: fixture.runId,
    config: fixture.config,
    orchestrationInventory: fixture.inventory,
    providerResources: new Map([[resource.instance_id, {
      instanceId: resource.instance_id,
      name: resource.name,
      substrate: resource.substrate,
    }]]),
    persistInventory: () => {},
    qemuCleanupHelper: liveOrchestration.qemuCleanupHelperTestAdapter,
    sendWorkerCommand: async () => { throw new Error("recovery replayed a provider action"); },
  };
}

function durableRuntimeFor(fixture, resource) {
  if (existsSync(fixture.config.inventory_path)) throw new Error("durable runtime fixture inventory already exists");
  commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
  const runtime = runtimeFor(fixture, resource);
  delete runtime.persistInventory;
  runtime.persistedJournalHeadSha256 = fixture.inventory.journal_head_sha256;
  return runtime;
}

function addAppliedFault(inventory, { faultId = "f".repeat(32), kind = "callback_relay_pause", target = "celld-owned-relay" } = {}) {
  const planned = planOrchestrationMutation(inventory, {
    mutation: "fault_apply",
    scenarioId: "UAT-CELLD-006",
    subjectType: "fault",
    subject: {
      fault_id: faultId,
      kind,
      target,
      target_identity_sha256: "7".repeat(64),
      target_ownership_sha256: "8".repeat(64),
    },
  }, new Date("2026-08-23T00:00:01.000Z"));
  return finishOrchestrationMutation(inventory, planned.entry, {
    outcome: "applied",
  }, new Date("2026-08-23T00:00:02.000Z")).materialized;
}

function addPendingFaultApplyAndHeal(inventory, {
  faultId = "b".repeat(32),
  kind = "callback_relay_pause",
  target = "celld-owned-relay",
} = {}) {
  const subject = {
    fault_id: faultId,
    kind,
    target,
    target_identity_sha256: "7".repeat(64),
    target_ownership_sha256: "8".repeat(64),
  };
  const apply = planOrchestrationMutation(inventory, {
    mutation: "fault_apply",
    scenarioId: "UAT-CELLD-006",
    subjectType: "fault",
    subject,
  }, new Date("2026-08-23T00:00:01.000Z"));
  const heal = planOrchestrationMutation(inventory, {
    mutation: "fault_heal",
    scenarioId: "UAT-CELLD-006",
    subjectType: "fault",
    subject,
    allowConflictWithMutationId: apply.entry.mutation_id,
  }, new Date("2026-08-23T00:00:02.000Z"));
  return { apply: apply.entry, heal: heal.entry, fault: heal.materialized };
}

function makeV1Inventory(fixture) {
  return {
    schema_version: "agentic-sandbox.celld-orchestration-inventory/v1",
    run_id: fixture.runId,
    working_root: fixture.root,
    owner: { repository: "roctinam/agentic-sandbox", workflow: "celld-qualification.yml", run_id: fixture.runId },
    host_sha256: fixture.inventory.host_sha256,
    created_at: "2026-08-23T00:00:00.000Z",
    updated_at: "2026-08-23T00:00:00.000Z",
    state: "active",
    resources: [{
      scenario_id: "UAT-CELLD-003",
      instance_id: "123e4567-e89b-42d3-a456-426614174000",
      name: "celld-v1-provider",
      substrate: "docker",
      status: "planned",
      planned_at: "2026-08-23T00:00:00.000Z",
      updated_at: "2026-08-23T00:00:00.000Z",
    }],
    faults: [],
  };
}

function trustedDockerObservation(fixture, {
  instanceId = "123e4567-e89b-42d3-a456-426614174000",
  providerId = "1".repeat(64),
  networkId = "2".repeat(64),
} = {}) {
  const providerLabels = { "agentic-instance-id": instanceId, "agentic-source": "admin-v2", "agentic-run-id": fixture.runId };
  const networkLabels = { "agentic-run-id": fixture.runId, "agentic-instance-id": instanceId };
  return {
    owned: true,
    present: true,
    state: "created",
    provider_storage_present: false,
    provider_id: providerId,
    provider_labels: providerLabels,
    ownership_binding_sha256: sha256(canonicalOrchestrationJson(providerLabels)),
    provider_identity_sha256: sha256(`docker:${providerId}`),
    configuration_sha256: sha256(canonicalOrchestrationJson({ image: fixture.config.docker_image_ref, labels: providerLabels, managed_network_id: networkId })),
    managed_network_id: networkId,
    managed_network_labels: networkLabels,
    managed_network_identity_sha256: sha256(`docker-network:${networkId}`),
    managed_network_configuration_sha256: sha256(canonicalOrchestrationJson(networkLabels)),
  };
}

function persistedDockerObservation(resource, present = true) {
  if (!present) return { owned: true, present: false, state: "absent", provider_storage_present: false, provider_identity_sha256: null, configuration_sha256: null };
  return {
    owned: true,
    present: true,
    state: "created",
    provider_storage_present: false,
    provider_id: resource.provider_id,
    provider_identity_sha256: resource.provider_identity_sha256,
    configuration_sha256: resource.configuration_sha256,
    ownership_binding_sha256: resource.ownership_binding_sha256,
    managed_network_id: resource.managed_network_id,
    managed_network_identity_sha256: resource.managed_network_identity_sha256,
    managed_network_configuration_sha256: resource.managed_network_configuration_sha256,
  };
}

function childCompletion(child) {
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  return new Promise((resolve) => child.on("close", (status, signal) => resolve({ status, signal, stdout, stderr })));
}

function processStartTimeTicksForTest(pid) {
  const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
  return stat.slice(stat.lastIndexOf(")") + 2).trim().split(/\s+/)[19];
}

function stagedLifecycleCandidate(root, { pid, nonce, owner }) {
  const path = join(root, `.orchestration-inventory.lock.pending-${pid}-${nonce}`);
  mkdirSync(path, { mode: 0o700 });
  if (owner !== undefined) {
    const ownerPath = join(path, "owner.json");
    writeFileSync(ownerPath, typeof owner === "string" ? owner : `${JSON.stringify(owner)}\n`, { mode: 0o600 });
    const ownerDescriptor = openSync(ownerPath, "r");
    try { fsyncSync(ownerDescriptor); } finally { closeSync(ownerDescriptor); }
  }
  const candidateDescriptor = openSync(path, "r");
  try { fsyncSync(candidateDescriptor); } finally { closeSync(candidateDescriptor); }
  const rootDescriptor = openSync(root, "r");
  try { fsyncSync(rootDescriptor); } finally { closeSync(rootDescriptor); }
  return path;
}

async function waitForPathOrExit(path, completion, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs;
  while (!existsSync(path)) {
    const completed = await Promise.race([
      completion.then((result) => ({ completed: result })),
      new Promise((resolve) => setTimeout(() => resolve(null), 10)),
    ]);
    if (completed) throw new Error(`CAS writer exited before the compare/replace barrier: ${completed.completed.stderr}`);
    if (Date.now() > deadline) throw new Error("CAS writer did not reach the compare/replace barrier");
  }
}

async function startPendingLockHolder(fixture, nonce) {
  const child = spawn(process.execPath, [PENDING_LOCK_HOLDER, fixture.root, nonce], {
    cwd: REPO_ROOT,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const completion = childCompletion(child);
  const candidate = join(fixture.root, `.orchestration-inventory.lock.pending-${child.pid}-${nonce}`);
  await waitForPathOrExit(candidate, completion);
  return { child, completion, candidate, processStartTimeTicks: processStartTimeTicksForTest(child.pid) };
}

async function stopPendingLockHolder(holder) {
  if (holder.child.exitCode === null && holder.child.signalCode === null) holder.child.kill("SIGTERM");
  return holder.completion;
}

function persistedQemuObservation(record, storagePath, { present = true, state = "shut off" } = {}) {
  if (!present) {
    return {
      owned: true,
      present: false,
      state: "absent",
      provider_storage_present: false,
      provider_identity_sha256: null,
      configuration_sha256: null,
    };
  }
  return {
    owned: true,
    present: true,
    state,
    provider_storage_present: true,
    provider_id: record.provider_id,
    provider_identity_sha256: record.provider_identity_sha256,
    configuration_sha256: record.configuration_sha256,
    ownership_binding_sha256: record.ownership_binding_sha256,
    provider_storage_identity_sha256: record.provider_storage_identity_sha256,
    storage_path: storagePath,
    storage_device: record.storage_device,
    storage_inode: record.storage_inode,
    storage_uid: record.storage_uid,
    storage_gid: record.storage_gid,
    disk_source_paths: [join(storagePath, "disk.qcow2")],
  };
}

function installQemuQuarantineDeletionSubstitution(quarantinePath, authorizedResiduePath) {
  const foreignMarker = join(quarantinePath, "foreign-marker");
  let substituted = false;
  return {
    beforeDelete({ path }) {
      assert.equal(path, quarantinePath, "the race seam is scoped to the exact journal-owned quarantine");
      if (substituted) return;
      substituted = true;
      renameSync(quarantinePath, authorizedResiduePath);
      mkdirSync(quarantinePath, { mode: 0o700 });
      writeFileSync(foreignMarker, "foreign replacement\n", { mode: 0o600 });
    },
    foreignMarker,
    authorizedResiduePath,
    wasSubstituted: () => substituted,
  };
}

function installQemuFinalNameOperationSubstitution({ operation, quarantinePath, deletionRootPath = quarantinePath, storageRoot }) {
  const authorizedResiduePath = join(storageRoot, `authorized-residue-${operation}`);
  const foreignTarget = join(storageRoot, `foreign-target-${operation}`);
  const foreignPath = operation === "quarantine-root"
    ? deletionRootPath
    : join(deletionRootPath, operation === "nested-directory" ? "nested" : "disk.qcow2");
  let substituted = false;
  const substitute = (path) => {
    substituted = true;
    renameSync(path, authorizedResiduePath);
    if (operation === "leaf-file") writeFileSync(path, "foreign replacement\n", { mode: 0o600 });
    else if (operation === "leaf-symlink") {
      writeFileSync(foreignTarget, "foreign target\n", { mode: 0o600 });
      symlinkSync(foreignTarget, path);
    } else mkdirSync(path, { mode: 0o700 });
  };
  return {
    authorizedResiduePath,
    foreignPath,
    wasSubstituted: () => substituted,
    beforeFinalNameOperation({ operation: nameOperation, path }) {
      const matchesLeaf = ["leaf-file", "leaf-symlink"].includes(operation)
        && nameOperation === "unlink"
        && path.startsWith("/proc/self/fd/")
        && path.endsWith("/disk.qcow2");
      const matchesNested = operation === "nested-directory"
        && nameOperation === "rmdir"
        && path.startsWith("/proc/self/fd/")
        && path.endsWith("/nested");
      const matchesRoot = operation === "quarantine-root"
        && nameOperation === "rmdir"
        && path === deletionRootPath;
      if (!substituted && (matchesLeaf || matchesNested || matchesRoot)) substitute(path);
    },
  };
}

function expectedQemuFinalCapturePath(cleanupPlan) {
  return cleanupPlan.subject.storage_final_capture_path;
}

function physicalQemuTestCapturePath(cleanupPlan) {
  return `${cleanupPlan.subject.storage_quarantine_path}.final`;
}

function stablePathForOpenDescriptorPath(path) {
  if (!path.startsWith("/proc/self/fd/")) return path;
  return join(realpathSync(dirname(path)), basename(path));
}

function installQemuPostCaptureSubstitution({ operation, quarantinePath, deletionRootPath = quarantinePath, storageRoot }) {
  const expectedOriginalPath = operation === "quarantine-root"
    ? deletionRootPath
    : join(deletionRootPath, operation === "nested-directory" ? "nested" : "disk.qcow2");
  const foreignTarget = join(storageRoot, `post-capture-foreign-target-${operation}`);
  let substitution = null;

  function matches({ operation: nameOperation, originalPath }) {
    if (operation === "quarantine-root") return nameOperation === "rmdir" && originalPath === deletionRootPath;
    if (operation === "nested-directory") {
      return nameOperation === "rmdir"
        && originalPath.startsWith("/proc/self/fd/")
        && originalPath.endsWith("/nested");
    }
    return nameOperation === "unlink"
      && originalPath.startsWith("/proc/self/fd/")
      && originalPath.endsWith("/disk.qcow2");
  }

  return {
    wasSubstituted: () => substitution !== null,
    get originalPath() { return substitution?.originalPath ?? expectedOriginalPath; },
    get capturedPath() { return substitution?.capturedPath ?? null; },
    get foreignPath() { return substitution?.foreignPath ?? expectedOriginalPath; },
    get authorizedResiduePath() { return substitution?.authorizedResiduePath ?? null; },
    get authorizedMetadata() { return substitution?.authorizedMetadata ?? null; },
    beforeCapturedPathDeletion(context) {
      if (substitution !== null || !matches(context)) return;
      const { operation: nameOperation, originalPath, capturedPath, device, inode } = context;
      assert.notEqual(capturedPath, originalPath, "the final deletion seam must expose the actual post-capture pathname");
      const stableOriginalPath = stablePathForOpenDescriptorPath(originalPath);
      const stableCapturedPath = stablePathForOpenDescriptorPath(capturedPath);
      const authorizedMetadata = lstatSync(capturedPath);
      assert.equal(String(authorizedMetadata.dev), device);
      assert.equal(String(authorizedMetadata.ino), inode);
      const authorizedResiduePath = join(
        dirname(stableCapturedPath),
        `.celld-qemu-authorized-residue-${operation}`,
      );
      renameSync(capturedPath, authorizedResiduePath);
      if (operation === "leaf-file") writeFileSync(capturedPath, "foreign post-capture file\n", { mode: 0o600 });
      else if (operation === "leaf-symlink") {
        writeFileSync(foreignTarget, "foreign post-capture symlink target\n", { mode: 0o600 });
        symlinkSync(foreignTarget, capturedPath);
      } else {
        mkdirSync(capturedPath, { mode: 0o700 });
        writeFileSync(join(capturedPath, "foreign-marker"), "foreign post-capture directory\n", { mode: 0o600 });
      }
      substitution = {
        nameOperation,
        originalPath,
        capturedPath,
        foreignPath: stableOriginalPath,
        authorizedResiduePath,
        authorizedMetadata,
      };
    },
  };
}

function assertProtectedDirectoryForDeletion(path, description) {
  const metadata = lstatSync(path);
  assert.equal(metadata.isDirectory(), true, `${description} must be a directory`);
  assert.equal(metadata.isSymbolicLink(), false, `${description} must not be a symlink`);
  assert.equal(metadata.mode & 0o777, 0o700, `${description} must be owner-only before destructive cleanup`);
  if (typeof process.getuid === "function") assert.equal(metadata.uid, process.getuid(), `${description} must be owned by the controller uid`);
  if (typeof process.getgid === "function") assert.equal(metadata.gid, process.getgid(), `${description} must be owned by the controller gid`);
}

function assertPostCaptureSubstitutionResidue(race) {
  assert.equal(race.wasSubstituted(), true, "the attacker must reach the exact post-capture/pre-delete boundary");
  const authorized = lstatSync(race.authorizedResiduePath);
  assert.equal(authorized.dev, race.authorizedMetadata.dev, "the moved authorized inode device remains stable");
  assert.equal(authorized.ino, race.authorizedMetadata.ino, "the moved authorized inode remains explicit residue");
  const foreign = lstatSync(race.foreignPath);
  if (race.foreignPath.endsWith("disk.qcow2") || race.originalPath.endsWith("disk.qcow2")) {
    assert.equal(foreign.isFile() || foreign.isSymbolicLink(), true, "the foreign leaf is restored at its original name");
  } else {
    assert.equal(foreign.isDirectory(), true, "the foreign directory is restored at its original name");
  }
}

function qemuPostCaptureHarness(prefix, operation, mode) {
  const fixture = exactRunFixture(prefix);
  const previousPath = process.env.PATH;
  const fakeBin = join(fixture.root, "fake-bin");
  const storageRoot = join(fixture.root, "vm-storage");
  const storagePath = join(storageRoot, "celld-owned-provider");
  fixture.config.vm_storage_dir = storageRoot;
  mkdirSync(fakeBin, { mode: 0o700 });
  mkdirSync(storageRoot, { mode: 0o700 });
  mkdirSync(storagePath, { mode: 0o700 });
  writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
  if (operation === "leaf-symlink") {
    rmSync(join(storagePath, "disk.qcow2"));
    const target = join(storagePath, "authorized-target");
    writeFileSync(target, "authorized symlink target\n", { mode: 0o600 });
    symlinkSync(target, join(storagePath, "disk.qcow2"));
  }
  if (operation === "nested-directory") {
    mkdirSync(join(storagePath, "nested"), { mode: 0o700 });
    writeFileSync(join(storagePath, "nested", "inner.bin"), "nested authorized storage\n", { mode: 0o600 });
  }
  writeFileSync(join(fakeBin, "virsh"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  chmodSync(join(fakeBin, "virsh"), 0o700);
  process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;

  const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
  const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
  const runtime = durableRuntimeFor(fixture, resource);
  const observation = persistedQemuObservation(resource, storagePath);
  const quarantinePath = cleanupPlan.subject.storage_quarantine_path;
  const deletionRootPath = physicalQemuTestCapturePath(cleanupPlan);
  if (mode === "recovery") renameSync(storagePath, quarantinePath);
  const race = installQemuPostCaptureSubstitution({ operation, quarantinePath, deletionRootPath, storageRoot });
  let observeCalls = 0;

  function dependencies(hook) {
    return {
      ...(hook ? { beforeQemuCapturedPathDeletion: hook } : {}),
      observeProviderResource: async () => {
        observeCalls += 1;
        return observeCalls === 1 && mode === "normal"
          ? observation
          : persistedQemuObservation(resource, storagePath, { present: false });
      },
      removeProviderResource: async ({ plan, record, resource: ownedResource, observation: authorized }) => liveOrchestration.removeExactlyObservedProvider(
        runtime,
        ownedResource,
        record,
        authorized,
        {
          plan,
          observeProviderEffectTarget: async () => observation,
          ...(hook ? { beforeQemuCapturedPathDeletion: hook } : {}),
        },
      ),
    };
  }

  return {
    fixture,
    runtime,
    resource,
    cleanupPlan,
    race,
    storageRoot,
    storagePath,
    quarantinePath,
    async runPass(withRace) {
      return mode === "normal"
        ? liveOrchestration.cleanupOwnedProviderResources(runtime, dependencies(withRace ? race.beforeCapturedPathDeletion : undefined))
        : liveOrchestration.recoverOrchestrationInventory(runtime, dependencies(withRace ? race.beforeCapturedPathDeletion : undefined));
    },
    async runPassWithHook(hook) {
      return mode === "normal"
        ? liveOrchestration.cleanupOwnedProviderResources(runtime, dependencies(hook))
        : liveOrchestration.recoverOrchestrationInventory(runtime, dependencies(hook));
    },
    cleanup() {
      if (previousPath === undefined) delete process.env.PATH;
      else process.env.PATH = previousPath;
      fixture.cleanup();
    },
  };
}

test("fixed golden journals reject semantically invalid provider terminal outcomes", () => {
  const golden = JSON.parse(readFileSync(new URL("./fixtures/orchestration-invalid-semantics-v2.json", import.meta.url), "utf8"));
  const config = {
    run_id: golden.header.run_id,
    working_root: golden.header.working_root,
    inventory_path: `${golden.header.working_root}/orchestration-inventory.json`,
  };
  for (const vector of golden.vectors) {
    const inventory = {
      ...structuredClone(golden.header),
      journal_head_sha256: vector.journal_head_sha256,
      journal: [structuredClone(golden.plan), structuredClone(vector.terminal)],
    };
    const errors = validateOrchestrationInventoryDocument(inventory, config);
    assert.match(errors.join("; "), /outcome|semantic/, vector.name);
  }
});

test("semantically invalid terminal outcomes are rejected before mutating the in-memory inventory", () => {
  const fixture = exactRunFixture("red-terminal-transaction");
  try {
    const planned = planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action",
      scenarioId: "UAT-CELLD-003",
      subjectType: "provider_resource",
      subject: providerSubject(),
    });
    const before = structuredClone(fixture.inventory);
    assert.throws(
      () => finishOrchestrationMutation(fixture.inventory, planned.entry, { outcome: "applied" }),
      /semantic|outcome/,
    );
    assert.deepEqual(fixture.inventory, before);
  } finally {
    fixture.cleanup();
  }
});

test("journal commit compare-and-swap rejects a stale head", () => {
  const fixture = exactRunFixture("red-cas");
  try {
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    const first = structuredClone(fixture.inventory);
    const stale = structuredClone(fixture.inventory);
    planOrchestrationMutation(first, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject(),
    });
    commitOrchestrationInventory(fixture.config.inventory_path, first, {
      config: fixture.config,
      expectedJournalHeadSha256: null,
    });
    planOrchestrationMutation(stale, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject({ name: "celld-stale-provider" }),
    });
    assert.throws(
      () => commitOrchestrationInventory(fixture.config.inventory_path, stale, {
        config: fixture.config,
        expectedJournalHeadSha256: null,
      }),
      /stale|head|compare-and-swap/,
    );
  } finally {
    fixture.cleanup();
  }
});

test("two processes serialize compare-through-replace so exactly one journal writer wins", async () => {
  const fixture = exactRunFixture("red-cas-process");
  const readyPath = join(fixture.root, "writer-one.ready");
  const releasePath = join(fixture.root, "writer-one.release");
  const secondLoadedPath = join(fixture.root, "writer-two.loaded");
  let first;
  let second;
  try {
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    const spawnWriter = (instanceId, loadedPath) => spawn(process.execPath, [
      CAS_WRITER,
      JSON.stringify(fixture.config),
      instanceId,
      readyPath,
      releasePath,
      ...(loadedPath ? [loadedPath] : []),
    ], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });

    first = spawnWriter("123e4567-e89b-42d3-a456-426614174010");
    const firstCompletion = childCompletion(first);
    await waitForPathOrExit(readyPath, firstCompletion);
    second = spawnWriter("123e4567-e89b-42d3-a456-426614174011", secondLoadedPath);
    const secondCompletion = childCompletion(second);
    await waitForPathOrExit(secondLoadedPath, secondCompletion);
    writeFileSync(releasePath, "release\n", { flag: "wx", mode: 0o600 });

    const results = await Promise.all([firstCompletion, secondCompletion]);
    assert.deepEqual(results.map((result) => result.status).sort((left, right) => left - right), [0, 10], results.map((result) => result.stderr).join("; "));
    const committed = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config);
    assert.equal(committed.journal.length, 1, "a concurrent stale writer must not overwrite the winner");
  } finally {
    if (!existsSync(releasePath)) writeFileSync(releasePath, "release\n", { mode: 0o600 });
    if (first?.exitCode === null) first.kill("SIGTERM");
    if (second?.exitCode === null) second.kill("SIGTERM");
    fixture.cleanup();
  }
});

test("a resource cannot acquire a concurrent mutation while its prior plan is incomplete", () => {
  const fixture = exactRunFixture("red-serialize");
  try {
    const subject = providerSubject();
    planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject,
    });
    assert.throws(
      () => planOrchestrationMutation(fixture.inventory, {
        mutation: "provider_action",
        scenarioId: "UAT-CELLD-003",
        subjectType: "provider_resource",
        subject: providerSubject({ action: "start", operationId: "concurrent-start" }),
      }),
      /incomplete|serial|pending/,
    );
  } finally {
    fixture.cleanup();
  }
});

test("run-root substitution after authorization cannot redirect the protected inventory read", () => {
  const fixture = exactRunFixture("red-path-race");
  const displaced = `${fixture.root}-authorized`;
  let hookCalls = 0;
  try {
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    const loaded = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config, {
      expectedHostSha256: fixture.inventory.host_sha256,
      beforeInventoryOpen: () => {
        hookCalls += 1;
        renameSync(fixture.root, displaced);
        mkdirSync(fixture.root, { mode: 0o700 });
        const substituted = { ...fixture.inventory, host_sha256: "f".repeat(64) };
        writeFileSync(fixture.config.inventory_path, `${JSON.stringify(substituted)}\n`, { mode: 0o600 });
      },
    });
    assert.equal(hookCalls, 1, "the deterministic substitution seam must run after stable root authorization");
    assert.equal(loaded.host_sha256, fixture.inventory.host_sha256, "the read stays bound to the authorized directory descriptor");
  } finally {
    rmSync(displaced, { recursive: true, force: true });
    fixture.cleanup();
  }
});

test("authorization profile protected read rejects hard-link substitution", async () => {
  const directory = mkdtempSync(join("/dev/shm/", "red-profile-link-"));
  try {
    const backing = join(directory, "profile-backing.json");
    const profilePath = join(directory, "profile.json");
    const profile = {
      schema_version: "agentic-sandbox.celld-live-profile/v1",
      profile_id: "test-profile",
      run_id: "test-run",
      expected_sandbox_git: "1".repeat(40),
      environment: { kind: "disposable-local", single_host: true, host_sha256: "2".repeat(64) },
      authorization: { destructive_faults: false, inventory_path: "/tmp/test-inventory.json" },
      drivers: { "celld-live-orchestration": { enabled: false, config_path: "/tmp/orchestration.json" } },
    };
    writeFileSync(backing, `${JSON.stringify(profile)}\n`, { mode: 0o600 });
    linkSync(backing, profilePath);
    await assert.rejects(
      liveOrchestration.executeOrchestrationDriver({ scenarioId: "UAT-CELLD-003", runId: "test-run", liveProfilePath: profilePath, artifactDir: join(directory, "artifacts") }),
      /protected|single-link|ownership/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("authorized orchestration config protected read rejects hard-link substitution", async () => {
  const fixture = exactRunFixture("red-config-link");
  try {
    const configBacking = join(fixture.root, "orchestration-backing.json");
    const configPath = join(fixture.root, "orchestration.json");
    const profilePath = join(fixture.root, "live-profile.json");
    writeFileSync(configBacking, `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
    linkSync(configBacking, configPath);
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    writeFileSync(profilePath, `${JSON.stringify({
      schema_version: "agentic-sandbox.celld-live-profile/v1",
      profile_id: `profile-${fixture.runId}`,
      run_id: fixture.runId,
      expected_sandbox_git: "1".repeat(40),
      environment: { kind: "titan-single-host", single_host: true, host_sha256: fixture.inventory.host_sha256 },
      authorization: { destructive_faults: false, exact_run_owner: fixture.runId, inventory_path: fixture.config.inventory_path },
      drivers: { "celld-live-orchestration": { enabled: true, config_path: configPath } },
    })}\n`, { mode: 0o600 });
    await assert.rejects(
      liveOrchestration.executeOrchestrationDriver(
        { scenarioId: "UAT-CELLD-003", runId: fixture.runId, liveProfilePath: profilePath, artifactDir: join(fixture.root, "artifacts") },
        { gitCommit: () => "1".repeat(40), hostname: () => hostname() },
      ),
      /protected|single-link|ownership/,
    );
  } finally {
    fixture.cleanup();
  }
});

test("nested and multiple pending recovery converges in two passes without replay", async () => {
  const fixture = exactRunFixture("red-nested-recovery");
  try {
    const { resource: firstResource } = addObservedProvider(fixture.inventory);
    const first = addPendingCleanup(fixture.inventory, firstResource);
    const secondSubject = providerSubject({
      instanceId: "123e4567-e89b-42d3-a456-426614174001",
      name: "celld-second-provider",
      operationId: "second-provision",
    });
    planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: secondSubject,
    }, new Date("2026-08-23T00:00:02.000Z"));
    const present = new Map([[first.entry.subject.instance_id, true], [secondSubject.instance_id, false]]);
    let removeCalls = 0;
    let replayCalls = 0;
    const runtime = {
      scenarioId: "UAT-CELLD-003",
      runId: fixture.runId,
      config: fixture.config,
      orchestrationInventory: fixture.inventory,
      providerResources: new Map(fixture.inventory.resources.map((resource) => [resource.instance_id, {
        instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate,
      }])),
      persistInventory: () => {},
      sendWorkerCommand: async () => { replayCalls += 1; },
    };
    const dependencies = {
      observeProviderResource: async ({ resource }) => persistedDockerObservation(
        fixture.inventory.resources.find((candidate) => candidate.instance_id === resource.instanceId),
        present.get(resource.instanceId),
      ),
      removeProviderResource: async ({ resource }) => {
        removeCalls += 1;
        present.set(resource.instanceId, false);
      },
    };
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    assert.equal(replayCalls, 0);
    assert.equal(removeCalls, 1);
    assert.deepEqual(fixture.inventory.incomplete_mutation_ids, []);
    assert.equal(fixture.inventory.resources.every((resource) => resource.status === "removed"), true);
  } finally {
    fixture.cleanup();
  }
});

test("nested cleanup already absent after its effect converges without replay across two recovery passes", async () => {
  const fixture = exactRunFixture("red-nested-absent");
  try {
    const { resource } = addObservedProvider(fixture.inventory);
    addPendingCleanup(fixture.inventory, resource);
    const runtime = runtimeFor(fixture, resource);
    let observationCalls = 0;
    let destructiveCalls = 0;
    const dependencies = {
      observeProviderResource: async () => {
        observationCalls += 1;
        return {
          owned: true,
          present: false,
          state: "absent",
          provider_storage_present: false,
          provider_identity_sha256: null,
          configuration_sha256: null,
        };
      },
      removeProviderResource: async () => { destructiveCalls += 1; },
    };
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    assert.ok(observationCalls >= 1);
    assert.equal(destructiveCalls, 0);
    assert.deepEqual(fixture.inventory.incomplete_mutation_ids, []);
    assert.equal(resource.status, "removed");
    assert.equal(runtime.providerResources.has(resource.instance_id), false);
  } finally {
    fixture.cleanup();
  }
});

test("recovery heals an active materialized fault even after its apply terminal was durably committed", async () => {
  const fixture = exactRunFixture("red-active-fault");
  try {
    const fault = addAppliedFault(fixture.inventory);
    const runtime = {
      scenarioId: "UAT-CELLD-006",
      runId: fixture.runId,
      config: fixture.config,
      orchestrationInventory: fixture.inventory,
      providerResources: new Map(),
      persistInventory: () => {},
    };
    let active = true;
    let observeCalls = 0;
    let healCalls = 0;
    const dependencies = {
      observeFaultTarget: async () => {
        observeCalls += 1;
        return {
          owned: true,
          present: active,
          target_identity_sha256: fault.target_identity_sha256,
          target_ownership_sha256: fault.target_ownership_sha256,
        };
      },
      healFaultTarget: async () => {
        healCalls += 1;
        active = false;
      },
    };
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    assert.equal(healCalls, 1);
    assert.ok(observeCalls >= 2);
    assert.equal(fault.status, "healed");
    assert.deepEqual(fixture.inventory.incomplete_mutation_ids, []);
  } finally {
    fixture.cleanup();
  }
});

test("recovery does not trust caller-supplied owned true for unbound provider or fault plans", async (t) => {
  await t.test("provider plan", async () => {
    const fixture = exactRunFixture("red-unbound-provider");
    try {
      const planned = planOrchestrationMutation(fixture.inventory, {
        mutation: "provider_action",
        scenarioId: "UAT-CELLD-003",
        subjectType: "provider_resource",
        subject: providerSubject(),
      });
      const runtime = runtimeFor(fixture, planned.materialized);
      let destructiveCalls = 0;
      await assert.rejects(
        liveOrchestration.recoverOrchestrationInventory(runtime, {
          observeProviderResource: async () => ({
            owned: true,
            present: true,
            state: "created",
            provider_storage_present: false,
            provider_identity_sha256: "9".repeat(64),
            configuration_sha256: "8".repeat(64),
          }),
          removeProviderResource: async () => { destructiveCalls += 1; },
        }),
      );
      assert.equal(destructiveCalls, 0);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("typed fault plan", async () => {
    const fixture = exactRunFixture("red-unbound-fault");
    try {
      appendLegacyUnboundFaultPlan(fixture.inventory);
      const runtime = {
        scenarioId: "UAT-CELLD-006",
        runId: fixture.runId,
        config: fixture.config,
        orchestrationInventory: fixture.inventory,
        providerResources: new Map(),
        persistInventory: () => {},
      };
      let destructiveCalls = 0;
      await assert.rejects(
        liveOrchestration.recoverOrchestrationInventory(runtime, {
          observeFaultTarget: async () => ({ owned: true, present: true }),
          healFaultTarget: async () => { destructiveCalls += 1; },
        }),
      );
      assert.equal(destructiveCalls, 0);
    } finally {
      fixture.cleanup();
    }
  });
});

test("destructive provider effect revalidates immutable container network UUID config labels and storage bindings", async (t) => {
  const removeExactlyObserved = liveOrchestration.removeExactlyObservedProvider;
  const fixture = exactRunFixture("red-effect-binding");
  try {
    const docker = trustedDockerObservation(fixture);
    fixture.config.vm_storage_dir = join(fixture.root, "vm-storage");
    const qemuStoragePath = join(fixture.config.vm_storage_dir, "celld-owned-provider");
    mkdirSync(qemuStoragePath, { recursive: true, mode: 0o700 });
    writeFileSync(join(qemuStoragePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
    const { resource: qemuRecord } = addObservedQemuProvider(fixture.inventory, qemuStoragePath);
    const qemuCleanupPlan = addPendingCleanup(fixture.inventory, qemuRecord).entry;
    const qemuRuntime = durableRuntimeFor(fixture, qemuRecord);
    const qemu = persistedQemuObservation(qemuRecord, qemuStoragePath);
    const vectors = [
      { name: "Docker container ID", substrate: "docker", initial: docker, mutate: (value) => { value.provider_id = "3".repeat(64); value.provider_identity_sha256 = sha256(`docker:${value.provider_id}`); } },
      { name: "Docker container labels", substrate: "docker", initial: docker, mutate: (value) => { value.provider_labels["agentic-run-id"] = "foreign-run"; value.ownership_binding_sha256 = sha256(canonicalOrchestrationJson(value.provider_labels)); } },
      { name: "Docker network ID", substrate: "docker", initial: docker, mutate: (value) => { value.managed_network_id = "4".repeat(64); value.managed_network_identity_sha256 = sha256(`docker-network:${value.managed_network_id}`); } },
      { name: "Docker network labels", substrate: "docker", initial: docker, mutate: (value) => { value.managed_network_labels["agentic-run-id"] = "foreign-run"; value.managed_network_configuration_sha256 = sha256(canonicalOrchestrationJson(value.managed_network_labels)); } },
      { name: "QEMU UUID", substrate: "qemu", initial: qemu, mutate: (value) => { value.provider_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"; value.provider_identity_sha256 = sha256(`qemu:${value.provider_id}`); } },
      { name: "QEMU inactive config", substrate: "qemu", initial: qemu, mutate: (value) => { value.configuration_sha256 = "a".repeat(64); } },
      { name: "QEMU storage identity", substrate: "qemu", initial: qemu, mutate: (value) => { value.provider_storage_identity_sha256 = "b".repeat(64); } },
    ];
    for (const vector of vectors) {
      await t.test(vector.name, async () => {
        assert.equal(typeof removeExactlyObserved, "function", "the production destructive boundary must be directly behavior-testable");
        const initial = structuredClone(vector.initial);
        const substituted = structuredClone(vector.initial);
        vector.mutate(substituted);
        const record = vector.substrate === "qemu" ? qemuRecord : {
          instance_id: "123e4567-e89b-42d3-a456-426614174000",
          name: "celld-owned-provider",
          substrate: vector.substrate,
          provider_identity_sha256: initial.provider_identity_sha256,
          configuration_sha256: initial.configuration_sha256,
          ownership_binding_sha256: initial.ownership_binding_sha256,
          managed_network_identity_sha256: initial.managed_network_identity_sha256,
          managed_network_configuration_sha256: initial.managed_network_configuration_sha256,
          provider_storage_identity_sha256: initial.provider_storage_identity_sha256,
          storage_device: initial.storage_device,
          storage_inode: initial.storage_inode,
          storage_uid: initial.storage_uid,
          storage_gid: initial.storage_gid,
        };
        const resource = { instanceId: record.instance_id, name: record.name, substrate: record.substrate };
        let observeCalls = 0;
        let destructiveCalls = 0;
        await assert.rejects(
          async () => removeExactlyObserved(vector.substrate === "qemu" ? qemuRuntime : { config: fixture.config, runId: fixture.runId }, resource, record, initial, {
            ...(vector.substrate === "qemu" ? { plan: qemuCleanupPlan } : {}),
            observeProviderEffectTarget: async () => { observeCalls += 1; return substituted; },
            destroyProviderTarget: async () => { destructiveCalls += 1; },
          }),
          /substitut|foreign|binding|identity/,
        );
        assert.equal(observeCalls, 1);
        assert.equal(destructiveCalls, 0);
      });
    }
  } finally {
    fixture.cleanup();
  }
});

const unsafeCleanupCases = [
  { name: "ambiguous ownership", substrate: "docker", observe: () => ({ present: true, owned: false, ambiguous: true }) },
  { name: "foreign ownership", substrate: "docker", observe: () => ({ present: true, owned: false }) },
  { name: "substituted identity", substrate: "docker", observe: () => ({ present: true, owned: true, provider_identity_sha256: "e".repeat(64), configuration_sha256: PROVIDER_CONFIGURATION }) },
  { name: "Docker multi-match", substrate: "docker", observe: () => { throw new Error("Docker provider observation is ambiguous"); } },
  { name: "QEMU UUID mismatch", substrate: "qemu", observe: () => ({ present: true, owned: true, provider_identity_sha256: "e".repeat(64), configuration_sha256: PROVIDER_CONFIGURATION }) },
  { name: "QEMU domain absent with storage residue", substrate: "qemu", observe: () => ({ present: false, owned: true, provider_storage_present: true, provider_identity_sha256: null, configuration_sha256: null }) },
];

test("normal and restart cleanup make zero destructive calls for ambiguous foreign or substituted identities", async (t) => {
  for (const mode of ["normal", "recovery"]) {
    for (const vector of unsafeCleanupCases) {
      await t.test(`${mode}: ${vector.name}`, async () => {
        const fixture = exactRunFixture(`red-${mode}-${vector.substrate}`);
        try {
          const { resource } = addObservedProvider(fixture.inventory, {
            subject: providerSubject({ substrate: vector.substrate }),
          });
          if (mode === "recovery") addPendingCleanup(fixture.inventory, resource);
          const runtime = runtimeFor(fixture, resource);
          let observeCalls = 0;
          let destructiveCalls = 0;
          const dependencies = {
            observeProviderResource: async () => {
              observeCalls += 1;
              return vector.observe();
            },
            removeProviderResource: async () => { destructiveCalls += 1; },
          };
          const operation = mode === "normal"
            ? liveOrchestration.cleanupOwnedProviderResources
            : liveOrchestration.recoverOrchestrationInventory;
          assert.equal(typeof operation, "function", "normal cleanup needs an observable exact-identity boundary");
          await assert.rejects(operation(runtime, dependencies));
          assert.ok(observeCalls >= 1, "cleanup must independently observe before refusing");
          assert.equal(destructiveCalls, 0);
          assert.equal(fixture.inventory.state, "cleanup_residue");
        } finally {
          fixture.cleanup();
        }
      });
    }
  }
});

test("normal and restart cleanup remove an exact immutable identity once and retain its digests", async (t) => {
  for (const mode of ["normal", "recovery"]) {
    await t.test(mode, async () => {
      const fixture = exactRunFixture(`red-exact-${mode}`);
      try {
        const { resource } = addObservedProvider(fixture.inventory);
        if (mode === "recovery") addPendingCleanup(fixture.inventory, resource);
        const runtime = runtimeFor(fixture, resource);
        let present = true;
        let removeCalls = 0;
        let observationCalls = 0;
        const dependencies = {
          observeProviderResource: async () => {
            observationCalls += 1;
            return present
              ? {
                  present: true,
                  owned: true,
                  provider_storage_present: false,
                  provider_id: resource.provider_id,
                  provider_identity_sha256: resource.provider_identity_sha256,
                  configuration_sha256: resource.configuration_sha256,
                  ownership_binding_sha256: resource.ownership_binding_sha256,
                  managed_network_id: resource.managed_network_id,
                  managed_network_identity_sha256: resource.managed_network_identity_sha256,
                  managed_network_configuration_sha256: resource.managed_network_configuration_sha256,
                }
              : { present: false, owned: true, provider_storage_present: false, provider_identity_sha256: null, configuration_sha256: null };
          },
          removeProviderResource: async () => {
            removeCalls += 1;
            present = false;
          },
        };
        const operation = mode === "normal"
          ? liveOrchestration.cleanupOwnedProviderResources
          : liveOrchestration.recoverOrchestrationInventory;
        assert.equal(typeof operation, "function");
        await operation(runtime, dependencies);
        await operation(runtime, dependencies);
        assert.equal(removeCalls, 1);
        assert.ok(observationCalls >= 2);
        assert.equal(resource.status, "removed");
        assert.equal(resource.provider_identity_sha256, PROVIDER_IDENTITY);
        assert.equal(resource.configuration_sha256, PROVIDER_CONFIGURATION);
      } finally {
        fixture.cleanup();
      }
    });
  }
});

test("normal provider cleanup continues exact Docker cleanup after QEMU helper residue while preserving failure", async () => {
  const fixture = exactRunFixture("red-qemu-docker-residue");
  try {
    fixture.config.vm_storage_dir = join(fixture.root, "vm-storage");
    const qemuStoragePath = join(fixture.config.vm_storage_dir, "celld-qemu-provider");
    mkdirSync(qemuStoragePath, { recursive: true, mode: 0o700 });
    writeFileSync(join(qemuStoragePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
    const { resource: qemuResource } = addObservedQemuProvider(fixture.inventory, qemuStoragePath, {
      name: "celld-qemu-provider",
    });
    const { resource: dockerResource } = addObservedProvider(fixture.inventory, {
      subject: providerSubject({
        instanceId: "123e4567-e89b-42d3-a456-426614174001",
        name: "celld-docker-provider",
        substrate: "docker",
      }),
    });
    const runtime = {
      scenarioId: "UAT-CELLD-003",
      runId: fixture.runId,
      config: fixture.config,
      orchestrationInventory: fixture.inventory,
      providerResources: new Map(fixture.inventory.resources.map((resource) => [resource.instance_id, {
        instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate,
      }])),
      persistInventory: () => {},
    };
    const present = new Map([[qemuResource.instance_id, true], [dockerResource.instance_id, true]]);
    const removals = [];
    let cleanupError;
    await assert.rejects(
      liveOrchestration.cleanupOwnedProviderResources(runtime, {
        observeProviderResource: async ({ resource }) => {
          const record = fixture.inventory.resources.find((entry) => entry.instance_id === resource.instanceId);
          if (resource.substrate === "qemu") return persistedQemuObservation(record, qemuStoragePath, { present: present.get(resource.instanceId) });
          return persistedDockerObservation(record, present.get(resource.instanceId));
        },
        removeProviderResource: async ({ resource }) => {
          removals.push(resource.instanceId);
          if (resource.substrate === "qemu") throw new liveOrchestration.OrchestrationCleanupResidueError("simulated QEMU helper failure");
          present.set(resource.instanceId, false);
        },
      }),
      (error) => {
        cleanupError = error;
        return error instanceof liveOrchestration.OrchestrationCleanupResidueError;
      },
    );
    assert.equal(cleanupError.message, "simulated QEMU helper failure");
    assert.deepEqual(removals, [qemuResource.instance_id, dockerResource.instance_id]);
    assert.equal(qemuResource.status, "cleanup_pending");
    assert.equal(dockerResource.status, "removed");
    assert.equal(fixture.inventory.state, "cleanup_residue");
    assert.equal(runtime.providerResources.has(dockerResource.instance_id), false);
  } finally {
    fixture.cleanup();
  }
});

test("v1 recovery and provider cleanup are read-only fail-closed and never enter the legacy destructive path", async (t) => {
  await t.test("recovery", async () => {
    const fixture = exactRunFixture("red-v1-recovery");
    try {
      const legacy = makeV1Inventory(fixture);
      const before = structuredClone(legacy);
      let observationCalls = 0;
      let destructiveCalls = 0;
      await assert.rejects(
        liveOrchestration.recoverOrchestrationInventory({
          scenarioId: "UAT-CELLD-003",
          runId: fixture.runId,
          config: fixture.config,
          orchestrationInventory: legacy,
          providerResources: new Map(),
          persistInventory: () => {},
        }, {
          observeProviderResource: async () => { observationCalls += 1; return { owned: true, present: false }; },
          removeProviderResource: async () => { destructiveCalls += 1; },
        }),
        /v1|read-only|upgrade/,
      );
      assert.equal(observationCalls, 0);
      assert.equal(destructiveCalls, 0);
      assert.deepEqual(legacy, before);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("cleanup", async () => {
    const fixture = exactRunFixture("red-v1-cleanup");
    const previousPath = process.env.PATH;
    const previousLog = process.env.CELLD_FAKE_DOCKER_LOG;
    try {
      const fakeBin = join(fixture.root, "fake-bin");
      const logPath = join(fixture.root, "fake-docker.log");
      mkdirSync(fakeBin, { mode: 0o700 });
      writeFileSync(join(fakeBin, "docker"), [
        "#!/bin/sh",
        "printf '%s\\n' \"$*\" >> \"$CELLD_FAKE_DOCKER_LOG\"",
        "if [ \"$1\" = ps ]; then printf '%s\\n' 111111111111; fi",
        "if [ \"$1\" = inspect ]; then printf '%s\\n' '<no value>'; fi",
        "",
      ].join("\n"), { mode: 0o700 });
      chmodSync(join(fakeBin, "docker"), 0o700);
      process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;
      process.env.CELLD_FAKE_DOCKER_LOG = logPath;
      const legacy = makeV1Inventory(fixture);
      const before = structuredClone(legacy);
      const resource = legacy.resources[0];
      await assert.rejects(
        liveOrchestration.cleanupOwnedProviderResources({
          scenarioId: resource.scenario_id,
          runId: fixture.runId,
          config: fixture.config,
          orchestrationInventory: legacy,
          providerResources: new Map([[resource.instance_id, {
            instanceId: resource.instance_id,
            name: resource.name,
            substrate: resource.substrate,
          }]]),
          persistInventory: () => {},
        }),
      );
      const calls = existsSync(logPath) ? readFileSync(logPath, "utf8") : "";
      assert.equal(calls, "", "v1 compatibility must not invoke even a scoped legacy provider command");
      assert.deepEqual(legacy, before);
    } finally {
      if (previousPath === undefined) delete process.env.PATH;
      else process.env.PATH = previousPath;
      if (previousLog === undefined) delete process.env.CELLD_FAKE_DOCKER_LOG;
      else process.env.CELLD_FAKE_DOCKER_LOG = previousLog;
      fixture.cleanup();
    }
  });
});

test("re-provision clears removed identity bindings and crash recovery binds only the legitimate new provider", async (t) => {
  function removedThenReprovision(fixture) {
    const { resource } = addObservedProvider(fixture.inventory, { scenarioId: "UAT-CELLD-005" });
    const cleanup = addPendingCleanup(fixture.inventory, resource);
    finishOrchestrationMutation(fixture.inventory, cleanup.entry, {
      outcome: "absent",
      observedIdentitySha256: null,
      observedConfigurationSha256: null,
    }, new Date("2026-08-23T00:00:04.000Z"));
    const reprovision = planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action",
      scenarioId: "UAT-CELLD-005",
      subjectType: "provider_resource",
      subject: providerSubject({ generation: 2, operationId: "reprovision-generation-2" }),
    }, new Date("2026-08-23T00:00:05.000Z"));
    return { resource, reprovision };
  }

  await t.test("planning clears the retired generation binding", () => {
    const fixture = exactRunFixture("red-reprovision-clear");
    try {
      const { resource } = removedThenReprovision(fixture);
      assert.equal(resource.status, "planned");
      for (const field of [
        "provider_identity_sha256",
        "configuration_sha256",
        "ownership_binding_sha256",
        "managed_network_identity_sha256",
        "managed_network_configuration_sha256",
        "provider_storage_identity_sha256",
      ]) assert.equal(Object.hasOwn(resource, field), false, field);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("recovery binds and removes the legitimate new generation exactly once", async () => {
    const fixture = exactRunFixture("red-reprovision-recovery");
    try {
      const { resource } = removedThenReprovision(fixture);
      const observation = trustedDockerObservation(fixture, { providerId: "5".repeat(64), networkId: "6".repeat(64) });
      let present = true;
      let removeCalls = 0;
      const runtime = runtimeFor(fixture, resource);
      const dependencies = {
        observeProviderResource: async () => present
          ? observation
          : { owned: true, present: false, state: "absent", provider_storage_present: false, provider_identity_sha256: null, configuration_sha256: null },
        removeProviderResource: async ({ observation: effect }) => {
          removeCalls += 1;
          assert.equal(effect.provider_id, observation.provider_id);
          assert.equal(effect.ownership_binding_sha256, observation.ownership_binding_sha256);
          present = false;
        },
      };
      await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
      await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
      assert.equal(removeCalls, 1);
      assert.equal(resource.status, "removed");
      assert.equal(resource.provider_identity_sha256, observation.provider_identity_sha256);
      assert.equal(resource.configuration_sha256, observation.configuration_sha256);
      assert.equal(resource.ownership_binding_sha256, observation.ownership_binding_sha256);
    } finally {
      fixture.cleanup();
    }
  });
});

function writeProtectedRunFiles(fixture, inventory = fixture.inventory, { profileRoot = fixture.root } = {}) {
  writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
  commitOrchestrationInventory(fixture.config.inventory_path, inventory, { config: fixture.config });
  mkdirSync(profileRoot, { recursive: true, mode: 0o700 });
  const profilePath = join(profileRoot, "live-profile.json");
  writeFileSync(profilePath, `${JSON.stringify({
    schema_version: "agentic-sandbox.celld-live-profile/v1",
    profile_id: `profile-${fixture.runId}`,
    run_id: fixture.runId,
    expected_sandbox_git: "1".repeat(40),
    environment: { kind: "titan-single-host", single_host: true, host_sha256: inventory.host_sha256 },
    authorization: { destructive_faults: true, exact_run_owner: fixture.runId, inventory_path: fixture.config.inventory_path },
    drivers: { "celld-live-orchestration": { enabled: true, config_path: join(fixture.root, "orchestration.json") } },
  })}\n`, { mode: 0o600 });
  chmodSync(profilePath, 0o600);
  return profilePath;
}

test("restart recovery is reachable from the explicit protected CLI boundary", () => {
  const fixture = exactRunFixture("red-cli-recover");
  try {
    const profilePath = writeProtectedRunFiles(fixture);
    const result = spawnSync(process.execPath, [DRIVER, "recover", "--config", join(fixture.root, "orchestration.json"), "--profile", profilePath], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).status, "PASS");
  } finally {
    fixture.cleanup();
  }
});

test("production recovery CLI never reports PASS for an active fault without an exact observer and healer", () => {
  const fixture = exactRunFixture("red-cli-active-fault");
  try {
    addAppliedFault(fixture.inventory);
    const profilePath = writeProtectedRunFiles(fixture);
    const result = spawnSync(process.execPath, [DRIVER, "recover", "--config", join(fixture.root, "orchestration.json"), "--profile", profilePath], { encoding: "utf8" });
    assert.equal(result.status, 4, result.stderr);
    assert.equal(result.stdout, "", "unreconciled active faults must never produce PASS JSON");
    const retained = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config);
    assert.equal(retained.state, "cleanup_residue");
    assert.equal(retained.faults[0].status, "applied");
  } finally {
    fixture.cleanup();
  }
});

test("retained-run CLI discovers only the exact same-run inventory before any new preparation", async (t) => {
  await t.test("same run", () => {
    const fixture = exactRunFixture("red-retained-same");
    try {
      writeProtectedRunFiles(fixture);
      const result = spawnSync(process.execPath, [
        DRIVER,
        "recover-retained",
        "--run-id", fixture.runId,
        "--orchestration-root", "/dev/shm/agentic-celld-orchestration",
        "--retained-run-root", fixture.root,
        "--exact-run-owner", fixture.runId,
      ], { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      const response = JSON.parse(result.stdout);
      assert.equal(response.status, "PASS");
      assert.equal(response.run_id, fixture.runId);
      assert.equal(response.discovered_retained_inventory, true);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("new run cannot claim retained inventory", () => {
    const fixture = exactRunFixture("red-retained-foreign");
    try {
      writeProtectedRunFiles(fixture);
      const before = readFileSync(fixture.config.inventory_path, "utf8");
      const result = spawnSync(process.execPath, [
        DRIVER,
        "recover-retained",
        "--run-id", `${fixture.runId}-new`,
        "--orchestration-root", "/dev/shm/agentic-celld-orchestration",
        "--retained-run-root", fixture.root,
        "--exact-run-owner", `${fixture.runId}-new`,
      ], { encoding: "utf8" });
      assert.notEqual(result.status, 0);
      assert.equal(readFileSync(fixture.config.inventory_path, "utf8"), before);
    } finally {
      fixture.cleanup();
    }
  });
});

test("cleanup residue takes exit-4 precedence over a simultaneous campaign error", () => {
  const selectFailure = liveOrchestration.selectOrchestrationRunFailure;
  assert.equal(typeof selectFailure, "function", "campaign and cleanup failures need one typed precedence boundary");
  const campaignError = new Error("campaign failed before producing measurements");
  const cleanupError = new liveOrchestration.OrchestrationCleanupResidueError("exact provider residue remains");
  const selected = selectFailure({ campaignError, cleanupErrors: [cleanupError] });
  assert.equal(selected, cleanupError);
  assert.equal(selected.exitCode, 4);
  assert.equal(selected.campaignError, campaignError);
});

test("cleanup residue exits 4 while ordinary orchestration driver errors remain exit 3", () => {
  const fixture = exactRunFixture("red-cli-exit");
  try {
    planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject(),
    });
    const profilePath = writeProtectedRunFiles(fixture);
    const residue = spawnSync(process.execPath, [DRIVER, "cleanup", "--config", join(fixture.root, "orchestration.json"), "--profile", profilePath], { encoding: "utf8" });
    assert.equal(residue.status, 4, residue.stderr);

    const ordinary = spawnSync(process.execPath, [DRIVER, "unknown-command"], { encoding: "utf8" });
    assert.equal(ordinary.status, 3, ordinary.stderr);
  } finally {
    fixture.cleanup();
  }
});

test("retained recovery reads the protected live profile from the production storage layout", () => {
  const fixture = exactRunFixture("red-retained-layout");
  const storageParent = "/dev/shm/agentic-celld-storage";
  const removeStorageParent = !existsSync(storageParent);
  const storageRoot = join(storageParent, fixture.runId);
  try {
    const profilePath = writeProtectedRunFiles(fixture, fixture.inventory, { profileRoot: storageRoot });
    const result = spawnSync(process.execPath, [
      DRIVER,
      "recover-retained",
      "--run-id", fixture.runId,
      "--orchestration-root", "/dev/shm/agentic-celld-orchestration",
      "--retained-run-root", fixture.root,
      "--profile", profilePath,
      "--exact-run-owner", fixture.runId,
    ], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).discovered_retained_inventory, true);
  } finally {
    fixture.cleanup();
    rmSync(storageRoot, { recursive: true, force: true });
    if (removeStorageParent && existsSync(storageParent)) rmdirSync(storageParent);
  }
});

test("restart recovery cleans completed active and newly-bound provision effects before returning PASS", async (t) => {
  for (const terminalEvent of ["completed", "recovered"]) {
    await t.test(terminalEvent === "completed" ? "completed active provider" : "crash after recovered provision binding", async () => {
      const fixture = exactRunFixture(`red-active-${terminalEvent}`);
      try {
        const planned = planOrchestrationMutation(fixture.inventory, {
          mutation: "provider_action",
          scenarioId: "UAT-CELLD-003",
          subjectType: "provider_resource",
          subject: providerSubject(),
        }, new Date("2026-08-23T00:00:01.000Z"));
        const completed = finishOrchestrationMutation(fixture.inventory, planned.entry, {
          event: terminalEvent,
          outcome: "effect_observed",
          observedProviderId: "1".repeat(64),
          observedIdentitySha256: PROVIDER_IDENTITY,
          observedConfigurationSha256: PROVIDER_CONFIGURATION,
          observedOwnershipBindingSha256: "2".repeat(64),
          observedManagedNetworkId: "3".repeat(64),
          observedManagedNetworkIdentitySha256: "4".repeat(64),
          observedManagedNetworkConfigurationSha256: "5".repeat(64),
        }, new Date("2026-08-23T00:00:02.000Z"));
        const resource = completed.materialized;
        const runtime = runtimeFor(fixture, resource);
        let present = true;
        let removeCalls = 0;
        const dependencies = {
          observeProviderResource: async () => present
            ? {
                owned: true,
                present: true,
                state: "created",
                provider_storage_present: false,
                provider_id: resource.provider_id,
                provider_identity_sha256: resource.provider_identity_sha256,
                configuration_sha256: resource.configuration_sha256,
                ownership_binding_sha256: resource.ownership_binding_sha256,
                managed_network_id: resource.managed_network_id,
                managed_network_identity_sha256: resource.managed_network_identity_sha256,
                managed_network_configuration_sha256: resource.managed_network_configuration_sha256,
              }
            : { owned: true, present: false, state: "absent", provider_storage_present: false, provider_identity_sha256: null, configuration_sha256: null },
          removeProviderResource: async () => { removeCalls += 1; present = false; },
        };
        await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
        await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
        assert.equal(removeCalls, 1, "a durable active effect must be cleaned once even when no plan is incomplete");
        assert.equal(resource.status, "removed");
        assert.deepEqual(fixture.inventory.incomplete_mutation_ids, []);
      } finally {
        fixture.cleanup();
      }
    });
  }
});

test("recovery reuses a heal plan persisted before the fault-apply terminal and converges once", async () => {
  const fixture = exactRunFixture("red-existing-heal");
  try {
    const { apply, heal, fault } = addPendingFaultApplyAndHeal(fixture.inventory);
    let active = true;
    let healCalls = 0;
    let observeCalls = 0;
    const binding = {
      owned: true,
      present: true,
      target_identity_sha256: apply.subject.target_identity_sha256,
      target_ownership_sha256: apply.subject.target_ownership_sha256,
    };
    const runtime = {
      scenarioId: "UAT-CELLD-006",
      runId: fixture.runId,
      config: fixture.config,
      orchestrationInventory: fixture.inventory,
      providerResources: new Map(),
      persistInventory: () => {},
    };
    const dependencies = {
      observeFaultTarget: async () => { observeCalls += 1; return { ...binding, present: active }; },
      healFaultTarget: async ({ plan }) => {
        healCalls += 1;
        assert.equal(plan.mutation_id, heal.mutation_id, "restart must reuse the already durable heal intent");
        active = false;
      },
    };
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
    assert.equal(healCalls, 1);
    assert.ok(observeCalls >= 2);
    assert.equal(fault.status, "healed");
    assert.deepEqual(fixture.inventory.incomplete_mutation_ids, []);
    assert.equal(fixture.inventory.journal.filter((entry) => entry.event === "planned" && entry.mutation === "fault_heal").length, 1);
  } finally {
    fixture.cleanup();
  }
});

test("effect-observed provider terminals durably carry every destructive binding", async () => {
  const fixture = exactRunFixture("red-terminal-bindings");
  try {
    const subject = providerSubject();
    const planned = planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject,
    });
    const observation = trustedDockerObservation(fixture);
    await liveOrchestration.completeObservedProviderMutation({
      scenarioId: "UAT-CELLD-003",
      runId: fixture.runId,
      config: fixture.config,
      orchestrationInventory: fixture.inventory,
      persistInventory: () => {},
    }, { effect: { operation_id: subject.operation_id }, instanceId: subject.instance_id, generation: 1, action: "provision", observation });
    const terminal = fixture.inventory.journal.find((entry) => entry.event === "completed" && entry.mutation_id === planned.entry.mutation_id);
    assert.deepEqual({
      provider_id: terminal.observed_provider_id,
      ownership: terminal.observed_ownership_binding_sha256,
      network_id: terminal.observed_managed_network_id,
      network_identity: terminal.observed_managed_network_identity_sha256,
      network_configuration: terminal.observed_managed_network_configuration_sha256,
    }, {
      provider_id: observation.provider_id,
      ownership: observation.ownership_binding_sha256,
      network_id: observation.managed_network_id,
      network_identity: observation.managed_network_identity_sha256,
      network_configuration: observation.managed_network_configuration_sha256,
    });

  } finally {
    fixture.cleanup();
  }
});

test("journal replay rejects an arbitrary replacement of a materialized destructive binding", () => {
  const fixture = exactRunFixture("red-replay-bindings");
  try {
    addObservedProvider(fixture.inventory);
    fixture.inventory.resources[0].ownership_binding_sha256 = "9".repeat(64);
    assert.match(validateOrchestrationInventoryDocument(fixture.inventory, fixture.config).join("; "), /journal|replay|ownership|binding/);
  } finally {
    fixture.cleanup();
  }
});

test("invalid effect-observed evidence is rejected before journal append", () => {
  const fixture = exactRunFixture("red-effect-transaction");
  try {
    const invalidEvidence = planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject(),
    });
    const beforeEvidence = structuredClone(fixture.inventory);
    assert.throws(
      () => finishOrchestrationMutation(fixture.inventory, invalidEvidence.entry, {
        outcome: "effect_observed", observedIdentitySha256: null, observedConfigurationSha256: null,
      }),
      /evidence|identity|configuration|digest/,
    );
    assert.deepEqual(fixture.inventory, beforeEvidence, "invalid evidence must not append or update materialized state");

  } finally {
    fixture.cleanup();
  }
});

test("a missing effect-observed materialized target is rejected before journal append", () => {
  const fixture = exactRunFixture("red-effect-missing-target");
  try {
    const missingTarget = planOrchestrationMutation(fixture.inventory, {
      mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject(),
    });
    fixture.inventory.resources.splice(0);
    const before = structuredClone(fixture.inventory);
    assert.throws(
      () => finishOrchestrationMutation(fixture.inventory, missingTarget.entry, {
        outcome: "effect_observed", observedIdentitySha256: PROVIDER_IDENTITY, observedConfigurationSha256: PROVIDER_CONFIGURATION,
      }),
      /target|materialized|absent/,
    );
    assert.deepEqual(fixture.inventory, before, "target validation must precede append");
  } finally {
    fixture.cleanup();
  }
});

test("cleanupOrchestrationRoot treats v1 inventory as read-only residue", () => {
  const fixture = exactRunFixture("red-root-v1");
  try {
    const legacy = makeV1Inventory(fixture);
    legacy.resources = [];
    legacy.state = "prepared";
    writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
    writeFileSync(fixture.config.inventory_path, `${JSON.stringify(legacy)}\n`, { mode: 0o600 });
    const before = readFileSync(fixture.config.inventory_path, "utf8");
    assert.throws(() => liveOrchestration.cleanupOrchestrationRoot(join(fixture.root, "orchestration.json")), /v1|read-only|upgrade/);
    assert.equal(readFileSync(fixture.config.inventory_path, "utf8"), before);
    assert.equal(existsSync(fixture.root), true);
  } finally {
    fixture.cleanup();
  }
});

test("an inventory lock records process identity and start time before compare-and-swap", async () => {
  const fixture = exactRunFixture("red-lock-owner");
  const readyPath = join(fixture.root, "holder.ready");
  const releasePath = join(fixture.root, "holder.release");
  let holder;
  try {
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    holder = spawn(process.execPath, [CAS_WRITER, JSON.stringify(fixture.config), "123e4567-e89b-42d3-a456-426614174021", readyPath, releasePath], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const completion = childCompletion(holder);
    await waitForPathOrExit(readyPath, completion);
    const ownerPath = join(fixture.root, ".orchestration-inventory.lock", "owner.json");
    assert.equal(existsSync(ownerPath), true, "lock acquisition must durably identify its holder");
    const owner = JSON.parse(readFileSync(ownerPath, "utf8"));
    assert.equal(owner.schema_version, "agentic-sandbox.celld-orchestration-lock/v1");
    assert.equal(owner.pid, holder.pid);
    assert.match(String(owner.process_start_time_ticks), /^[0-9]+$/);
  } finally {
    if (holder?.exitCode === null) holder.kill("SIGKILL");
    fixture.cleanup();
  }
});

test("a dead inventory lock holder is reclaimed without stealing a live holder's journal", async () => {
  const fixture = exactRunFixture("red-lock-reclaim");
  const firstReady = join(fixture.root, "first.ready");
  const firstRelease = join(fixture.root, "first.release");
  const secondReady = join(fixture.root, "second.ready");
  const secondRelease = join(fixture.root, "second.release");
  let first;
  let second;
  try {
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    first = spawn(process.execPath, [CAS_WRITER, JSON.stringify(fixture.config), "123e4567-e89b-42d3-a456-426614174031", firstReady, firstRelease], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const firstCompletion = childCompletion(first);
    await waitForPathOrExit(firstReady, firstCompletion);
    first.kill("SIGKILL");
    await firstCompletion;

    writeFileSync(secondRelease, "release\n", { flag: "wx", mode: 0o600 });
    second = spawn(process.execPath, [CAS_WRITER, JSON.stringify(fixture.config), "123e4567-e89b-42d3-a456-426614174032", secondReady, secondRelease], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const result = await childCompletion(second);
    assert.equal(result.status, 0, result.stderr);
    const committed = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config);
    assert.equal(committed.journal.length, 1);
  } finally {
    if (first?.exitCode === null) first.kill("SIGKILL");
    if (second?.exitCode === null) second.kill("SIGKILL");
    fixture.cleanup();
  }
});

test("cleanup lifecycle exclusion remains held through root deletion and blocks a concurrent writer", async () => {
  const fixture = exactRunFixture("red-cleanup-lock");
  const coordination = mkdtempSync(join("/dev/shm/", "red-cleanup-coordination-"));
  const configPath = join(fixture.root, "orchestration.json");
  const cleanupReady = join(coordination, "cleanup.ready");
  const cleanupRelease = join(coordination, "cleanup.release");
  const writerReady = join(coordination, "writer.ready");
  const writerRelease = join(coordination, "writer.release");
  let cleaner;
  let writer;
  try {
    writeFileSync(configPath, `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    cleaner = spawn(process.execPath, [CLEANUP_WORKER, configPath, cleanupReady, cleanupRelease], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const cleanerCompletion = childCompletion(cleaner);
    await waitForPathOrExit(cleanupReady, cleanerCompletion);
    writer = spawn(process.execPath, [CAS_WRITER, JSON.stringify(fixture.config), "123e4567-e89b-42d3-a456-426614174041", writerReady, writerRelease], { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] });
    const writerCompletion = childCompletion(writer);
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(existsSync(writerReady), false, "writer must not cross compare while cleanup owns lifecycle exclusion");
    writer.kill("SIGTERM");
    await writerCompletion;
    writeFileSync(cleanupRelease, "release\n", { flag: "wx", mode: 0o600 });
    const cleaned = await cleanerCompletion;
    assert.equal(cleaned.status, 0, cleaned.stderr);
    assert.equal(existsSync(fixture.root), false);
  } finally {
    if (!existsSync(cleanupRelease)) writeFileSync(cleanupRelease, "release\n", { mode: 0o600 });
    if (cleaner?.exitCode === null) cleaner.kill("SIGKILL");
    if (writer?.exitCode === null) writer.kill("SIGKILL");
    fixture.cleanup();
    rmSync(coordination, { recursive: true, force: true });
  }
});

test("cleanup root deletion rejects deterministic pathname substitution", () => {
  const fixture = exactRunFixture("red-cleanup-substitution");
  const displaced = `${fixture.root}-authorized`;
  let hookCalls = 0;
  try {
    writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    assert.throws(() => liveOrchestration.cleanupOrchestrationRoot(join(fixture.root, "orchestration.json"), {
      beforeRootDelete: () => {
        hookCalls += 1;
        renameSync(fixture.root, displaced);
        mkdirSync(fixture.root, { mode: 0o700 });
        writeFileSync(join(fixture.root, "foreign-marker"), "foreign\n", { mode: 0o600 });
      },
    }), /substitut|identity|changed|race/);
    assert.equal(hookCalls, 1);
    assert.equal(readFileSync(join(fixture.root, "foreign-marker"), "utf8"), "foreign\n");
  } finally {
    rmSync(displaced, { recursive: true, force: true });
    fixture.cleanup();
  }
});

test("QEMU cleanup revalidates between destroy and undefine and refuses substituted identity", async () => {
  const fixture = exactRunFixture("red-qemu-revalidate");
  const previousPath = process.env.PATH;
  const previousLog = process.env.CELLD_FAKE_VIRSH_LOG;
  const fakeBin = join(fixture.root, "fake-bin");
  const logPath = join(fixture.root, "virsh.log");
  const storagePath = join(fixture.root, "vm-storage", "celld-owned-provider");
  const uuid = "11111111-2222-4333-8444-555555555555";
  try {
    mkdirSync(fakeBin, { mode: 0o700 });
    mkdirSync(storagePath, { recursive: true, mode: 0o700 });
    writeFileSync(join(fakeBin, "virsh"), "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CELLD_FAKE_VIRSH_LOG\"\n", { mode: 0o700 });
    chmodSync(join(fakeBin, "virsh"), 0o700);
    process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;
    process.env.CELLD_FAKE_VIRSH_LOG = logPath;
    const { resource: record } = addObservedQemuProvider(fixture.inventory, storagePath, { providerId: uuid });
    const cleanupPlan = addPendingCleanup(fixture.inventory, record).entry;
    const runtime = durableRuntimeFor(fixture, record);
    const authorized = persistedQemuObservation(record, storagePath, { state: "running" });
    const substituted = { ...authorized, configuration_sha256: "9".repeat(64), state: "shut off" };
    const resource = { instanceId: record.instance_id, name: record.name, substrate: record.substrate };
    let observeCalls = 0;
    await assert.rejects(
      liveOrchestration.removeExactlyObservedProvider(runtime, resource, record, authorized, {
        plan: cleanupPlan,
        observeProviderEffectTarget: async () => { observeCalls += 1; return observeCalls === 1 ? authorized : substituted; },
      }),
      /substitut|configuration|identity|binding/,
    );
    assert.equal(observeCalls, 2, "QEMU must be re-observed after destroy and before undefine");
    const commands = existsSync(logPath) ? readFileSync(logPath, "utf8") : "";
    assert.match(commands, new RegExp(`destroy ${uuid}`));
    assert.doesNotMatch(commands, /undefine/, "substitution after destroy must stop before undefine");
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousLog === undefined) delete process.env.CELLD_FAKE_VIRSH_LOG;
    else process.env.CELLD_FAKE_VIRSH_LOG = previousLog;
    fixture.cleanup();
  }
});

test("QEMU cleanup confines XML disks and removes only the exact authorized storage directory", async (t) => {
  async function runCase({ prefix, diskPath, expectRejected }) {
    const fixture = exactRunFixture(prefix);
    const previousPath = process.env.PATH;
    const previousLog = process.env.CELLD_FAKE_VIRSH_LOG;
    const fakeBin = join(fixture.root, "fake-bin");
    const logPath = join(fixture.root, "virsh.log");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    const uuid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    try {
      mkdirSync(fakeBin, { mode: 0o700 });
      mkdirSync(storagePath, { recursive: true, mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "test-disk\n", { mode: 0o600 });
      writeFileSync(join(fakeBin, "virsh"), "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CELLD_FAKE_VIRSH_LOG\"\n", { mode: 0o700 });
      chmodSync(join(fakeBin, "virsh"), 0o700);
      process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;
      process.env.CELLD_FAKE_VIRSH_LOG = logPath;
      const { resource: record } = addObservedQemuProvider(fixture.inventory, storagePath, { providerId: uuid });
      const cleanupPlan = addPendingCleanup(fixture.inventory, record).entry;
      const runtime = durableRuntimeFor(fixture, record);
      const observation = {
        ...persistedQemuObservation(record, storagePath),
        disk_source_paths: [diskPath ?? join(storagePath, "disk.qcow2")],
      };
      fixture.config.vm_storage_dir = storageRoot;
      const operation = liveOrchestration.removeExactlyObservedProvider(
        runtime,
        { instanceId: record.instance_id, name: record.name, substrate: record.substrate },
        record,
        observation,
        { plan: cleanupPlan, observeProviderEffectTarget: async () => ({ ...observation, state: "shut off" }) },
      );
      if (expectRejected) {
        await assert.rejects(operation, /disk|path|storage|escape|confine/);
        assert.equal(existsSync(logPath), false, "path escape must be rejected before any virsh mutation");
        assert.equal(existsSync(storagePath), true);
      } else {
        await operation;
        const commands = readFileSync(logPath, "utf8");
        assert.match(commands, new RegExp(`undefine ${uuid}`));
        assert.match(commands, new RegExp(`undefine ${uuid} --nvram`));
        assert.equal(existsSync(storagePath), false, "the exact authorized directory is removed after undefine");
        assert.equal(existsSync(storageRoot), true, "cleanup must not broaden beyond the exact provider directory");
      }
    } finally {
      if (previousPath === undefined) delete process.env.PATH;
      else process.env.PATH = previousPath;
      if (previousLog === undefined) delete process.env.CELLD_FAKE_VIRSH_LOG;
      else process.env.CELLD_FAKE_VIRSH_LOG = previousLog;
      fixture.cleanup();
    }
  }

  await t.test("exact confined storage", () => runCase({ prefix: "red-qemu-storage" }));
  await t.test("XML disk path escape", () => runCase({ prefix: "red-qemu-escape", diskPath: "/etc/passwd", expectRejected: true }));
});

test("QEMU cleanup preserves a pathname substituted during virsh undefine and reports typed residue", async () => {
  const fixture = exactRunFixture("red-qemu-post-undefine");
  const previousPath = process.env.PATH;
  const previousLog = process.env.CELLD_FAKE_VIRSH_LOG;
  const previousStorage = process.env.CELLD_FAKE_QEMU_STORAGE;
  const previousDisplaced = process.env.CELLD_FAKE_QEMU_DISPLACED;
  const fakeBin = join(fixture.root, "fake-bin");
  const logPath = join(fixture.root, "virsh.log");
  const storageRoot = join(fixture.root, "vm-storage");
  const storagePath = join(storageRoot, "celld-owned-provider");
  const displacedPath = join(storageRoot, "authorized-before-undefine");
  const markerPath = join(storagePath, "foreign-marker");
  const uuid = "dddddddd-eeee-4fff-8aaa-bbbbbbbbbbbb";
  try {
    mkdirSync(fakeBin, { mode: 0o700 });
    mkdirSync(storagePath, { recursive: true, mode: 0o700 });
    writeFileSync(join(storagePath, "disk.qcow2"), "authorized-disk\n", { mode: 0o600 });
    writeFileSync(join(fakeBin, "virsh"), [
      "#!/bin/sh",
      "printf '%s\\n' \"$*\" >> \"$CELLD_FAKE_VIRSH_LOG\"",
      "case \" $* \" in",
      "  *\" undefine \"*)",
      "    mv \"$CELLD_FAKE_QEMU_STORAGE\" \"$CELLD_FAKE_QEMU_DISPLACED\"",
      "    mkdir -m 700 \"$CELLD_FAKE_QEMU_STORAGE\"",
      "    printf '%s\\n' foreign > \"$CELLD_FAKE_QEMU_STORAGE/foreign-marker\"",
      "    ;;",
      "esac",
      "",
    ].join("\n"), { mode: 0o700 });
    chmodSync(join(fakeBin, "virsh"), 0o700);
    process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;
    process.env.CELLD_FAKE_VIRSH_LOG = logPath;
    process.env.CELLD_FAKE_QEMU_STORAGE = storagePath;
    process.env.CELLD_FAKE_QEMU_DISPLACED = displacedPath;
    const { resource: record } = addObservedQemuProvider(fixture.inventory, storagePath, { providerId: uuid });
    const cleanupPlan = addPendingCleanup(fixture.inventory, record).entry;
    const runtime = durableRuntimeFor(fixture, record);
    const observation = persistedQemuObservation(record, storagePath);
    let cleanupError = null;
    try {
      fixture.config.vm_storage_dir = storageRoot;
      await liveOrchestration.removeExactlyObservedProvider(
        runtime,
        { instanceId: record.instance_id, name: record.name, substrate: record.substrate },
        record,
        observation,
        { plan: cleanupPlan, observeProviderEffectTarget: async () => observation },
      );
    } catch (error) {
      cleanupError = error;
    }
    assert.equal(existsSync(markerPath), true, "post-undefine substitution must never redirect deletion to the replacement path");
    assert.equal(existsSync(displacedPath), true, "the authorized pre-undefine directory remains explicit cleanup residue");
    assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
    assert.equal(cleanupError.exitCode, 4);
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousLog === undefined) delete process.env.CELLD_FAKE_VIRSH_LOG;
    else process.env.CELLD_FAKE_VIRSH_LOG = previousLog;
    if (previousStorage === undefined) delete process.env.CELLD_FAKE_QEMU_STORAGE;
    else process.env.CELLD_FAKE_QEMU_STORAGE = previousStorage;
    if (previousDisplaced === undefined) delete process.env.CELLD_FAKE_QEMU_DISPLACED;
    else process.env.CELLD_FAKE_QEMU_DISPLACED = previousDisplaced;
    fixture.cleanup();
  }
});

test("terminal completion and runtime validation enforce the schema-required provider and fault bindings", async (t) => {
  const schema = JSON.parse(readFileSync(new URL("./orchestration-inventory-v2.schema.json", import.meta.url), "utf8"));
  const effectObservedRule = schema.$defs.journalEntry.allOf.find((rule) => rule.if?.properties?.subject_type?.const === "provider_resource");
  for (const field of ["observed_provider_id", "observed_identity_sha256", "observed_configuration_sha256", "observed_ownership_binding_sha256"]) {
    assert.ok(effectObservedRule.then.required.includes(field), field);
  }
  for (const field of ["target_identity_sha256", "target_ownership_sha256"]) {
    assert.ok(schema.$defs.faultIntent.required.includes(field), field);
  }

  for (const vector of [
    { name: "configuration omitted", value: undefined },
    { name: "configuration null", value: null },
  ]) {
    await t.test(vector.name, () => {
      const fixture = exactRunFixture(`red-config-${vector.name.replaceAll(" ", "-")}`);
      try {
        const subject = providerSubject();
        const planned = planOrchestrationMutation(fixture.inventory, {
          mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject,
        });
        const before = structuredClone(fixture.inventory);
        const options = {
          outcome: "effect_observed",
          observedIdentitySha256: PROVIDER_IDENTITY,
          observedProviderId: "1".repeat(64),
          observedOwnershipBindingSha256: "2".repeat(64),
          observedManagedNetworkId: "3".repeat(64),
          observedManagedNetworkIdentitySha256: "4".repeat(64),
          observedManagedNetworkConfigurationSha256: "5".repeat(64),
          ...(vector.value !== undefined ? { observedConfigurationSha256: vector.value } : {}),
        };
        assert.throws(() => finishOrchestrationMutation(fixture.inventory, planned.entry, options), /configuration|config|digest/);
        assert.deepEqual(fixture.inventory, before, "configuration evidence must be validated before append");
      } finally {
        fixture.cleanup();
      }
    });
  }

  await t.test("provider terminal bindings required by schema", () => {
    const fixture = exactRunFixture("red-provider-runtime-schema");
    try {
      const planned = planOrchestrationMutation(fixture.inventory, {
        mutation: "provider_action", scenarioId: "UAT-CELLD-003", subjectType: "provider_resource", subject: providerSubject(),
      });
      const before = structuredClone(fixture.inventory);
      assert.throws(() => finishOrchestrationMutation(fixture.inventory, planned.entry, {
        outcome: "effect_observed",
        observedIdentitySha256: PROVIDER_IDENTITY,
        observedConfigurationSha256: PROVIDER_CONFIGURATION,
      }), /provider|ownership|network|binding/);
      assert.deepEqual(fixture.inventory, before);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("fault intent bindings required by schema", () => {
    const fixture = exactRunFixture("red-fault-runtime-schema");
    try {
      const before = structuredClone(fixture.inventory);
      assert.throws(() => planOrchestrationMutation(fixture.inventory, {
        mutation: "fault_apply",
        scenarioId: "UAT-CELLD-006",
        subjectType: "fault",
        subject: { fault_id: "a".repeat(32), kind: "callback_relay_pause", target: "celld-owned-relay" },
      }), /identity|ownership|binding/);
      assert.deepEqual(fixture.inventory, before);
    } finally {
      fixture.cleanup();
    }
  });
});

test("pre-publication lock crashes are reclaimable and cleanup failures retain residue exit typing", async (t) => {
  const vectors = [
    { name: "owner file absent", writeOwner: null },
    { name: "owner file empty", writeOwner: "" },
    { name: "owner file malformed", writeOwner: "{\"schema_version\":" },
  ];
  for (const vector of vectors) {
    await t.test(vector.name, () => {
      const fixture = exactRunFixture(`red-lock-${vector.name.replaceAll(" ", "-")}`);
      let lifecycle = null;
      try {
        const lockRoot = join(fixture.root, ".orchestration-inventory.lock");
        mkdirSync(lockRoot, { mode: 0o700 });
        if (vector.writeOwner !== null) writeFileSync(join(lockRoot, "owner.json"), vector.writeOwner, { mode: 0o600 });
        lifecycle = acquireOrchestrationInventoryLifecycle(fixture.config, { deadlineMs: 100 });
        assert.ok(lifecycle, "a crash before complete owner publication must not permanently wedge the run");
      } finally {
        if (lifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
        fixture.cleanup();
      }
    });
  }

  await t.test("cleanup remains typed residue after malformed pre-publication lock", () => {
    const fixture = exactRunFixture("red-lock-cleanup-typing");
    try {
      const legacy = makeV1Inventory(fixture);
      legacy.resources = [];
      legacy.state = "prepared";
      writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
      writeFileSync(fixture.config.inventory_path, `${JSON.stringify(legacy)}\n`, { mode: 0o600 });
      const lockRoot = join(fixture.root, ".orchestration-inventory.lock");
      mkdirSync(lockRoot, { mode: 0o700 });
      writeFileSync(join(lockRoot, "owner.json"), "{", { mode: 0o600 });
      let cleanupError = null;
      try { liveOrchestration.cleanupOrchestrationRoot(join(fixture.root, "orchestration.json")); } catch (error) { cleanupError = error; }
      assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
      assert.equal(cleanupError.exitCode, 4);
      assert.equal(existsSync(fixture.root), true);
    } finally {
      fixture.cleanup();
    }
  });
});

test("restart recovery owns QEMU storage quarantine across the rename-delete crash window", async (t) => {
  await t.test("journal-owned quarantine is rediscovered and removed without following a substituted original pathname", async () => {
    const fixture = exactRunFixture("red-qemu-quarantine-recovery");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    let quarantinePath;
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(storagePath, { recursive: true, mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "authorized-storage\n", { mode: 0o600 });
      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      const cleanupPlan = addPendingCleanup(fixture.inventory, resource);
      quarantinePath = join(storageRoot, `.${resource.name}.cleanup-${cleanupPlan.entry.mutation_id}`);
      const runtime = durableRuntimeFor(fixture, resource);

      renameSync(storagePath, quarantinePath);
      mkdirSync(storagePath, { mode: 0o700 });
      const foreignMarker = join(storagePath, "foreign-marker");
      writeFileSync(foreignMarker, "foreign replacement\n", { mode: 0o600 });

      const result = await liveOrchestration.recoverOrchestrationInventory(runtime, {
        observeProviderResource: async () => ({
          owned: true,
          present: false,
          state: "absent",
          provider_storage_present: false,
          provider_identity_sha256: null,
          configuration_sha256: null,
        }),
        removeProviderResource: async () => { throw new Error("restart followed the substituted original pathname"); },
      });

      assert.equal(result.status, "PASS");
      assert.equal(existsSync(quarantinePath), false, "restart must rediscover and delete the exact journal-owned quarantine before PASS");
      assert.equal(readFileSync(foreignMarker, "utf8"), "foreign replacement\n", "restart must not follow or delete the substituted original pathname");
      assert.equal(resource.status, "removed", "removed is valid only after exact quarantine deletion");
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("original pathname absence cannot prove cleanup while an unbound quarantine retains the persisted storage identity", async () => {
    const fixture = exactRunFixture("red-qemu-quarantine-ambiguous");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    const unknownQuarantine = join(storageRoot, ".celld-owned-provider.cleanup-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(storagePath, { recursive: true, mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "persisted-storage-identity\n", { mode: 0o600 });
      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      addPendingCleanup(fixture.inventory, resource);
      const runtime = durableRuntimeFor(fixture, resource);
      renameSync(storagePath, unknownQuarantine);

      let recoveryError = null;
      try {
        await liveOrchestration.recoverOrchestrationInventory(runtime, {
          observeProviderResource: async () => ({
            owned: true,
            present: false,
            state: "absent",
            provider_storage_present: false,
            provider_identity_sha256: null,
            configuration_sha256: null,
          }),
          removeProviderResource: async () => { throw new Error("ambiguous quarantine must not be destroyed"); },
        });
      } catch (error) {
        recoveryError = error;
      }

      assert.ok(recoveryError instanceof liveOrchestration.OrchestrationCleanupResidueError, "unbound quarantine residue must prevent a false PASS");
      assert.equal(recoveryError.exitCode, 4);
      assert.notEqual(resource.status, "removed", "original pathname absence alone must not complete an active persisted storage identity");
      assert.equal(existsSync(unknownQuarantine), true, "ambiguous quarantine must remain untouched for explicit recovery");
    } finally {
      fixture.cleanup();
    }
  });
});

test("durable staged lifecycle owners are reclaimed or refused before cleanup proceeds", async (t) => {
  const lockSchema = "agentic-sandbox.celld-orchestration-lock/v1";
  const deadPid = 2_147_483_647;

  await t.test("dead and invalid fsynced pending-owner candidates are reclaimed before lock publication", () => {
    const fixture = exactRunFixture("red-staged-lock-reclaim");
    let lifecycle = null;
    try {
      const deadCandidate = stagedLifecycleCandidate(fixture.root, {
        pid: deadPid,
        nonce: "1111111111111111",
        owner: { schema_version: lockSchema, pid: deadPid, process_start_time_ticks: "1" },
      });
      const invalidCandidate = stagedLifecycleCandidate(fixture.root, {
        pid: deadPid,
        nonce: "2222222222222222",
        owner: "{\"schema_version\":",
      });

      lifecycle = acquireOrchestrationInventoryLifecycle(fixture.config, { deadlineMs: 100 });
      assert.equal(existsSync(deadCandidate), false, "a dead staged claimant must be removed before a successor publishes the lock");
      assert.equal(existsSync(invalidCandidate), false, "a stable invalid staged claimant must be removed before a successor publishes the lock");
    } finally {
      if (lifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
      fixture.cleanup();
    }
  });

  await t.test("a live fsynced pending owner is not stolen", () => {
    const fixture = exactRunFixture("red-staged-lock-live");
    let lifecycle = null;
    let acquisitionError = null;
    try {
      const liveCandidate = stagedLifecycleCandidate(fixture.root, {
        pid: process.pid,
        nonce: "3333333333333333",
        owner: {
          schema_version: lockSchema,
          pid: process.pid,
          process_start_time_ticks: processStartTimeTicksForTest(process.pid),
        },
      });
      try {
        lifecycle = acquireOrchestrationInventoryLifecycle(fixture.config, { deadlineMs: 50 });
      } catch (error) {
        acquisitionError = error;
      }
      assert.ok(acquisitionError, "a successor must not publish over a live staged claimant");
      assert.match(acquisitionError.message, /lock|live|owner|deadline|pending/);
      assert.equal(existsSync(liveCandidate), true, "live staged ownership evidence must remain intact");
    } finally {
      if (lifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
      fixture.cleanup();
    }
  });

  await t.test("cleanup reclaims a dead staged owner and converges", () => {
    const fixture = exactRunFixture("red-staged-lock-cleanup");
    try {
      writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
      commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
      stagedLifecycleCandidate(fixture.root, {
        pid: deadPid,
        nonce: "4444444444444444",
        owner: { schema_version: lockSchema, pid: deadPid, process_start_time_ticks: "1" },
      });
      const result = liveOrchestration.cleanupOrchestrationRoot(join(fixture.root, "orchestration.json"));
      assert.equal(result.status, "PASS");
      assert.equal(existsSync(fixture.root), false, "dead staged-owner residue must not wedge cleanup recovery");
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("cleanup refusal for a live staged owner retains typed residue semantics", () => {
    const fixture = exactRunFixture("red-staged-lock-cleanup-live");
    try {
      writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
      commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
      const liveCandidate = stagedLifecycleCandidate(fixture.root, {
        pid: process.pid,
        nonce: "5555555555555555",
        owner: {
          schema_version: lockSchema,
          pid: process.pid,
          process_start_time_ticks: processStartTimeTicksForTest(process.pid),
        },
      });
      let cleanupError = null;
      try { liveOrchestration.cleanupOrchestrationRoot(join(fixture.root, "orchestration.json")); } catch (error) { cleanupError = error; }
      assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
      assert.equal(cleanupError.exitCode, 4);
      assert.match(cleanupError.message, /acquire exact lifecycle exclusion|live staged|pending owner/, "cleanup must refuse at lifecycle acquisition rather than publishing over the live staged owner");
      assert.equal(existsSync(liveCandidate), true, "cleanup must preserve a live staged owner's evidence");
      assert.equal(existsSync(fixture.root), true);
    } finally {
      fixture.cleanup();
    }
  });
});

test("a production-format staged lock without owner.json is protected by its live process identity", async (t) => {
  await t.test("a live child paused before owner publication is neither stolen nor removed", async () => {
    const fixture = exactRunFixture("red-live-pre-owner-lock");
    let holder;
    let lifecycle = null;
    try {
      holder = await startPendingLockHolder(fixture, "6666666666666666");
      assert.equal(processStartTimeTicksForTest(holder.child.pid), holder.processStartTimeTicks, "the staged claimant retains the exact live process identity");
      let acquisitionError = null;
      try { lifecycle = acquireOrchestrationInventoryLifecycle(fixture.config, { deadlineMs: 50 }); } catch (error) { acquisitionError = error; }
      assert.ok(acquisitionError, "lock publication must wait rather than steal a live pre-owner staging directory");
      assert.match(acquisitionError.message, /live|pending|owner|lock|deadline/);
      assert.equal(existsSync(holder.candidate), true, "the live pre-owner candidate must remain intact");
    } finally {
      if (lifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
      if (holder) await stopPendingLockHolder(holder);
      fixture.cleanup();
    }
  });

  await t.test("the same ownerless staging format is reclaimable only after the child dies", async () => {
    const fixture = exactRunFixture("red-dead-pre-owner-lock");
    let holder;
    let lifecycle = null;
    try {
      holder = await startPendingLockHolder(fixture, "7777777777777777");
      await stopPendingLockHolder(holder);
      assert.equal(existsSync(holder.candidate), true, "the crash residue exists before successor acquisition");
      lifecycle = acquireOrchestrationInventoryLifecycle(fixture.config, { deadlineMs: 100 });
      assert.equal(existsSync(holder.candidate), false, "a successor reclaims the candidate after its exact process identity is dead");
    } finally {
      if (lifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
      if (holder) await stopPendingLockHolder(holder);
      fixture.cleanup();
    }
  });
});

test("restart recovery maps a live pre-owner lifecycle exclusion to typed cleanup residue", async () => {
  const fixture = exactRunFixture("red-recovery-live-pre-owner-lock");
  let holder;
  try {
    const { resource } = addObservedProvider(fixture.inventory);
    addPendingCleanup(fixture.inventory, resource);
    writeFileSync(join(fixture.root, "orchestration.json"), `${JSON.stringify(fixture.config)}\n`, { mode: 0o600 });
    commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
    const runtime = runtimeFor(fixture, resource);
    delete runtime.persistInventory;
    runtime.persistedJournalHeadSha256 = fixture.inventory.journal_head_sha256;
    holder = await startPendingLockHolder(fixture, "8888888888888888");

    let recoveryError = null;
    try {
      await liveOrchestration.recoverOrchestrationInventory(runtime, {
        observeProviderResource: async () => { throw new Error("recovery crossed the live lifecycle exclusion"); },
      });
    } catch (error) {
      recoveryError = error;
    }
    assert.ok(recoveryError instanceof liveOrchestration.OrchestrationCleanupResidueError, "a recovery lock refusal is cleanup residue, not an ordinary driver error");
    assert.equal(recoveryError.exitCode, 4);
    assert.match(recoveryError.message, /lifecycle|lock|live|pending|owner/);
    assert.equal(existsSync(holder.candidate), true, "typed refusal must preserve the live staged claimant");
  } finally {
    if (holder) await stopPendingLockHolder(holder);
    fixture.cleanup();
  }
});

test("QEMU quarantine deletion refuses pathname substitution after inode verification", async (t) => {
  async function runCase(mode) {
    const fixture = exactRunFixture(`red-qemu-quarantine-race-${mode}`);
    const previousPath = process.env.PATH;
    const fakeBin = join(fixture.root, "fake-bin");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    let race = null;
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(fakeBin, { mode: 0o700 });
      mkdirSync(storagePath, { recursive: true, mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
      writeFileSync(join(fakeBin, "virsh"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
      chmodSync(join(fakeBin, "virsh"), 0o700);
      process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;

      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
      const observation = persistedQemuObservation(resource, storagePath);
      const quarantinePath = cleanupPlan.subject.storage_quarantine_path;
      const deletionRootPath = physicalQemuTestCapturePath(cleanupPlan);
      race = installQemuQuarantineDeletionSubstitution(deletionRootPath, `${deletionRootPath}.authorized-residue`);
      const runtime = durableRuntimeFor(fixture, resource);
      let observeCalls = 0;
      const dependencies = {
        observeProviderResource: async () => {
          observeCalls += 1;
          return observeCalls === 1 ? observation : persistedQemuObservation(resource, storagePath, { present: false });
        },
        removeProviderResource: async ({ plan, record, resource: ownedResource, observation: authorized }) => liveOrchestration.removeExactlyObservedProvider(
          runtime,
          ownedResource,
          record,
          authorized,
          { plan, observeProviderEffectTarget: async () => observation, beforeQuarantineDelete: race.beforeDelete },
        ),
      };
      let cleanupError = null;
      try {
        if (mode === "normal") await liveOrchestration.cleanupOwnedProviderResources(runtime, dependencies);
        else await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
      } catch (error) {
        cleanupError = error;
      }

      assert.equal(race.wasSubstituted(), true, "the explicit race seam substitutes only after the quarantine descriptor and inode are verified");
      assert.equal(existsSync(race.foreignMarker), true, "foreign replacement at the quarantine pathname must never be deleted");
      assert.equal(existsSync(race.authorizedResiduePath), true, "the authorized inode remains explicit cleanup residue");
      assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
      assert.equal(cleanupError.exitCode, 4);
      assert.notEqual(resource.status, "removed", "cleanup must not terminalize while authorized residue remains");
      assert.ok(fixture.inventory.incomplete_mutation_ids.includes(cleanupPlan.mutation_id), "the cleanup plan remains recoverable after the race");
    } finally {
      if (previousPath === undefined) delete process.env.PATH;
      else process.env.PATH = previousPath;
      fixture.cleanup();
    }
  }

  await t.test("normal cleanup", () => runCase("normal"));
  await t.test("restart recovery", () => runCase("recovery"));
});

test("the public QEMU removal boundary requires a persisted journal quarantine binding", async (t) => {
  for (const vector of [
    { name: "missing cleanup plan", plan: undefined },
    { name: "cleanup plan missing quarantine binding", plan: "invalid-binding" },
  ]) {
    await t.test(vector.name, async () => {
      const fixture = exactRunFixture(`red-qemu-boundary-${vector.name.replaceAll(" ", "-")}`);
      const storageRoot = join(fixture.root, "vm-storage");
      const storagePath = join(storageRoot, "celld-owned-provider");
      try {
        fixture.config.vm_storage_dir = storageRoot;
        mkdirSync(storagePath, { recursive: true, mode: 0o700 });
        writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
        const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
        const persistedPlan = addPendingCleanup(fixture.inventory, resource).entry;
        const runtime = durableRuntimeFor(fixture, resource);
        const plan = vector.plan === "invalid-binding"
          ? { ...persistedPlan, subject: { ...persistedPlan.subject, storage_quarantine_path: undefined, storage_quarantine_identity_sha256: undefined } }
          : undefined;
        const observation = persistedQemuObservation(resource, storagePath);
        let destructiveCalls = 0;
        await assert.rejects(
          liveOrchestration.removeExactlyObservedProvider(
            runtime,
            { instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate },
            resource,
            observation,
            {
              ...(plan ? { plan } : {}),
              observeProviderEffectTarget: async () => observation,
              destroyProviderTarget: async () => { destructiveCalls += 1; },
            },
          ),
          /plan|quarantine|binding|persisted|journal/,
        );
        assert.equal(destructiveCalls, 0, "missing durable quarantine authority must be rejected before the provider effect");
        assert.equal(existsSync(storagePath), true);
      } finally {
        fixture.cleanup();
      }
    });
  }
});

test("the public QEMU effect boundary requires exact protected on-disk authorization", async (t) => {
  for (const vector of [
    { name: "structurally valid plan was never committed", diskState: "missing" },
    { name: "protected disk retains a stale different journal head", diskState: "stale" },
  ]) {
    await t.test(vector.name, async () => {
      const fixture = exactRunFixture(`red-qemu-durable-authority-${vector.diskState}`);
      const storageRoot = join(fixture.root, "vm-storage");
      const storagePath = join(storageRoot, "celld-owned-provider");
      try {
        fixture.config.vm_storage_dir = storageRoot;
        mkdirSync(storagePath, { recursive: true, mode: 0o700 });
        writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
        const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
        let durableHead = null;
        if (vector.diskState === "stale") {
          commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
          durableHead = fixture.inventory.journal_head_sha256;
        }
        const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
        assert.notEqual(fixture.inventory.journal_head_sha256, durableHead, "the in-memory cleanup authority is newer than durable state");
        const runtime = runtimeFor(fixture, resource);
        runtime.persistedJournalHeadSha256 = durableHead;
        const observation = persistedQemuObservation(resource, storagePath);
        let destructiveCalls = 0;

        await assert.rejects(
          liveOrchestration.removeExactlyObservedProvider(
            runtime,
            { instanceId: resource.instance_id, name: resource.name, substrate: resource.substrate },
            resource,
            observation,
            {
              plan: cleanupPlan,
              observeProviderEffectTarget: async () => observation,
              destroyProviderTarget: async () => { destructiveCalls += 1; },
            },
          ),
          /durable|disk|protected|inventory|journal|head|persisted|commit/,
        );
        assert.equal(destructiveCalls, 0, "an in-memory-only plan must authorize zero provider effects");
        assert.equal(existsSync(storagePath), true);
        assert.ok(fixture.inventory.incomplete_mutation_ids.includes(cleanupPlan.mutation_id));
        if (vector.diskState === "stale") {
          const disk = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config, { expectedHostSha256: fixture.inventory.host_sha256 });
          assert.equal(disk.journal_head_sha256, durableHead);
          assert.notEqual(disk.journal_head_sha256, fixture.inventory.journal_head_sha256);
          assert.equal(disk.journal.some((entry) => entry.mutation_id === cleanupPlan.mutation_id), false);
        }
      } finally {
        fixture.cleanup();
      }
    });
  }
});

test("QEMU cleanup refuses substitution at every final pathname deletion", async (t) => {
  const operations = ["quarantine-root", "leaf-file", "leaf-symlink", "nested-directory"];

  async function runCase(mode, operation) {
    const fixture = exactRunFixture(`red-qemu-final-name-${mode}-${operation}`);
    const previousPath = process.env.PATH;
    const fakeBin = join(fixture.root, "fake-bin");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    let race = null;
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(fakeBin, { mode: 0o700 });
      mkdirSync(storagePath, { recursive: true, mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
      if (operation === "nested-directory") {
        mkdirSync(join(storagePath, "nested"), { mode: 0o700 });
        writeFileSync(join(storagePath, "nested", "inner.bin"), "nested authorized storage\n", { mode: 0o600 });
      }
      writeFileSync(join(fakeBin, "virsh"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
      chmodSync(join(fakeBin, "virsh"), 0o700);
      process.env.PATH = `${fakeBin}:${previousPath ?? ""}`;

      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
      const runtime = durableRuntimeFor(fixture, resource);
      const observation = persistedQemuObservation(resource, storagePath);
      const quarantinePath = cleanupPlan.subject.storage_quarantine_path;
      const deletionRootPath = physicalQemuTestCapturePath(cleanupPlan);
      if (mode === "recovery") renameSync(storagePath, quarantinePath);
      race = installQemuFinalNameOperationSubstitution({ operation, quarantinePath, deletionRootPath, storageRoot });
      let observeCalls = 0;
      const dependencies = {
        beforeQemuFinalNameOperation: race.beforeFinalNameOperation,
        observeProviderResource: async () => {
          observeCalls += 1;
          return observeCalls === 1 && mode === "normal"
            ? observation
            : persistedQemuObservation(resource, storagePath, { present: false });
        },
        removeProviderResource: async ({ plan, record, resource: ownedResource, observation: authorized }) => liveOrchestration.removeExactlyObservedProvider(
          runtime,
          ownedResource,
          record,
          authorized,
          {
            plan,
            observeProviderEffectTarget: async () => observation,
            beforeQemuFinalNameOperation: race.beforeFinalNameOperation,
          },
        ),
      };
      let cleanupError = null;
      try {
        if (mode === "normal") await liveOrchestration.cleanupOwnedProviderResources(runtime, dependencies);
        else await liveOrchestration.recoverOrchestrationInventory(runtime, dependencies);
      } catch (error) {
        cleanupError = error;
      }

      assert.equal(race.wasSubstituted(), true, "the race seam must run immediately at the selected final name operation");
      assert.doesNotThrow(() => lstatSync(race.foreignPath), "the foreign replacement must survive final pathname deletion");
      assert.equal(existsSync(race.authorizedResiduePath), true, "moved authorized residue remains discoverable");
      assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
      assert.equal(cleanupError.exitCode, 4);
      assert.notEqual(resource.status, "removed", "authorized residue prevents cleanup terminalization");
      assert.ok(fixture.inventory.incomplete_mutation_ids.includes(cleanupPlan.mutation_id));
    } finally {
      if (previousPath === undefined) delete process.env.PATH;
      else process.env.PATH = previousPath;
      fixture.cleanup();
    }
  }

  for (const mode of ["normal", "recovery"]) {
    for (const operation of operations) await t.test(`${mode}: ${operation}`, () => runCase(mode, operation));
  }
});

test("QEMU cleanup refuses substitution of the actual post-capture pathname", async (t) => {
  for (const operation of ["leaf-file", "leaf-symlink", "nested-directory", "quarantine-root"]) {
    await t.test(operation, async () => {
      const harness = qemuPostCaptureHarness(`red-qemu-post-capture-${operation}`, operation, "normal");
      try {
        assertProtectedDirectoryForDeletion(harness.fixture.root, "orchestration run root");
        assertProtectedDirectoryForDeletion(harness.storageRoot, "configured QEMU storage root");
        let cleanupError = null;
        try { await harness.runPass(true); } catch (error) { cleanupError = error; }

        assertPostCaptureSubstitutionResidue(harness.race);
        assert.ok(cleanupError instanceof liveOrchestration.OrchestrationCleanupResidueError);
        assert.equal(cleanupError.exitCode, 4);
        assert.notEqual(harness.resource.status, "removed", "the moved authorized inode prevents cleanup terminalization");
        assert.ok(harness.fixture.inventory.incomplete_mutation_ids.includes(harness.cleanupPlan.mutation_id));
      } finally {
        harness.cleanup();
      }
    });
  }
});

test("QEMU final capture requires a protected rooted deletion boundary", async (t) => {
  await t.test("journal-owned final capture is outside the runner-controlled VM root", () => {
    const fixture = exactRunFixture("red-qemu-capture-protection-path");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(storagePath, { recursive: true, mode: 0o750 });
      writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
      assert.match(cleanupPlan.subject.storage_final_capture_path, /^\/build\/agentic-sandbox\/\.celld-qemu-cleanup\//);
      assert.equal(cleanupPlan.subject.storage_final_capture_path.startsWith(`${storageRoot}/`), false);
      assert.equal(resource.storage_final_capture_path, cleanupPlan.subject.storage_final_capture_path);
      assert.equal(resource.storage_final_capture_identity_sha256, resource.provider_storage_identity_sha256);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("the unprivileged test adapter refuses the production VM root", () => {
    const source = "/build/agentic-sandbox/vms/.celld-owned-provider.cleanup-123e4567-e89b-42d3-a456-426614174000";
    assert.throws(
      () => liveOrchestration.qemuCleanupHelperTestAdapter({
        request: {
          schema_version: "agentic-sandbox.celld-qemu-cleanup-helper/v1",
          operation: "capture-delete",
          run_id: "titan-123",
          source_path: source,
          capture_path: `/build/agentic-sandbox/.celld-qemu-cleanup/titan-123/${basename(source)}.final`,
          expected_uid: "1000",
          expected_gid: "1000",
          expected_device: "8",
          expected_inode: "9",
        },
      }),
      /unprivileged|production VM root|isolated Node test fixture/,
    );
  });

  await t.test("the root helper source fixes VM and root-only capture directories", () => {
    const helperSource = readFileSync(join(REPO_ROOT, "tools/celld-callback-relay/src/bin/agentic-celld-qemu-cleanup-helper.rs"), "utf8");
    assert.match(helperSource, /const VM_ROOT: &str = "\/build\/agentic-sandbox\/vms";/);
    assert.match(helperSource, /const CAPTURE_ROOT: &str = "\/build\/agentic-sandbox\/\.celld-qemu-cleanup";/);
    assert.match(helperSource, /ensure_root_only_directory\(Path::new\(CAPTURE_ROOT\)\)/);
    assert.match(helperSource, /require_root_directory\(Path::new\("\/build"\), false\)/);
    assert.match(helperSource, /require_root_directory\(Path::new\("\/build\/agentic-sandbox"\), false\)/);
    assert.match(helperSource, /require_root_directory\(Path::new\(VM_ROOT\), false\)/);
    assert.match(helperSource, /if owner_only \{ 0o077 \} else \{ 0o002 \}/);
    assert.match(helperSource, /metadata\.file_type\(\)\.is_symlink\(\)/);
    assert.match(helperSource, /libc::getpwuid\(sudo_uid\)/);
    assert.match(helperSource, /libc::getgrouplist/);
    assert.match(helperSource, /expected_group_is_authorized\(sudo_gid, &supplementary_gids, request\.expected_gid\)/);
    assert.match(helperSource, /gid == expected_gid && \(!is_directory \|\| uid == expected_uid\)/);
    assert.match(helperSource, /\(kind\.is_file\(\) \|\| kind\.is_symlink\(\)\) && metadata\.nlink\(\) == 1/);
  });
});

test("post-capture substitution remains fail-closed across two cleanup passes", async (t) => {
  for (const mode of ["normal", "recovery"]) {
    for (const operation of ["leaf-file", "leaf-symlink", "nested-directory"]) {
      await t.test(`${mode}: ${operation}`, async () => {
        const harness = qemuPostCaptureHarness(`red-qemu-two-pass-${mode}-${operation}`, operation, mode);
        try {
          let firstError = null;
          try { await harness.runPass(true); } catch (error) { firstError = error; }
          assert.ok(firstError instanceof liveOrchestration.OrchestrationCleanupResidueError, "the first substitution is typed cleanup residue");
          assert.equal(firstError.exitCode, 4);
          assertPostCaptureSubstitutionResidue(harness.race);
          const foreignBefore = lstatSync(harness.race.foreignPath);
          const authorizedBefore = lstatSync(harness.race.authorizedResiduePath);

          let secondError = null;
          try { await harness.runPass(false); } catch (error) { secondError = error; }
          assert.ok(secondError instanceof liveOrchestration.OrchestrationCleanupResidueError, "a second pass must retain ambiguous post-capture residue");
          assert.equal(secondError.exitCode, 4);
          const foreignAfter = lstatSync(harness.race.foreignPath);
          const authorizedAfter = lstatSync(harness.race.authorizedResiduePath);
          assert.equal(foreignAfter.dev, foreignBefore.dev);
          assert.equal(foreignAfter.ino, foreignBefore.ino, "the second pass must not delete the restored foreign entry");
          assert.equal(authorizedAfter.dev, authorizedBefore.dev);
          assert.equal(authorizedAfter.ino, authorizedBefore.ino, "the second pass must preserve the moved authorized inode");
          assert.notEqual(harness.resource.status, "removed");
          assert.ok(harness.fixture.inventory.incomplete_mutation_ids.includes(harness.cleanupPlan.mutation_id));
        } finally {
          harness.cleanup();
        }
      });
    }
  }
});

test("QEMU root final capture is deterministic, journal-owned, and restart recoverable", async (t) => {
  await t.test("the cleanup plan and materialized resource durably bind the deterministic final capture", () => {
    const fixture = exactRunFixture("red-qemu-final-capture-contract");
    const storageRoot = join(fixture.root, "vm-storage");
    const storagePath = join(storageRoot, "celld-owned-provider");
    try {
      fixture.config.vm_storage_dir = storageRoot;
      mkdirSync(storageRoot, { mode: 0o700 });
      mkdirSync(storagePath, { mode: 0o700 });
      writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
      const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
      const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
      const expectedCapture = expectedQemuFinalCapturePath(cleanupPlan);
      const expectedPhysicalCapture = physicalQemuTestCapturePath(cleanupPlan);
      const schema = JSON.parse(readFileSync(new URL("./orchestration-inventory-v2.schema.json", import.meta.url), "utf8"));
      const qemuCleanupRule = schema.$defs.journalEntry.allOf.find((rule) => rule.if?.properties?.mutation?.const === "provider_cleanup");
      for (const field of ["storage_final_capture_path", "storage_final_capture_identity_sha256"]) {
        assert.ok(schema.$defs.providerIntent.properties[field], `${field} is a typed provider intent field`);
        assert.ok(schema.$defs.resource.properties[field], `${field} is a typed materialized resource field`);
        assert.ok(qemuCleanupRule.then.properties.subject.required.includes(field), `${field} is required for a QEMU cleanup plan`);
      }
      assert.equal(cleanupPlan.subject.storage_final_capture_path, expectedCapture);
      assert.match(expectedCapture, /^\/build\/agentic-sandbox\/\.celld-qemu-cleanup\//);
      assert.equal(expectedPhysicalCapture, `${cleanupPlan.subject.storage_quarantine_path}.final`);
      assert.equal(cleanupPlan.subject.storage_final_capture_identity_sha256, resource.provider_storage_identity_sha256);
      assert.equal(resource.storage_final_capture_path, expectedCapture);
      assert.equal(resource.storage_final_capture_identity_sha256, resource.provider_storage_identity_sha256);

      commitOrchestrationInventory(fixture.config.inventory_path, fixture.inventory, { config: fixture.config });
      const durable = loadProtectedOrchestrationInventory(fixture.config.inventory_path, fixture.config, { expectedHostSha256: fixture.inventory.host_sha256 });
      const durablePlan = durable.journal.find((entry) => entry.mutation_id === cleanupPlan.mutation_id && entry.event === "planned");
      assert.equal(durablePlan.subject.storage_final_capture_path, expectedCapture);
      assert.equal(durablePlan.subject.storage_final_capture_identity_sha256, resource.provider_storage_identity_sha256);
    } finally {
      fixture.cleanup();
    }
  });

  await t.test("restart rediscovers and removes the exact inode after a crash immediately following root capture", async () => {
    const harness = qemuPostCaptureHarness("red-qemu-final-capture-crash", "quarantine-root", "normal");
    const expectedCapture = expectedQemuFinalCapturePath(harness.cleanupPlan);
    const expectedPhysicalCapture = physicalQemuTestCapturePath(harness.cleanupPlan);
    let crashHookReached = false;
    let capturedIdentity = null;
    try {
      const crashHook = ({ operation, originalPath, capturedPath, logicalCapturePath, device, inode }) => {
        if (operation !== "rmdir" || originalPath !== harness.quarantinePath) return;
        crashHookReached = true;
        assert.equal(logicalCapturePath, expectedCapture, "helper request uses the deterministic plan-owned pathname");
        assert.equal(capturedPath, expectedPhysicalCapture, "the test adapter uses a local physical capture stand-in");
        const metadata = lstatSync(capturedPath);
        assert.equal(String(metadata.dev), device);
        assert.equal(String(metadata.ino), inode);
        capturedIdentity = { dev: metadata.dev, ino: metadata.ino };
        const durable = loadProtectedOrchestrationInventory(
          harness.fixture.config.inventory_path,
          harness.fixture.config,
          { expectedHostSha256: harness.fixture.inventory.host_sha256 },
        );
        const durablePlan = durable.journal.find((entry) => entry.event === "planned" && entry.mutation_id === harness.cleanupPlan.mutation_id);
        assert.equal(durablePlan.subject.storage_final_capture_path, logicalCapturePath, "capture authority is durable before the rename-delete window");
        assert.equal(durablePlan.subject.storage_final_capture_identity_sha256, harness.resource.provider_storage_identity_sha256);
        throw new Error("simulated crash after journal-owned root capture");
      };
      let crashError = null;
      try { await harness.runPassWithHook(crashHook); } catch (error) { crashError = error; }
      assert.equal(crashHookReached, true, "the crash seam must be after capture and before rmdir");
      assert.ok(crashError instanceof liveOrchestration.OrchestrationCleanupResidueError);
      const captured = lstatSync(expectedPhysicalCapture);
      assert.equal(captured.dev, capturedIdentity.dev);
      assert.equal(captured.ino, capturedIdentity.ino, "the crash leaves the exact captured inode discoverable");
      assert.equal(existsSync(harness.quarantinePath), false);
      assert.notEqual(harness.resource.status, "removed");

      const restartedInventory = loadProtectedOrchestrationInventory(
        harness.fixture.config.inventory_path,
        harness.fixture.config,
        { expectedHostSha256: harness.fixture.inventory.host_sha256 },
      );
      harness.fixture.inventory = restartedInventory;
      const restartedRecord = restartedInventory.resources.find((entry) => entry.instance_id === harness.resource.instance_id);
      const restartedRuntime = runtimeFor(harness.fixture, restartedRecord);
      delete restartedRuntime.persistInventory;
      restartedRuntime.persistedJournalHeadSha256 = restartedInventory.journal_head_sha256;
      const result = await liveOrchestration.recoverOrchestrationInventory(restartedRuntime, {
        observeProviderResource: async () => persistedQemuObservation(restartedRecord, harness.storagePath, { present: false }),
        removeProviderResource: async () => { throw new Error("restart must reconcile the journal-owned capture without replaying provider deletion"); },
      });
      assert.equal(result.status, "PASS");
      assert.equal(existsSync(expectedPhysicalCapture), false, "restart removes the exact journal-owned captured inode");
      assert.equal(restartedRecord.status, "removed");
      assert.equal(restartedInventory.incomplete_mutation_ids.includes(harness.cleanupPlan.mutation_id), false);
    } finally {
      harness.cleanup();
    }
  });

  for (const vector of [
    { name: "unknown final-capture candidate", kind: "unknown" },
    { name: "foreign inode at the journal-bound capture pathname", kind: "foreign" },
  ]) {
    await t.test(vector.name, async () => {
      const fixture = exactRunFixture(`red-qemu-final-capture-${vector.kind}`);
      const storageRoot = join(fixture.root, "vm-storage");
      const storagePath = join(storageRoot, "celld-owned-provider");
      try {
        fixture.config.vm_storage_dir = storageRoot;
        mkdirSync(storageRoot, { mode: 0o700 });
        mkdirSync(storagePath, { mode: 0o700 });
        writeFileSync(join(storagePath, "disk.qcow2"), "authorized storage\n", { mode: 0o600 });
        const { resource } = addObservedQemuProvider(fixture.inventory, storagePath);
        const cleanupPlan = addPendingCleanup(fixture.inventory, resource).entry;
        const expectedCapture = physicalQemuTestCapturePath(cleanupPlan);
        const runtime = durableRuntimeFor(fixture, resource);
        let authorizedPath;
        if (vector.kind === "unknown") {
          authorizedPath = `${expectedCapture}.unknown`;
          renameSync(storagePath, authorizedPath);
        } else {
          authorizedPath = `${expectedCapture}.authorized-residue`;
          renameSync(storagePath, authorizedPath);
          mkdirSync(expectedCapture, { mode: 0o700 });
          writeFileSync(join(expectedCapture, "foreign-marker"), "foreign final capture\n", { mode: 0o600 });
        }

        let recoveryError = null;
        try {
          await liveOrchestration.recoverOrchestrationInventory(runtime, {
            observeProviderResource: async () => persistedQemuObservation(resource, storagePath, { present: false }),
            removeProviderResource: async () => { throw new Error("candidate classification must not replay provider deletion"); },
          });
        } catch (error) {
          recoveryError = error;
        }
        assert.ok(recoveryError instanceof liveOrchestration.OrchestrationCleanupResidueError, "unknown or substituted final captures are fail-closed residue");
        assert.equal(recoveryError.exitCode, 4);
        assert.equal(existsSync(authorizedPath), true, "authorized residue is preserved");
        if (vector.kind === "foreign") assert.equal(existsSync(join(expectedCapture, "foreign-marker")), true, "foreign candidate is preserved");
        assert.notEqual(resource.status, "removed");
        assert.ok(fixture.inventory.incomplete_mutation_ids.includes(cleanupPlan.mutation_id));
      } finally {
        fixture.cleanup();
      }
    });
  }
});
