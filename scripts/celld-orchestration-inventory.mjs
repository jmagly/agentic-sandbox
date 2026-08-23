import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fstatSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  rmdirSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";

export const ORCHESTRATION_INVENTORY_V1 = "agentic-sandbox.celld-orchestration-inventory/v1";
export const ORCHESTRATION_INVENTORY_V2 = "agentic-sandbox.celld-orchestration-inventory/v2";
export const ORCHESTRATION_ROOT = "/dev/shm/agentic-celld-orchestration";
export const QEMU_CLEANUP_CAPTURE_ROOT = "/build/agentic-sandbox/.celld-qemu-cleanup";
export const ORCHESTRATION_OWNER = Object.freeze({ repository: "roctinam/agentic-sandbox", workflow: "celld-qualification.yml" });

const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const DECIMAL_ID = /^(?:0|[1-9][0-9]{0,19})$/;
const NONZERO_DECIMAL_ID = /^[1-9][0-9]{0,19}$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const FAULT_ID = /^[0-9a-f]{32}$/;
const RESOURCE_NAME = /^celld-[a-z0-9-]{1,62}$/;
const FAULT_TARGET = /^(?:management|celld-[a-z0-9-]{1,80})$/;
const DOCKER_ID = /^[0-9a-f]{64}$/;
const QEMU_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NETWORK_ID = /^[0-9a-f]{12,64}$/;
const SCENARIOS = new Set(["UAT-CELLD-003", "UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"]);
const SUBSTRATES = new Set(["qemu", "docker"]);
const FAULT_KINDS = new Set(["management_process_kill", "fleet_node_stop", "callback_response_loss", "callback_relay_pause"]);
const PROVIDER_ACTIONS = new Set(["provision", "start", "stop", "destroy", "cleanup"]);
const MUTATIONS = new Set(["provider_action", "provider_cleanup", "fault_apply", "fault_heal"]);
const EVENTS = new Set(["planned", "completed", "recovered"]);
const OUTCOMES = new Set(["acknowledged", "effect_observed", "applied", "healed", "absent", "not_observed"]);
const MAX_INVENTORY_BYTES = 16 * 1024 * 1024;
const MAX_PROTECTED_JSON_BYTES = 1024 * 1024;

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function canonicalOrchestrationJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("orchestration inventory contains a non-finite number");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalOrchestrationJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalOrchestrationJson(value[key])}`).join(",")}}`;
  }
  throw new Error("orchestration inventory contains an unsupported JSON value");
}

function validTimestamp(value) {
  return typeof value === "string" && Number.isFinite(Date.parse(value));
}

function ownObject(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

function rejectUnknown(errors, value, allowed, context) {
  for (const key of Object.keys(value ?? {})) if (!allowed.has(key)) errors.push(`${context}.${key} is not allowed`);
}

function processIdentity() {
  return {
    uid: typeof process.getuid === "function" ? process.getuid() : null,
    gid: typeof process.getgid === "function" ? process.getgid() : null,
  };
}

function validateV1(inventory, config, { expectedHostSha256 } = {}) {
  const errors = [];
  const allowed = new Set(["schema_version", "run_id", "working_root", "owner", "host_sha256", "created_at", "updated_at", "state", "resources", "faults"]);
  rejectUnknown(errors, inventory, allowed, "inventory");
  if (inventory.schema_version !== ORCHESTRATION_INVENTORY_V1) errors.push(`inventory.schema_version must be ${ORCHESTRATION_INVENTORY_V1}`);
  if (inventory.run_id !== config.run_id
      || inventory.working_root !== config.working_root
      || inventory.owner?.repository !== ORCHESTRATION_OWNER.repository
      || inventory.owner?.workflow !== ORCHESTRATION_OWNER.workflow
      || inventory.owner?.run_id !== config.run_id) errors.push("inventory run/owner does not match orchestration config");
  if (!SHA256.test(inventory.host_sha256 ?? "") || (expectedHostSha256 && inventory.host_sha256 !== expectedHostSha256)) errors.push("inventory host does not match the authorized host");
  if (!validTimestamp(inventory.created_at) || !validTimestamp(inventory.updated_at)) errors.push("inventory timestamps are invalid");
  if (!["prepared", "active", "cleanup_residue", "clean"].includes(inventory.state)) errors.push("inventory state is invalid");
  if (!Array.isArray(inventory.resources) || !Array.isArray(inventory.faults)) return [...errors, "inventory resources/faults must be arrays"];

  const resourceKeys = new Set();
  for (const [index, resource] of inventory.resources.entries()) {
    const allowedResource = new Set(["scenario_id", "instance_id", "name", "substrate", "status", "planned_at", "updated_at", "removed_at"]);
    rejectUnknown(errors, resource, allowedResource, `inventory.resources[${index}]`);
    const key = resource?.instance_id ?? "";
    if (resourceKeys.has(key)) errors.push(`inventory resource is duplicated: ${key}`);
    resourceKeys.add(key);
    if (!SCENARIOS.has(resource?.scenario_id)
        || !UUID.test(resource?.instance_id ?? "")
        || !RESOURCE_NAME.test(resource?.name ?? "")
        || !SUBSTRATES.has(resource?.substrate)
        || !["planned", "removed"].includes(resource?.status)
        || !validTimestamp(resource?.planned_at)
        || !validTimestamp(resource?.updated_at)
        || (resource?.status === "removed" && !validTimestamp(resource?.removed_at))) errors.push(`inventory resource is invalid at index ${index}`);
  }

  const faultIds = new Set();
  for (const [index, fault] of inventory.faults.entries()) {
    const allowedFault = new Set(["id", "scenario_id", "kind", "target", "status", "planned_at", "updated_at", "applied_at", "healed_at"]);
    rejectUnknown(errors, fault, allowedFault, `inventory.faults[${index}]`);
    if (faultIds.has(fault?.id)) errors.push(`inventory fault is duplicated: ${fault?.id}`);
    faultIds.add(fault?.id);
    if (!FAULT_ID.test(fault?.id ?? "")
        || !SCENARIOS.has(fault?.scenario_id)
        || !FAULT_KINDS.has(fault?.kind)
        || !FAULT_TARGET.test(fault?.target ?? "")
        || !["planned", "applied", "healed"].includes(fault?.status)
        || !validTimestamp(fault?.planned_at)
        || !validTimestamp(fault?.updated_at)
        || (["applied", "healed"].includes(fault?.status) && !validTimestamp(fault?.applied_at))
        || (fault?.status === "healed" && !validTimestamp(fault?.healed_at))) errors.push(`inventory fault is invalid at index ${index}`);
  }
  return errors;
}

function validateProviderIntent(subject, errors, context) {
  const allowed = new Set([
    "instance_id", "name", "substrate", "operation_id", "generation", "action", "request_sha256",
    "storage_quarantine_path", "storage_quarantine_identity_sha256",
    "storage_final_capture_path", "storage_final_capture_identity_sha256",
    "storage_expected_device", "storage_expected_inode", "storage_expected_uid", "storage_expected_gid",
  ]);
  rejectUnknown(errors, subject, allowed, context);
  if (!ownObject(subject)
      || !UUID.test(subject?.instance_id ?? "")
      || !RESOURCE_NAME.test(subject?.name ?? "")
      || !SUBSTRATES.has(subject?.substrate)
      || typeof subject?.operation_id !== "string" || subject.operation_id.length < 1 || subject.operation_id.length > 256
      || !Number.isSafeInteger(subject?.generation) || subject.generation < 1
      || !PROVIDER_ACTIONS.has(subject?.action)
      || !SHA256.test(subject?.request_sha256 ?? "")
      || (subject?.storage_quarantine_path !== undefined && !isAbsolute(subject.storage_quarantine_path))
      || (subject?.storage_quarantine_identity_sha256 !== undefined && !SHA256.test(subject.storage_quarantine_identity_sha256))
      || (subject?.storage_final_capture_path !== undefined && !isAbsolute(subject.storage_final_capture_path))
      || (subject?.storage_final_capture_identity_sha256 !== undefined && !SHA256.test(subject.storage_final_capture_identity_sha256))
      || (subject?.storage_expected_device !== undefined && !DECIMAL_ID.test(subject.storage_expected_device))
      || (subject?.storage_expected_inode !== undefined && !NONZERO_DECIMAL_ID.test(subject.storage_expected_inode))
      || (subject?.storage_expected_uid !== undefined && !DECIMAL_ID.test(subject.storage_expected_uid))
      || (subject?.storage_expected_gid !== undefined && !DECIMAL_ID.test(subject.storage_expected_gid))
      || ((subject?.storage_quarantine_path === undefined) !== (subject?.storage_quarantine_identity_sha256 === undefined))
      || ((subject?.storage_final_capture_path === undefined) !== (subject?.storage_final_capture_identity_sha256 === undefined))) errors.push(`${context} is invalid`);
}

function validateFaultIntent(subject, errors, context) {
  const allowed = new Set(["fault_id", "kind", "target", "target_identity_sha256", "target_ownership_sha256"]);
  rejectUnknown(errors, subject, allowed, context);
  if (!ownObject(subject)
      || !FAULT_ID.test(subject?.fault_id ?? "")
      || !FAULT_KINDS.has(subject?.kind)
      || !FAULT_TARGET.test(subject?.target ?? "")
      || !SHA256.test(subject?.target_identity_sha256 ?? "")
      || !SHA256.test(subject?.target_ownership_sha256 ?? "")) errors.push(`${context} requires exact identity and ownership bindings`);
}

function providerKey(subject) {
  return subject?.instance_id ?? "";
}

function faultKey(subject) {
  return subject?.fault_id ?? "";
}

function validProviderId(substrate, value) {
  return (substrate === "docker" ? DOCKER_ID : QEMU_UUID).test(value ?? "");
}

function validateTargetTransition(value, errors, context) {
  if (!ownObject(value)
      || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify([
        "kind", "previous_identity_sha256", "replacement_identity_sha256", "replacement_ownership_sha256", "replacement_provider_id",
      ].sort())
      || value.kind !== "management_process_replacement"
      || !SHA256.test(value.previous_identity_sha256 ?? "")
      || !SHA256.test(value.replacement_identity_sha256 ?? "")
      || !SHA256.test(value.replacement_ownership_sha256 ?? "")
      || !/^[1-9][0-9]*$/.test(value.replacement_provider_id ?? "")) errors.push(`${context} is invalid`);
}

function validateTerminalSemantics(plan, entry, errors, context, { validateEvidence = true } = {}) {
  const outcomes = {
    provider_action: new Set(["effect_observed", "absent"]),
    provider_cleanup: new Set(["absent"]),
    fault_apply: new Set(["applied", "not_observed"]),
    fault_heal: new Set(["healed"]),
  }[plan?.mutation];
  if (!outcomes?.has(entry.outcome)) {
    errors.push(`${context}.outcome is not semantically valid for ${plan?.mutation ?? "the mutation"}`);
    return;
  }
  if (plan.mutation === "provider_action") {
    const expectsAbsence = plan.subject.action === "destroy";
    if ((entry.outcome === "absent") !== expectsAbsence && entry.event === "completed") {
      errors.push(`${context}.outcome does not match the provider action semantic terminal state`);
    }
  }
  if (entry.outcome === "effect_observed") {
    if (validateEvidence && (!validProviderId(plan.subject.substrate, entry.observed_provider_id)
        || !SHA256.test(entry.observed_identity_sha256 ?? "")
        || !SHA256.test(entry.observed_configuration_sha256 ?? "")
        || !SHA256.test(entry.observed_ownership_binding_sha256 ?? ""))) {
      errors.push(`${context} effect_observed outcome requires exact provider, identity, configuration, and ownership bindings`);
    }
    if (entry.observed_provider_id !== undefined && !validProviderId(plan.subject.substrate, entry.observed_provider_id)) errors.push(`${context}.observed_provider_id is invalid`);
    for (const field of [
      "observed_ownership_binding_sha256", "observed_managed_network_identity_sha256",
      "observed_managed_network_configuration_sha256", "observed_provider_storage_identity_sha256",
    ]) if (entry[field] !== undefined && !SHA256.test(entry[field])) errors.push(`${context}.${field} is invalid`);
    if (entry.observed_managed_network_id !== undefined && !NETWORK_ID.test(entry.observed_managed_network_id)) errors.push(`${context}.observed_managed_network_id is invalid`);
    if (entry.observed_storage_path !== undefined && !isAbsolute(entry.observed_storage_path)) errors.push(`${context}.observed_storage_path is invalid`);
    if (entry.observed_storage_device !== undefined && !DECIMAL_ID.test(entry.observed_storage_device)) errors.push(`${context}.observed_storage_device is invalid`);
    if (entry.observed_storage_inode !== undefined && !NONZERO_DECIMAL_ID.test(entry.observed_storage_inode)) errors.push(`${context}.observed_storage_inode is invalid`);
    if (entry.observed_storage_uid !== undefined && !DECIMAL_ID.test(entry.observed_storage_uid)) errors.push(`${context}.observed_storage_uid is invalid`);
    if (entry.observed_storage_gid !== undefined && !DECIMAL_ID.test(entry.observed_storage_gid)) errors.push(`${context}.observed_storage_gid is invalid`);
    if (validateEvidence && plan.subject.substrate === "docker"
        && (!NETWORK_ID.test(entry.observed_managed_network_id ?? "")
          || !SHA256.test(entry.observed_managed_network_identity_sha256 ?? "")
          || !SHA256.test(entry.observed_managed_network_configuration_sha256 ?? ""))) {
      errors.push(`${context} Docker effect_observed outcome requires exact managed-network bindings`);
    }
    if (validateEvidence && plan.subject.substrate === "qemu"
        && (!SHA256.test(entry.observed_provider_storage_identity_sha256 ?? "")
          || !isAbsolute(entry.observed_storage_path ?? "")
          || !DECIMAL_ID.test(entry.observed_storage_device ?? "")
          || !NONZERO_DECIMAL_ID.test(entry.observed_storage_inode ?? "")
          || !DECIMAL_ID.test(entry.observed_storage_uid ?? "")
          || !DECIMAL_ID.test(entry.observed_storage_gid ?? ""))) {
      errors.push(`${context} QEMU effect_observed outcome requires exact storage bindings`);
    }
  } else if (entry.subject_type === "provider_resource"
      && (entry.observed_identity_sha256 !== null || entry.observed_configuration_sha256 !== null)) {
    errors.push(`${context} absent provider outcome cannot claim an observed provider identity`);
  }
  if (entry.target_transition !== undefined) {
    if (plan.mutation !== "fault_heal" || plan.subject.kind !== "management_process_kill" || entry.outcome !== "healed") {
      errors.push(`${context}.target_transition is not valid for this mutation`);
    } else validateTargetTransition(entry.target_transition, errors, `${context}.target_transition`);
  }
}

function replayV2Journal(inventory, errors) {
  const plans = new Map();
  const terminals = new Set();
  const expectedResources = new Map();
  const expectedFaults = new Map();
  let previous = null;

  for (const [index, entry] of inventory.journal.entries()) {
    const context = `inventory.journal[${index}]`;
    const allowed = new Set([
      "sequence", "mutation_id", "event", "mutation", "scenario_id", "subject_type", "subject",
      "plan_sequence", "outcome", "observed_identity_sha256", "observed_configuration_sha256",
      "observed_provider_id", "observed_ownership_binding_sha256", "observed_managed_network_id",
      "observed_managed_network_identity_sha256", "observed_managed_network_configuration_sha256",
      "observed_provider_storage_identity_sha256", "observed_storage_path", "observed_storage_device",
      "observed_storage_inode", "observed_storage_uid", "observed_storage_gid", "target_transition",
      "recorded_at", "previous_entry_sha256", "entry_sha256",
    ]);
    rejectUnknown(errors, entry, allowed, context);
    if (!ownObject(entry)) {
      errors.push(`${context} must be an object`);
      continue;
    }
    const { entry_sha256: observedHash, ...hashed } = entry;
    if (entry.sequence !== index + 1) errors.push(`${context}.sequence is not contiguous`);
    if (entry.previous_entry_sha256 !== previous) errors.push(`${context}.previous_entry_sha256 breaks the hash chain`);
    if (!SHA256.test(observedHash ?? "") || sha256(canonicalOrchestrationJson(hashed)) !== observedHash) errors.push(`${context}.entry_sha256 is invalid`);
    previous = observedHash;
    if (!UUID.test(entry.mutation_id ?? "") || !EVENTS.has(entry.event) || !MUTATIONS.has(entry.mutation)
        || !SCENARIOS.has(entry.scenario_id) || !["provider_resource", "fault"].includes(entry.subject_type)
        || !validTimestamp(entry.recorded_at)) errors.push(`${context} header is invalid`);
    if (entry.subject_type === "provider_resource") validateProviderIntent(entry.subject, errors, `${context}.subject`);
    else if (entry.subject_type === "fault") validateFaultIntent(entry.subject, errors, `${context}.subject`);
    if (["provider_action", "provider_cleanup"].includes(entry.mutation) !== (entry.subject_type === "provider_resource")) errors.push(`${context} mutation and subject type disagree`);
    if (["fault_apply", "fault_heal"].includes(entry.mutation) !== (entry.subject_type === "fault")) errors.push(`${context} mutation and subject type disagree`);
    if (entry.mutation === "provider_cleanup" && entry.subject?.substrate === "qemu") {
      const expectedQuarantinePath = join(dirname(entry.subject?.storage_quarantine_path ?? "/invalid"), `.${entry.subject?.name}.cleanup-${entry.mutation_id}`);
      const captureName = `${basename(expectedQuarantinePath)}.final`;
      const expectedCapturePath = join(QEMU_CLEANUP_CAPTURE_ROOT, inventory.run_id, captureName);
      if (!SHA256.test(entry.subject?.storage_quarantine_identity_sha256 ?? "")
          || !isAbsolute(entry.subject?.storage_quarantine_path ?? "")
          || entry.subject.storage_quarantine_path !== expectedQuarantinePath
          || !SHA256.test(entry.subject?.storage_final_capture_identity_sha256 ?? "")
          || entry.subject.storage_final_capture_identity_sha256 !== entry.subject.storage_quarantine_identity_sha256
          || !isAbsolute(entry.subject?.storage_final_capture_path ?? "")
          || entry.subject.storage_final_capture_path !== expectedCapturePath
          || !DECIMAL_ID.test(entry.subject?.storage_expected_device ?? "")
          || !NONZERO_DECIMAL_ID.test(entry.subject?.storage_expected_inode ?? "")
          || !DECIMAL_ID.test(entry.subject?.storage_expected_uid ?? "")
          || !DECIMAL_ID.test(entry.subject?.storage_expected_gid ?? "")) {
        errors.push(`${context}.subject requires the exact deterministic QEMU storage quarantine binding`);
      }
    }

    if (entry.event === "planned") {
      if (plans.has(entry.mutation_id)) errors.push(`${context} duplicates a mutation plan`);
      if (entry.plan_sequence !== undefined || entry.outcome !== undefined || [
        "observed_identity_sha256", "observed_configuration_sha256", "observed_provider_id", "observed_ownership_binding_sha256",
        "observed_managed_network_id", "observed_managed_network_identity_sha256", "observed_managed_network_configuration_sha256",
        "observed_provider_storage_identity_sha256", "observed_storage_path", "observed_storage_device",
        "observed_storage_inode", "observed_storage_uid", "observed_storage_gid", "target_transition",
      ].some((field) => entry[field] !== undefined)) errors.push(`${context} planned entry contains terminal fields`);
      plans.set(entry.mutation_id, entry);
    } else {
      const plan = plans.get(entry.mutation_id);
      if (!plan || terminals.has(entry.mutation_id)) errors.push(`${context} has no unique preceding plan`);
      else {
        if (entry.plan_sequence !== plan.sequence
            || entry.mutation !== plan.mutation
            || entry.scenario_id !== plan.scenario_id
            || entry.subject_type !== plan.subject_type
            || canonicalOrchestrationJson(entry.subject) !== canonicalOrchestrationJson(plan.subject)) errors.push(`${context} does not match its plan`);
        terminals.add(entry.mutation_id);
      }
      if (!OUTCOMES.has(entry.outcome)) errors.push(`${context}.outcome is invalid`);
      if (entry.observed_identity_sha256 !== undefined && entry.observed_identity_sha256 !== null && !SHA256.test(entry.observed_identity_sha256)) errors.push(`${context}.observed_identity_sha256 is invalid`);
      if (entry.observed_configuration_sha256 !== undefined && entry.observed_configuration_sha256 !== null && !SHA256.test(entry.observed_configuration_sha256)) errors.push(`${context}.observed_configuration_sha256 is invalid`);
      if (plan) validateTerminalSemantics(plan, entry, errors, context);
    }

    if (entry.subject_type === "provider_resource") {
      const key = providerKey(entry.subject);
      let expected = expectedResources.get(key);
      if (!expected) {
        if (entry.event !== "planned" || entry.mutation !== "provider_action" || entry.subject.action !== "provision") {
          errors.push(`${context} references a provider resource before its provision plan`);
        }
        expected = {
          scenario_id: entry.scenario_id,
          instance_id: entry.subject.instance_id,
          name: entry.subject.name,
          substrate: entry.subject.substrate,
          status: "planned",
          last_sequence: entry.sequence,
          planned_at: entry.recorded_at,
        };
        expectedResources.set(key, expected);
      }
      if (expected.name !== entry.subject.name || expected.substrate !== entry.subject.substrate) errors.push(`${context} changes a materialized provider identity`);
      if (entry.event === "planned" && entry.mutation === "provider_cleanup") {
        expected.status = "cleanup_pending";
        if (entry.subject.substrate === "qemu") {
          expected.storage_quarantine_path = entry.subject.storage_quarantine_path;
          expected.storage_quarantine_identity_sha256 = entry.subject.storage_quarantine_identity_sha256;
          expected.storage_final_capture_path = entry.subject.storage_final_capture_path;
          expected.storage_final_capture_identity_sha256 = entry.subject.storage_final_capture_identity_sha256;
          expected.storage_expected_device = entry.subject.storage_expected_device;
          expected.storage_expected_inode = entry.subject.storage_expected_inode;
          expected.storage_expected_uid = entry.subject.storage_expected_uid;
          expected.storage_expected_gid = entry.subject.storage_expected_gid;
        }
      }
      if (entry.event === "planned" && entry.mutation === "provider_action" && entry.subject.action === "provision") {
        if (expected.status === "removed") {
          delete expected.provider_id;
          delete expected.provider_identity_sha256;
          delete expected.configuration_sha256;
          delete expected.ownership_binding_sha256;
          delete expected.managed_network_id;
          delete expected.managed_network_identity_sha256;
          delete expected.managed_network_configuration_sha256;
          delete expected.provider_storage_identity_sha256;
          delete expected.storage_path;
          delete expected.storage_quarantine_path;
          delete expected.storage_quarantine_identity_sha256;
          delete expected.storage_final_capture_path;
          delete expected.storage_final_capture_identity_sha256;
          delete expected.storage_expected_device;
          delete expected.storage_expected_inode;
          delete expected.storage_expected_uid;
          delete expected.storage_expected_gid;
        }
        expected.status = "planned";
      }
      if (entry.event !== "planned" && entry.mutation === "provider_action") expected.status = entry.outcome === "absent" ? "removed" : "active";
      if (entry.event !== "planned" && entry.mutation === "provider_cleanup") expected.status = "removed";
      if (entry.event !== "planned" && entry.outcome === "effect_observed") {
        expected.provider_id = entry.observed_provider_id;
        expected.provider_identity_sha256 = entry.observed_identity_sha256;
        expected.configuration_sha256 = entry.observed_configuration_sha256;
        expected.ownership_binding_sha256 = entry.observed_ownership_binding_sha256;
        expected.managed_network_id = entry.observed_managed_network_id;
        expected.managed_network_identity_sha256 = entry.observed_managed_network_identity_sha256;
        expected.managed_network_configuration_sha256 = entry.observed_managed_network_configuration_sha256;
        expected.provider_storage_identity_sha256 = entry.observed_provider_storage_identity_sha256;
        expected.storage_path = entry.observed_storage_path;
        expected.storage_device = entry.observed_storage_device;
        expected.storage_inode = entry.observed_storage_inode;
        expected.storage_uid = entry.observed_storage_uid;
        expected.storage_gid = entry.observed_storage_gid;
      }
      expected.last_sequence = entry.sequence;
      expected.updated_at = entry.recorded_at;
      expected.removed_at = expected.status === "removed" ? entry.recorded_at : undefined;
    } else if (entry.subject_type === "fault") {
      const key = faultKey(entry.subject);
      let expected = expectedFaults.get(key);
      if (!expected) {
        if (entry.event !== "planned" || entry.mutation !== "fault_apply") errors.push(`${context} references a fault before its apply plan`);
        expected = {
          id: entry.subject.fault_id,
          scenario_id: entry.scenario_id,
          kind: entry.subject.kind,
          target: entry.subject.target,
          ...(entry.subject.target_identity_sha256 ? { target_identity_sha256: entry.subject.target_identity_sha256 } : {}),
          ...(entry.subject.target_ownership_sha256 ? { target_ownership_sha256: entry.subject.target_ownership_sha256 } : {}),
          status: "planned",
          last_sequence: entry.sequence,
          planned_at: entry.recorded_at,
        };
        expectedFaults.set(key, expected);
      }
      if (expected.kind !== entry.subject.kind || expected.target !== entry.subject.target) errors.push(`${context} changes a materialized fault identity`);
      if (entry.event === "planned" && entry.mutation === "fault_heal") expected.status = "heal_pending";
      if (entry.event !== "planned" && entry.mutation === "fault_apply") expected.status = entry.outcome === "applied" ? "applied" : "healed";
      if (entry.event !== "planned" && entry.mutation === "fault_heal") expected.status = "healed";
      expected.last_sequence = entry.sequence;
      expected.updated_at = entry.recorded_at;
      if (entry.event !== "planned" && entry.mutation === "fault_apply" && entry.outcome === "applied") expected.applied_at = entry.recorded_at;
      if (expected.status === "healed") expected.healed_at = entry.recorded_at;
    }
  }
  return { plans, terminals, expectedResources, expectedFaults, head: previous };
}

function validateV2(inventory, config, { expectedHostSha256, expectedUid, expectedGid } = {}) {
  const errors = [];
  const allowed = new Set([
    "schema_version", "run_id", "working_root", "owner", "host_sha256", "created_at", "updated_at", "state",
    "last_sequence", "journal_head_sha256", "incomplete_mutation_ids", "resources", "faults", "journal",
  ]);
  rejectUnknown(errors, inventory, allowed, "inventory");
  if (inventory.schema_version !== ORCHESTRATION_INVENTORY_V2) errors.push(`inventory.schema_version must be ${ORCHESTRATION_INVENTORY_V2}`);
  const root = resolve(inventory.working_root ?? "");
  if (!RUN_ID.test(inventory.run_id ?? "") || inventory.run_id !== config.run_id
      || inventory.working_root !== config.working_root
      || !root.startsWith(`${ORCHESTRATION_ROOT}${sep}`)
      || basename(root) !== inventory.run_id) errors.push("inventory run/root does not match the exact orchestration config");
  const ownerKeys = Object.keys(inventory.owner ?? {}).sort();
  if (JSON.stringify(ownerKeys) !== JSON.stringify(["gid", "repository", "run_id", "uid", "workflow"])
      || inventory.owner?.repository !== ORCHESTRATION_OWNER.repository
      || inventory.owner?.workflow !== ORCHESTRATION_OWNER.workflow
      || inventory.owner?.run_id !== config.run_id
      || !Number.isSafeInteger(inventory.owner?.uid) || inventory.owner.uid < 0
      || !Number.isSafeInteger(inventory.owner?.gid) || inventory.owner.gid < 0
      || (expectedUid !== undefined && expectedUid !== null && inventory.owner.uid !== expectedUid)
      || (expectedGid !== undefined && expectedGid !== null && inventory.owner.gid !== expectedGid)) errors.push("inventory owner does not match the exact process owner");
  if (!SHA256.test(inventory.host_sha256 ?? "") || (expectedHostSha256 && inventory.host_sha256 !== expectedHostSha256)) errors.push("inventory host does not match the authorized host");
  if (!validTimestamp(inventory.created_at) || !validTimestamp(inventory.updated_at)) errors.push("inventory timestamps are invalid");
  if (!["prepared", "active", "recovering", "cleanup_residue", "clean"].includes(inventory.state)) errors.push("inventory state is invalid");
  if (!Number.isSafeInteger(inventory.last_sequence) || inventory.last_sequence < 0) errors.push("inventory.last_sequence is invalid");
  if (inventory.journal_head_sha256 !== null && !SHA256.test(inventory.journal_head_sha256 ?? "")) errors.push("inventory.journal_head_sha256 is invalid");
  if (!Array.isArray(inventory.incomplete_mutation_ids) || !Array.isArray(inventory.resources) || !Array.isArray(inventory.faults) || !Array.isArray(inventory.journal)) {
    return [...errors, "inventory v2 journal/materialized collections must be arrays"];
  }
  if (new Set(inventory.incomplete_mutation_ids).size !== inventory.incomplete_mutation_ids.length
      || inventory.incomplete_mutation_ids.some((id) => !UUID.test(id ?? ""))) errors.push("inventory.incomplete_mutation_ids is invalid");

  const replay = replayV2Journal(inventory, errors);
  if (inventory.last_sequence !== inventory.journal.length) errors.push("inventory.last_sequence does not match the journal");
  if (inventory.journal_head_sha256 !== replay.head) errors.push("inventory.journal_head_sha256 does not match the journal");
  const expectedIncomplete = [...replay.plans.values()].filter((plan) => !replay.terminals.has(plan.mutation_id)).map((plan) => plan.mutation_id);
  if (JSON.stringify(inventory.incomplete_mutation_ids) !== JSON.stringify(expectedIncomplete)) errors.push("inventory.incomplete_mutation_ids does not match journal replay");

  const resourceIds = new Set();
  for (const [index, resource] of inventory.resources.entries()) {
    const context = `inventory.resources[${index}]`;
    const allowedResource = new Set([
      "scenario_id", "instance_id", "name", "substrate", "status", "provider_id", "provider_identity_sha256", "configuration_sha256",
      "ownership_binding_sha256", "managed_network_id", "managed_network_identity_sha256", "managed_network_configuration_sha256",
      "provider_storage_identity_sha256", "storage_path", "storage_quarantine_path", "storage_quarantine_identity_sha256",
      "storage_final_capture_path", "storage_final_capture_identity_sha256",
      "storage_device", "storage_inode", "storage_uid", "storage_gid",
      "storage_expected_device", "storage_expected_inode", "storage_expected_uid", "storage_expected_gid",
      "last_sequence", "planned_at", "updated_at", "removed_at",
    ]);
    rejectUnknown(errors, resource, allowedResource, context);
    const expected = replay.expectedResources.get(resource?.instance_id);
    if (resourceIds.has(resource?.instance_id)) errors.push(`${context} is duplicated`);
    resourceIds.add(resource?.instance_id);
    if (!expected
        || !SCENARIOS.has(resource?.scenario_id)
        || !UUID.test(resource?.instance_id ?? "")
        || !RESOURCE_NAME.test(resource?.name ?? "")
        || !SUBSTRATES.has(resource?.substrate)
        || !["planned", "active", "cleanup_pending", "removed"].includes(resource?.status)
        || !Number.isSafeInteger(resource?.last_sequence) || resource.last_sequence < 1
        || (resource?.provider_id !== undefined && !validProviderId(resource.substrate, resource.provider_id))
        || (resource?.provider_identity_sha256 !== undefined && !SHA256.test(resource.provider_identity_sha256))
        || (resource?.configuration_sha256 !== undefined && !SHA256.test(resource.configuration_sha256))
        || [
          "ownership_binding_sha256", "managed_network_identity_sha256", "managed_network_configuration_sha256",
          "provider_storage_identity_sha256", "storage_quarantine_identity_sha256",
          "storage_final_capture_identity_sha256",
        ]
          .some((field) => resource?.[field] !== undefined && !SHA256.test(resource[field]))
        || (resource?.managed_network_id !== undefined && !NETWORK_ID.test(resource.managed_network_id))
        || (resource?.storage_path !== undefined && !isAbsolute(resource.storage_path))
        || (resource?.storage_quarantine_path !== undefined && !isAbsolute(resource.storage_quarantine_path))
        || (resource?.storage_final_capture_path !== undefined && !isAbsolute(resource.storage_final_capture_path))
        || (resource?.storage_device !== undefined && !DECIMAL_ID.test(resource.storage_device))
        || (resource?.storage_inode !== undefined && !NONZERO_DECIMAL_ID.test(resource.storage_inode))
        || (resource?.storage_uid !== undefined && !DECIMAL_ID.test(resource.storage_uid))
        || (resource?.storage_gid !== undefined && !DECIMAL_ID.test(resource.storage_gid))
        || (resource?.storage_expected_device !== undefined && !DECIMAL_ID.test(resource.storage_expected_device))
        || (resource?.storage_expected_inode !== undefined && !NONZERO_DECIMAL_ID.test(resource.storage_expected_inode))
        || (resource?.storage_expected_uid !== undefined && !DECIMAL_ID.test(resource.storage_expected_uid))
        || (resource?.storage_expected_gid !== undefined && !DECIMAL_ID.test(resource.storage_expected_gid))
        || (["active", "cleanup_pending"].includes(resource?.status)
          && (!validProviderId(resource.substrate, resource.provider_id)
            || !SHA256.test(resource?.provider_identity_sha256 ?? "")
            || !SHA256.test(resource?.configuration_sha256 ?? "")
            || !SHA256.test(resource?.ownership_binding_sha256 ?? "")))
        || (["active", "cleanup_pending"].includes(resource?.status) && resource?.substrate === "docker"
          && (!NETWORK_ID.test(resource?.managed_network_id ?? "")
            || !SHA256.test(resource?.managed_network_identity_sha256 ?? "")
            || !SHA256.test(resource?.managed_network_configuration_sha256 ?? "")))
        || (["active", "cleanup_pending"].includes(resource?.status) && resource?.substrate === "qemu"
          && (!SHA256.test(resource?.provider_storage_identity_sha256 ?? "") || !isAbsolute(resource?.storage_path ?? "")
            || !DECIMAL_ID.test(resource?.storage_device ?? "") || !NONZERO_DECIMAL_ID.test(resource?.storage_inode ?? "")
            || !DECIMAL_ID.test(resource?.storage_uid ?? "") || !DECIMAL_ID.test(resource?.storage_gid ?? "")))
        || (resource?.status === "cleanup_pending" && resource?.substrate === "qemu"
          && (!SHA256.test(resource?.storage_quarantine_identity_sha256 ?? "") || !isAbsolute(resource?.storage_quarantine_path ?? "")
            || !SHA256.test(resource?.storage_final_capture_identity_sha256 ?? "") || !isAbsolute(resource?.storage_final_capture_path ?? "")
            || !DECIMAL_ID.test(resource?.storage_expected_device ?? "") || !NONZERO_DECIMAL_ID.test(resource?.storage_expected_inode ?? "")
            || !DECIMAL_ID.test(resource?.storage_expected_uid ?? "") || !DECIMAL_ID.test(resource?.storage_expected_gid ?? "")))
        || !validTimestamp(resource?.planned_at) || !validTimestamp(resource?.updated_at)
        || (resource?.status === "removed" && !validTimestamp(resource?.removed_at))
        || resource.instance_id !== expected?.instance_id || resource.name !== expected?.name || resource.substrate !== expected?.substrate
        || resource.scenario_id !== expected?.scenario_id
        || resource.status !== expected?.status || resource.last_sequence !== expected?.last_sequence
        || resource.provider_id !== expected?.provider_id
        || resource.provider_identity_sha256 !== expected?.provider_identity_sha256
        || resource.configuration_sha256 !== expected?.configuration_sha256
        || resource.ownership_binding_sha256 !== expected?.ownership_binding_sha256
        || resource.managed_network_id !== expected?.managed_network_id
        || resource.managed_network_identity_sha256 !== expected?.managed_network_identity_sha256
        || resource.managed_network_configuration_sha256 !== expected?.managed_network_configuration_sha256
        || resource.provider_storage_identity_sha256 !== expected?.provider_storage_identity_sha256
        || resource.storage_path !== expected?.storage_path
        || resource.storage_quarantine_path !== expected?.storage_quarantine_path
        || resource.storage_quarantine_identity_sha256 !== expected?.storage_quarantine_identity_sha256
        || resource.storage_final_capture_path !== expected?.storage_final_capture_path
        || resource.storage_final_capture_identity_sha256 !== expected?.storage_final_capture_identity_sha256
        || resource.storage_device !== expected?.storage_device
        || resource.storage_inode !== expected?.storage_inode
        || resource.storage_uid !== expected?.storage_uid
        || resource.storage_gid !== expected?.storage_gid
        || resource.storage_expected_device !== expected?.storage_expected_device
        || resource.storage_expected_inode !== expected?.storage_expected_inode
        || resource.storage_expected_uid !== expected?.storage_expected_uid
        || resource.storage_expected_gid !== expected?.storage_expected_gid
        || resource.planned_at !== expected?.planned_at || resource.updated_at !== expected?.updated_at
        || resource.removed_at !== expected?.removed_at) errors.push(`${context} does not match journal replay`);
  }
  if (resourceIds.size !== replay.expectedResources.size) errors.push("inventory resources do not materialize every provider journal subject");

  const faultIds = new Set();
  for (const [index, fault] of inventory.faults.entries()) {
    const context = `inventory.faults[${index}]`;
    const allowedFault = new Set(["id", "scenario_id", "kind", "target", "target_identity_sha256", "target_ownership_sha256", "status", "last_sequence", "planned_at", "updated_at", "applied_at", "healed_at"]);
    rejectUnknown(errors, fault, allowedFault, context);
    const expected = replay.expectedFaults.get(fault?.id);
    if (faultIds.has(fault?.id)) errors.push(`${context} is duplicated`);
    faultIds.add(fault?.id);
    if (!expected
        || !FAULT_ID.test(fault?.id ?? "")
        || !SCENARIOS.has(fault?.scenario_id)
        || !FAULT_KINDS.has(fault?.kind)
        || !FAULT_TARGET.test(fault?.target ?? "")
        || !SHA256.test(fault?.target_identity_sha256 ?? "")
        || !SHA256.test(fault?.target_ownership_sha256 ?? "")
        || !["planned", "applied", "heal_pending", "healed"].includes(fault?.status)
        || !Number.isSafeInteger(fault?.last_sequence) || fault.last_sequence < 1
        || !validTimestamp(fault?.planned_at) || !validTimestamp(fault?.updated_at)
        || (["applied", "healed"].includes(fault?.status) && !validTimestamp(fault?.applied_at))
        || (fault?.status === "healed" && !validTimestamp(fault?.healed_at))
        || fault.id !== expected?.id || fault.kind !== expected?.kind || fault.target !== expected?.target
        || fault.target_identity_sha256 !== expected?.target_identity_sha256
        || fault.target_ownership_sha256 !== expected?.target_ownership_sha256
        || fault.scenario_id !== expected?.scenario_id
        || fault.status !== expected?.status || fault.last_sequence !== expected?.last_sequence
        || fault.planned_at !== expected?.planned_at || fault.updated_at !== expected?.updated_at
        || fault.applied_at !== expected?.applied_at || fault.healed_at !== expected?.healed_at) errors.push(`${context} does not match journal replay`);
  }
  if (faultIds.size !== replay.expectedFaults.size) errors.push("inventory faults do not materialize every fault journal subject");
  return errors;
}

export function validateOrchestrationInventoryDocument(inventory, config, options = {}) {
  if (!ownObject(inventory)) return ["inventory must be an object"];
  if (inventory.schema_version === ORCHESTRATION_INVENTORY_V1) return validateV1(inventory, config, options);
  if (inventory.schema_version === ORCHESTRATION_INVENTORY_V2) return validateV2(inventory, config, options);
  return ["inventory.schema_version is unsupported"];
}

function openDirectoryWithoutFollowingComponents(path, description) {
  const absolute = resolve(path);
  if (!isAbsolute(path ?? "")) throw new Error(`${description} is missing`);
  const flags = constants.O_RDONLY | (constants.O_DIRECTORY ?? 0) | (constants.O_NOFOLLOW ?? 0);
  let descriptor = openSync(sep, flags);
  try {
    for (const component of absolute.split(sep).filter(Boolean)) {
      const next = openSync(`/proc/self/fd/${descriptor}/${component}`, flags);
      closeSync(descriptor);
      descriptor = next;
    }
    return descriptor;
  } catch (error) {
    closeSync(descriptor);
    throw new Error(`${description} contains an unsafe or symbolic-link component: ${error.message}`);
  }
}

function openProtectedOrchestrationRunRoot(root) {
  if (!isAbsolute(root ?? "") || !existsSync(root)) throw new Error("orchestration run root is missing");
  const descriptor = openDirectoryWithoutFollowingComponents(root, "orchestration run root");
  const metadata = fstatSync(descriptor);
  const identity = processIdentity();
  if (!metadata.isDirectory() || (metadata.mode & 0o077) !== 0
      || (identity.uid !== null && metadata.uid !== identity.uid)
      || (identity.gid !== null && metadata.gid !== identity.gid)) {
    closeSync(descriptor);
    throw new Error("orchestration run root must be an owner-only directory owned by the current process");
  }
  return { descriptor, metadata };
}

export function assertProtectedOrchestrationRunRoot(root) {
  const opened = openProtectedOrchestrationRunRoot(root);
  closeSync(opened.descriptor);
  return opened.metadata;
}

function openProtectedInventory(path) {
  if (!existsSync(path)) throw new Error("orchestration inventory is missing");
  const before = lstatSync(path);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || (before.mode & 0o077) !== 0) {
    throw new Error("orchestration inventory must be a protected regular non-symlink single-link file");
  }
  const flags = constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0);
  const descriptor = openSync(path, flags);
  const metadata = fstatSync(descriptor);
  const identity = processIdentity();
  if (!metadata.isFile() || metadata.nlink !== 1 || (metadata.mode & 0o077) !== 0
      || metadata.size < 2 || metadata.size > MAX_INVENTORY_BYTES
      || (identity.uid !== null && metadata.uid !== identity.uid)
      || (identity.gid !== null && metadata.gid !== identity.gid)) {
    closeSync(descriptor);
    throw new Error("orchestration inventory must be a protected regular non-symlink single-link bounded file owned by the current process");
  }
  return { descriptor, metadata };
}

export function loadProtectedJson(path, description = "protected JSON document") {
  if (!isAbsolute(path ?? "") || !existsSync(path)) throw new Error(`${description} is missing`);
  const directory = openDirectoryWithoutFollowingComponents(dirname(resolve(path)), description);
  const anchoredPath = `/proc/self/fd/${directory}/${basename(path)}`;
  let descriptor = null;
  try {
    const before = lstatSync(anchoredPath);
    if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || (before.mode & 0o077) !== 0) {
      throw new Error(`${description} must be a protected regular non-symlink single-link file`);
    }
    descriptor = openSync(anchoredPath, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    const metadata = fstatSync(descriptor);
    const identity = processIdentity();
    if (!metadata.isFile() || metadata.nlink !== 1 || (metadata.mode & 0o077) !== 0
        || metadata.size < 2 || metadata.size > MAX_PROTECTED_JSON_BYTES
        || (identity.uid !== null && metadata.uid !== identity.uid)
        || (identity.gid !== null && metadata.gid !== identity.gid)) {
      throw new Error(`${description} must be a protected single-link bounded file owned by the current process`);
    }
    return JSON.parse(readFileSync(descriptor, "utf8"));
  } finally {
    if (descriptor !== null) closeSync(descriptor);
    closeSync(directory);
  }
}

export function loadProtectedOrchestrationInventory(path, config, { expectedHostSha256, beforeInventoryOpen } = {}) {
  if (!isAbsolute(path ?? "") || resolve(path) !== join(resolve(config.working_root ?? ""), "orchestration-inventory.json")) {
    throw new Error("orchestration inventory is not the fixed run-root file");
  }
  const root = openProtectedOrchestrationRunRoot(config.working_root);
  if (beforeInventoryOpen !== undefined && typeof beforeInventoryOpen !== "function") {
    closeSync(root.descriptor);
    throw new Error("orchestration inventory pre-open seam must be a function");
  }
  beforeInventoryOpen?.();
  let opened;
  try {
    opened = openProtectedInventory(`/proc/self/fd/${root.descriptor}/orchestration-inventory.json`);
  } catch (error) {
    closeSync(root.descriptor);
    throw error;
  }
  const { descriptor, metadata } = opened;
  let inventory;
  try { inventory = JSON.parse(readFileSync(descriptor, "utf8")); } finally { closeSync(descriptor); closeSync(root.descriptor); }
  const identity = processIdentity();
  const errors = validateOrchestrationInventoryDocument(inventory, config, {
    expectedHostSha256,
    expectedUid: inventory?.schema_version === ORCHESTRATION_INVENTORY_V2 ? metadata.uid : undefined,
    expectedGid: inventory?.schema_version === ORCHESTRATION_INVENTORY_V2 ? metadata.gid : undefined,
  });
  if (inventory?.schema_version === ORCHESTRATION_INVENTORY_V2
      && (inventory.owner.uid !== identity.uid || inventory.owner.gid !== identity.gid)) errors.push("inventory owner does not match the running process");
  if (errors.length) throw new Error(errors.join("; "));
  return inventory;
}

const LIFECYCLE_TOKEN = Symbol("orchestration-inventory-lifecycle");
const LOCK_SCHEMA = "agentic-sandbox.celld-orchestration-lock/v1";

function processStartTimeTicks(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    const fields = stat.slice(stat.lastIndexOf(")") + 2).trim().split(/\s+/);
    return /^[0-9]+$/.test(fields[19] ?? "") ? fields[19] : null;
  } catch {
    return null;
  }
}

function readLockOwner(ownerPath) {
  if (!existsSync(ownerPath)) return { status: "absent" };
  const metadata = lstatSync(ownerPath);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1 || (metadata.mode & 0o077) !== 0) {
    throw new Error("orchestration inventory lock has foreign or unsafe owner metadata");
  }
  const raw = readFileSync(ownerPath, "utf8");
  let owner;
  try {
    owner = JSON.parse(raw);
  } catch {
    return { status: "invalid", raw, metadata };
  }
  if (!ownObject(owner)
      || JSON.stringify(Object.keys(owner).sort()) !== JSON.stringify(["pid", "process_start_time_ticks", "schema_version"].sort())
      || owner.schema_version !== LOCK_SCHEMA
      || !Number.isSafeInteger(owner.pid) || owner.pid < 1
      || !/^[0-9]+$/.test(String(owner.process_start_time_ticks ?? ""))) {
    return { status: "invalid", raw, metadata };
  }
  return { status: "valid", owner, raw, metadata };
}

function sameLockOwnerSnapshot(left, right) {
  return left?.status === right?.status
    && left?.raw === right?.raw
    && left?.metadata?.dev === right?.metadata?.dev
    && left?.metadata?.ino === right?.metadata?.ino
    && left?.metadata?.size === right?.metadata?.size
    && left?.metadata?.mtimeMs === right?.metadata?.mtimeMs;
}

function assertSafeLockDirectory(lockPath) {
  const metadata = lstatSync(lockPath);
  const identity = processIdentity();
  if (!metadata.isDirectory() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0
      || (identity.uid !== null && metadata.uid !== identity.uid)
      || (identity.gid !== null && metadata.gid !== identity.gid)) {
    throw new Error("orchestration inventory lock directory is foreign or unsafe");
  }
  return metadata;
}

function sameDirectorySnapshot(left, right) {
  return left?.dev === right?.dev
    && left?.ino === right?.ino
    && left?.uid === right?.uid
    && left?.gid === right?.gid
    && left?.mode === right?.mode;
}

function pendingLockPublisherPid(name) {
  const match = /^\.orchestration-inventory\.lock\.pending-([1-9][0-9]*)-[0-9a-f]{16}$/.exec(name);
  if (!match) return null;
  const pid = Number(match[1]);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

function reclaimPendingLockCandidate(root, name) {
  const candidatePath = `/proc/self/fd/${root.descriptor}/${name}`;
  const ownerPath = `${candidatePath}/owner.json`;
  const directory = assertSafeLockDirectory(candidatePath);
  const observed = readLockOwner(ownerPath);
  const publisherPid = pendingLockPublisherPid(name);
  if (publisherPid === null) throw new Error("orchestration inventory pending owner name is foreign or unsafe");
  // mkdir precedes owner.json publication.  The PID in the staged pathname is
  // therefore the only durable process identity available in that crash window.
  // Preserve the candidate while that exact PID exists; PID reuse can delay
  // reclamation, but must never permit stealing a live publisher's directory.
  if (processStartTimeTicks(publisherPid) !== null) return false;
  if (observed.status === "valid") {
    const observedStart = processStartTimeTicks(observed.owner.pid);
    if (observedStart !== null && observedStart === String(observed.owner.process_start_time_ticks)) return false;
  }
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
  const confirmationDirectory = assertSafeLockDirectory(candidatePath);
  const confirmation = readLockOwner(ownerPath);
  if (!sameDirectorySnapshot(directory, confirmationDirectory)
      || observed.status !== confirmation.status
      || (observed.status !== "absent" && !sameLockOwnerSnapshot(observed, confirmation))) {
    return false;
  }
  if (confirmation.status !== "absent") rmSync(ownerPath, { force: false });
  try {
    rmdirSync(candidatePath);
  } catch (error) {
    if (error.code === "ENOENT") return true;
    throw new Error(`orchestration inventory pending owner is foreign or unsafe: ${error.message}`);
  }
  fsyncSync(root.descriptor);
  return true;
}

function reclaimStalePendingLockCandidates(root) {
  const names = readdirSync(`/proc/self/fd/${root.descriptor}`)
    .filter((name) => pendingLockPublisherPid(name) !== null)
    .sort();
  let livePendingOwner = null;
  for (const name of names) {
    try {
      if (!reclaimPendingLockCandidate(root, name)) livePendingOwner ??= name;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  return livePendingOwner;
}

export function acquireOrchestrationInventoryLifecycle(config, { deadlineMs = 5_000 } = {}) {
  const root = openProtectedOrchestrationRunRoot(config.working_root);
  const lockPath = `/proc/self/fd/${root.descriptor}/.orchestration-inventory.lock`;
  const ownerPath = `${lockPath}/owner.json`;
  const deadline = Date.now() + deadlineMs;
  try {
    for (;;) {
      const livePendingOwner = reclaimStalePendingLockCandidates(root);
      if (livePendingOwner !== null) {
        if (Date.now() > deadline) throw new Error(`orchestration inventory live staged pending owner prevents lock acquisition: ${livePendingOwner}`);
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
        continue;
      }
      const stagingPath = `/proc/self/fd/${root.descriptor}/.orchestration-inventory.lock.pending-${process.pid}-${randomBytes(8).toString("hex")}`;
      try {
        mkdirSync(stagingPath, { mode: 0o700 });
        const owner = {
          schema_version: LOCK_SCHEMA,
          pid: process.pid,
          process_start_time_ticks: processStartTimeTicks(process.pid),
        };
        if (owner.process_start_time_ticks === null) throw new Error("orchestration inventory lock cannot identify the current process");
        const stagingOwnerPath = `${stagingPath}/owner.json`;
        const descriptor = openSync(stagingOwnerPath, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | (constants.O_NOFOLLOW ?? 0), 0o600);
        try {
          writeFileSync(descriptor, `${JSON.stringify(owner)}\n`, "utf8");
          fsyncSync(descriptor);
        } finally {
          closeSync(descriptor);
        }
        const stagingDescriptor = openSync(stagingPath, constants.O_RDONLY | (constants.O_DIRECTORY ?? 0) | (constants.O_NOFOLLOW ?? 0));
        try { fsyncSync(stagingDescriptor); } finally { closeSync(stagingDescriptor); }
        fsyncSync(root.descriptor);
        renameSync(stagingPath, lockPath);
        const lockDescriptor = openSync(lockPath, constants.O_RDONLY | (constants.O_DIRECTORY ?? 0) | (constants.O_NOFOLLOW ?? 0));
        try { fsyncSync(lockDescriptor); } finally { closeSync(lockDescriptor); }
        fsyncSync(root.descriptor);
        return { token: LIFECYCLE_TOKEN, root, lockPath, ownerPath, released: false };
      } catch (error) {
        rmSync(stagingPath, { recursive: true, force: true });
        if (!["EEXIST", "ENOTEMPTY"].includes(error.code)) throw error;
        assertSafeLockDirectory(lockPath);
        const observed = readLockOwner(ownerPath);
        if (observed.status === "valid") {
          const observedStart = processStartTimeTicks(observed.owner.pid);
          if (observedStart === null || observedStart !== String(observed.owner.process_start_time_ticks)) {
            const confirmation = readLockOwner(ownerPath);
            if (confirmation.status === "valid" && sameLockOwnerSnapshot(confirmation, observed)) {
              rmSync(ownerPath, { force: false });
              rmdirSync(lockPath);
              fsyncSync(root.descriptor);
              continue;
            }
          }
        } else if (observed.status === "invalid") {
          Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
          const confirmation = readLockOwner(ownerPath);
          if (confirmation.status === "invalid" && sameLockOwnerSnapshot(confirmation, observed)) {
            rmSync(ownerPath, { force: false });
            rmdirSync(lockPath);
            fsyncSync(root.descriptor);
            continue;
          }
        } else if (observed.status === "absent") {
          try {
            rmdirSync(lockPath);
            fsyncSync(root.descriptor);
            continue;
          } catch (removeError) {
            if (!["ENOENT", "ENOTEMPTY", "EEXIST"].includes(removeError.code)) throw removeError;
          }
        }
        if (Date.now() > deadline) throw new Error("orchestration inventory compare-and-swap lock deadline expired");
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
      }
    }
  } catch (error) {
    closeSync(root.descriptor);
    throw error;
  }
}

export function releaseOrchestrationInventoryLifecycle(lifecycle) {
  if (lifecycle?.token !== LIFECYCLE_TOKEN || lifecycle.released) throw new Error("orchestration inventory lifecycle token is invalid");
  lifecycle.released = true;
  try {
    if (existsSync(lifecycle.ownerPath)) rmSync(lifecycle.ownerPath, { force: false });
    if (existsSync(lifecycle.lockPath)) rmdirSync(lifecycle.lockPath);
    fsyncSync(lifecycle.root.descriptor);
  } finally {
    closeSync(lifecycle.root.descriptor);
  }
}

export function commitOrchestrationInventory(path, inventory, options = {}) {
  const { config, expectedHostSha256 = inventory?.host_sha256 } = options;
  if (!config || resolve(path ?? "") !== join(resolve(config.working_root ?? ""), "orchestration-inventory.json")) throw new Error("orchestration inventory commit path is invalid");
  const ownsLifecycle = options.lifecycle === undefined;
  const lifecycle = options.lifecycle ?? acquireOrchestrationInventoryLifecycle(config);
  if (lifecycle?.token !== LIFECYCLE_TOKEN || lifecycle.released) throw new Error("orchestration inventory lifecycle token is invalid");
  const root = lifecycle.root;
  const anchoredPath = `/proc/self/fd/${root.descriptor}/orchestration-inventory.json`;
  const temporary = `${anchoredPath}.tmp-${process.pid}-${randomBytes(8).toString("hex")}`;
  let descriptor = null;
  try {
    const errors = validateOrchestrationInventoryDocument(inventory, config, { expectedHostSha256 });
    if (errors.length) throw new Error(errors.join("; "));
    const hasExpectedHead = Object.hasOwn(options, "expectedJournalHeadSha256");
    if (existsSync(anchoredPath)) {
      const opened = openProtectedInventory(anchoredPath);
      let current;
      try { current = JSON.parse(readFileSync(opened.descriptor, "utf8")); } finally { closeSync(opened.descriptor); }
      if (hasExpectedHead && current?.journal_head_sha256 !== options.expectedJournalHeadSha256) {
        throw new Error("orchestration inventory compare-and-swap rejected a stale journal head");
      }
    } else if (hasExpectedHead && options.expectedJournalHeadSha256 !== null) {
      throw new Error("orchestration inventory compare-and-swap expected a missing journal head");
    }
    if (options.afterCompareBeforeReplace !== undefined && typeof options.afterCompareBeforeReplace !== "function") {
      throw new Error("orchestration inventory compare/replace seam must be a function");
    }
    options.afterCompareBeforeReplace?.();
    descriptor = openSync(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | (constants.O_NOFOLLOW ?? 0), 0o600);
    writeFileSync(descriptor, `${JSON.stringify(inventory, null, 2)}\n`, "utf8");
    fsyncSync(descriptor);
    closeSync(descriptor);
    descriptor = null;
    chmodSync(temporary, 0o600);
    renameSync(temporary, anchoredPath);
    const committed = openProtectedInventory(anchoredPath);
    try { fsyncSync(committed.descriptor); } finally { closeSync(committed.descriptor); }
    fsyncSync(root.descriptor);
  } finally {
    if (descriptor !== null) closeSync(descriptor);
    rmSync(temporary, { force: true });
    if (ownsLifecycle) releaseOrchestrationInventoryLifecycle(lifecycle);
  }
  return path;
}

export function createOrchestrationInventoryV2({ runId, workingRoot, hostSha256, now = new Date() }) {
  const timestamp = now.toISOString();
  const identity = processIdentity();
  if (identity.uid === null || identity.gid === null) throw new Error("orchestration inventory v2 requires a POSIX process owner");
  return {
    schema_version: ORCHESTRATION_INVENTORY_V2,
    run_id: runId,
    working_root: resolve(workingRoot),
    owner: { ...ORCHESTRATION_OWNER, run_id: runId, uid: identity.uid, gid: identity.gid },
    host_sha256: hostSha256,
    created_at: timestamp,
    updated_at: timestamp,
    state: "prepared",
    last_sequence: 0,
    journal_head_sha256: null,
    incomplete_mutation_ids: [],
    resources: [],
    faults: [],
    journal: [],
  };
}

export function isOrchestrationInventoryV2(inventory) {
  return inventory?.schema_version === ORCHESTRATION_INVENTORY_V2;
}

function refreshJournalDerived(inventory, timestamp) {
  inventory.last_sequence = inventory.journal.length;
  inventory.journal_head_sha256 = inventory.journal.at(-1)?.entry_sha256 ?? null;
  const terminal = new Set(inventory.journal.filter((entry) => entry.event !== "planned").map((entry) => entry.mutation_id));
  inventory.incomplete_mutation_ids = inventory.journal.filter((entry) => entry.event === "planned" && !terminal.has(entry.mutation_id)).map((entry) => entry.mutation_id);
  inventory.updated_at = timestamp;
}

function appendEntry(inventory, entry, now) {
  if (!isOrchestrationInventoryV2(inventory)) throw new Error("mutation journaling requires orchestration inventory v2");
  const timestamp = now instanceof Date ? now.toISOString() : new Date(now).toISOString();
  const base = {
    sequence: inventory.journal.length + 1,
    ...entry,
    recorded_at: timestamp,
    previous_entry_sha256: inventory.journal.at(-1)?.entry_sha256 ?? null,
  };
  const record = { ...base, entry_sha256: sha256(canonicalOrchestrationJson(base)) };
  inventory.journal.push(record);
  refreshJournalDerived(inventory, timestamp);
  return record;
}

export function planOrchestrationMutation(inventory, {
  mutation,
  scenarioId,
  subjectType,
  subject,
  mutationId = randomUUID(),
  allowConflictWithMutationId = null,
}, now = new Date()) {
  if (!MUTATIONS.has(mutation) || !SCENARIOS.has(scenarioId) || !["provider_resource", "fault"].includes(subjectType) || !UUID.test(mutationId)) throw new Error("orchestration mutation plan is invalid");
  const subjectErrors = [];
  if (subjectType === "provider_resource") validateProviderIntent(subject, subjectErrors, "orchestration mutation subject");
  else validateFaultIntent(subject, subjectErrors, "orchestration mutation subject");
  if (subjectErrors.length || (["provider_action", "provider_cleanup"].includes(mutation) !== (subjectType === "provider_resource"))
      || (["fault_apply", "fault_heal"].includes(mutation) !== (subjectType === "fault"))) {
    throw new Error(subjectErrors.join("; ") || "orchestration mutation and subject type disagree");
  }
  const subjectKey = subjectType === "provider_resource" ? subject.instance_id : subject.fault_id;
  const conflict = inventory.journal.find((entry) => entry.event === "planned"
    && inventory.incomplete_mutation_ids.includes(entry.mutation_id)
    && entry.subject_type === subjectType
    && (subjectType === "provider_resource" ? entry.subject.instance_id : entry.subject.fault_id) === subjectKey
    && entry.mutation_id !== allowConflictWithMutationId);
  if (conflict) throw new Error("orchestration mutation target already has an incomplete serialized plan");
  let materialized = subjectType === "provider_resource"
    ? inventory.resources.find((resource) => resource.instance_id === subject.instance_id)
    : inventory.faults.find((fault) => fault.id === subject.fault_id);
  if (subjectType === "provider_resource") {
    if (!materialized && (mutation !== "provider_action" || subject.action !== "provision")) throw new Error("provider mutation target has no persisted provision identity");
    if (materialized && (materialized.name !== subject.name || materialized.substrate !== subject.substrate
        || (materialized.status === "removed" && !(mutation === "provider_action" && subject.action === "provision")))) {
      throw new Error("provider mutation target is not an active exact-owned resource");
    }
  } else {
    if (!materialized && mutation !== "fault_apply") throw new Error("fault heal target has no persisted apply identity");
    const closingApply = mutation === "fault_heal"
      && materialized?.status === "planned"
      && allowConflictWithMutationId !== null
      && inventory.incomplete_mutation_ids.includes(allowConflictWithMutationId);
    if (materialized && (materialized.kind !== subject.kind || materialized.target !== subject.target || materialized.status === "healed"
        || mutation === "fault_heal" && !["applied", "heal_pending"].includes(materialized.status) && !closingApply)) {
      throw new Error("fault mutation target is not an active exact-owned fault");
    }
  }
  let persistedSubject = subject;
  if (subjectType === "provider_resource" && mutation === "provider_cleanup" && subject.substrate === "qemu") {
    if (!SHA256.test(materialized?.provider_storage_identity_sha256 ?? "") || !isAbsolute(materialized?.storage_path ?? "")
        || !DECIMAL_ID.test(materialized?.storage_device ?? "") || !NONZERO_DECIMAL_ID.test(materialized?.storage_inode ?? "")
        || !DECIMAL_ID.test(materialized?.storage_uid ?? "") || !DECIMAL_ID.test(materialized?.storage_gid ?? "")) {
      throw new Error("QEMU cleanup requires a persisted exact storage identity");
    }
    const storageQuarantinePath = join(dirname(materialized.storage_path), `.${materialized.name}.cleanup-${mutationId}`);
    const storageFinalCapturePath = join(QEMU_CLEANUP_CAPTURE_ROOT, inventory.run_id, `${basename(storageQuarantinePath)}.final`);
    if ((subject.storage_quarantine_path !== undefined && subject.storage_quarantine_path !== storageQuarantinePath)
        || (subject.storage_quarantine_identity_sha256 !== undefined
          && subject.storage_quarantine_identity_sha256 !== materialized.provider_storage_identity_sha256)
        || (subject.storage_final_capture_path !== undefined && subject.storage_final_capture_path !== storageFinalCapturePath)
        || (subject.storage_final_capture_identity_sha256 !== undefined
          && subject.storage_final_capture_identity_sha256 !== materialized.provider_storage_identity_sha256)
        || (subject.storage_expected_device !== undefined && subject.storage_expected_device !== materialized.storage_device)
        || (subject.storage_expected_inode !== undefined && subject.storage_expected_inode !== materialized.storage_inode)
        || (subject.storage_expected_uid !== undefined && subject.storage_expected_uid !== materialized.storage_uid)
        || (subject.storage_expected_gid !== undefined && subject.storage_expected_gid !== materialized.storage_gid)) {
      throw new Error("QEMU cleanup cannot replace the persisted deterministic storage quarantine binding");
    }
    persistedSubject = {
      ...subject,
      storage_quarantine_path: storageQuarantinePath,
      storage_quarantine_identity_sha256: materialized.provider_storage_identity_sha256,
      storage_final_capture_path: storageFinalCapturePath,
      storage_final_capture_identity_sha256: materialized.provider_storage_identity_sha256,
      storage_expected_device: materialized.storage_device,
      storage_expected_inode: materialized.storage_inode,
      storage_expected_uid: materialized.storage_uid,
      storage_expected_gid: materialized.storage_gid,
    };
  }
  const entry = appendEntry(inventory, {
    mutation_id: mutationId,
    event: "planned",
    mutation,
    scenario_id: scenarioId,
    subject_type: subjectType,
    subject: persistedSubject,
  }, now);
  const timestamp = entry.recorded_at;
  if (subjectType === "provider_resource") {
    if (!materialized) {
      materialized = {
        scenario_id: scenarioId,
        instance_id: subject.instance_id,
        name: subject.name,
        substrate: subject.substrate,
        status: "planned",
        last_sequence: entry.sequence,
        planned_at: timestamp,
        updated_at: timestamp,
      };
      inventory.resources.push(materialized);
    } else {
      if (mutation === "provider_cleanup") {
        materialized.status = "cleanup_pending";
        if (subject.substrate === "qemu") {
          materialized.storage_quarantine_path = persistedSubject.storage_quarantine_path;
          materialized.storage_quarantine_identity_sha256 = persistedSubject.storage_quarantine_identity_sha256;
          materialized.storage_final_capture_path = persistedSubject.storage_final_capture_path;
          materialized.storage_final_capture_identity_sha256 = persistedSubject.storage_final_capture_identity_sha256;
          materialized.storage_expected_device = persistedSubject.storage_expected_device;
          materialized.storage_expected_inode = persistedSubject.storage_expected_inode;
          materialized.storage_expected_uid = persistedSubject.storage_expected_uid;
          materialized.storage_expected_gid = persistedSubject.storage_expected_gid;
        }
      }
      if (mutation === "provider_action" && subject.action === "provision") {
        if (materialized.status === "removed") {
          for (const field of [
            "provider_id", "provider_identity_sha256", "configuration_sha256", "ownership_binding_sha256", "managed_network_id",
            "managed_network_identity_sha256", "managed_network_configuration_sha256", "provider_storage_identity_sha256", "storage_path",
            "storage_quarantine_path", "storage_quarantine_identity_sha256",
            "storage_final_capture_path", "storage_final_capture_identity_sha256",
            "storage_device", "storage_inode", "storage_uid", "storage_gid",
            "storage_expected_device", "storage_expected_inode", "storage_expected_uid", "storage_expected_gid",
          ]) delete materialized[field];
        }
        materialized.status = "planned";
      }
      materialized.last_sequence = entry.sequence;
      materialized.updated_at = timestamp;
      delete materialized.removed_at;
    }
  } else {
    if (!materialized) {
      materialized = {
        id: subject.fault_id,
        scenario_id: scenarioId,
        kind: subject.kind,
        target: subject.target,
        ...(subject.target_identity_sha256 ? { target_identity_sha256: subject.target_identity_sha256 } : {}),
        ...(subject.target_ownership_sha256 ? { target_ownership_sha256: subject.target_ownership_sha256 } : {}),
        status: "planned",
        last_sequence: entry.sequence,
        planned_at: timestamp,
        updated_at: timestamp,
      };
      inventory.faults.push(materialized);
    } else {
      if (mutation === "fault_heal") materialized.status = "heal_pending";
      materialized.last_sequence = entry.sequence;
      materialized.updated_at = timestamp;
    }
  }
  inventory.state = "active";
  return { entry, materialized };
}

export function finishOrchestrationMutation(inventory, plan, {
  event = "completed",
  outcome,
  observedIdentitySha256 = null,
  observedConfigurationSha256 = null,
  observedProviderId,
  observedOwnershipBindingSha256,
  observedManagedNetworkId,
  observedManagedNetworkIdentitySha256,
  observedManagedNetworkConfigurationSha256,
  observedProviderStorageIdentitySha256,
  observedStoragePath,
  observedStorageDevice,
  observedStorageInode,
  observedStorageUid,
  observedStorageGid,
  targetTransition,
} = {}, now = new Date()) {
  const storedPlan = inventory.journal.find((entry) => entry.event === "planned" && entry.mutation_id === plan?.mutation_id);
  if (!storedPlan || !["completed", "recovered"].includes(event) || !OUTCOMES.has(outcome)
      || inventory.journal.some((entry) => entry.event !== "planned" && entry.mutation_id === storedPlan.mutation_id)) {
    throw new Error("orchestration mutation terminal record is invalid");
  }
  const semanticErrors = [];
  const terminalEvidence = {
    event,
    outcome,
    subject_type: storedPlan.subject_type,
    observed_identity_sha256: observedIdentitySha256,
    observed_configuration_sha256: observedConfigurationSha256,
    ...(observedProviderId !== undefined ? { observed_provider_id: observedProviderId } : {}),
    ...(observedOwnershipBindingSha256 !== undefined ? { observed_ownership_binding_sha256: observedOwnershipBindingSha256 } : {}),
    ...(observedManagedNetworkId !== undefined ? { observed_managed_network_id: observedManagedNetworkId } : {}),
    ...(observedManagedNetworkIdentitySha256 !== undefined ? { observed_managed_network_identity_sha256: observedManagedNetworkIdentitySha256 } : {}),
    ...(observedManagedNetworkConfigurationSha256 !== undefined ? { observed_managed_network_configuration_sha256: observedManagedNetworkConfigurationSha256 } : {}),
    ...(observedProviderStorageIdentitySha256 !== undefined ? { observed_provider_storage_identity_sha256: observedProviderStorageIdentitySha256 } : {}),
    ...(observedStoragePath !== undefined ? { observed_storage_path: observedStoragePath } : {}),
    ...(observedStorageDevice !== undefined ? { observed_storage_device: observedStorageDevice } : {}),
    ...(observedStorageInode !== undefined ? { observed_storage_inode: observedStorageInode } : {}),
    ...(observedStorageUid !== undefined ? { observed_storage_uid: observedStorageUid } : {}),
    ...(observedStorageGid !== undefined ? { observed_storage_gid: observedStorageGid } : {}),
    ...(targetTransition !== undefined ? { target_transition: targetTransition } : {}),
  };
  validateTerminalSemantics(storedPlan, terminalEvidence, semanticErrors, "orchestration mutation terminal");
  const materializedBeforeAppend = storedPlan.subject_type === "provider_resource"
    ? inventory.resources.find((resource) => resource.instance_id === storedPlan.subject.instance_id)
    : inventory.faults.find((fault) => fault.id === storedPlan.subject.fault_id);
  if (!materializedBeforeAppend) semanticErrors.push("orchestration mutation terminal materialized target is absent");
  if (semanticErrors.length) throw new Error(semanticErrors.join("; "));
  const entry = appendEntry(inventory, {
    mutation_id: storedPlan.mutation_id,
    event,
    mutation: storedPlan.mutation,
    scenario_id: storedPlan.scenario_id,
    subject_type: storedPlan.subject_type,
    subject: storedPlan.subject,
    plan_sequence: storedPlan.sequence,
    outcome,
    observed_identity_sha256: observedIdentitySha256,
    observed_configuration_sha256: observedConfigurationSha256,
    ...(observedProviderId !== undefined ? { observed_provider_id: observedProviderId } : {}),
    ...(observedOwnershipBindingSha256 !== undefined ? { observed_ownership_binding_sha256: observedOwnershipBindingSha256 } : {}),
    ...(observedManagedNetworkId !== undefined ? { observed_managed_network_id: observedManagedNetworkId } : {}),
    ...(observedManagedNetworkIdentitySha256 !== undefined ? { observed_managed_network_identity_sha256: observedManagedNetworkIdentitySha256 } : {}),
    ...(observedManagedNetworkConfigurationSha256 !== undefined ? { observed_managed_network_configuration_sha256: observedManagedNetworkConfigurationSha256 } : {}),
    ...(observedProviderStorageIdentitySha256 !== undefined ? { observed_provider_storage_identity_sha256: observedProviderStorageIdentitySha256 } : {}),
    ...(observedStoragePath !== undefined ? { observed_storage_path: observedStoragePath } : {}),
    ...(observedStorageDevice !== undefined ? { observed_storage_device: observedStorageDevice } : {}),
    ...(observedStorageInode !== undefined ? { observed_storage_inode: observedStorageInode } : {}),
    ...(observedStorageUid !== undefined ? { observed_storage_uid: observedStorageUid } : {}),
    ...(observedStorageGid !== undefined ? { observed_storage_gid: observedStorageGid } : {}),
    ...(targetTransition !== undefined ? { target_transition: targetTransition } : {}),
  }, now);
  const timestamp = entry.recorded_at;
  let materialized;
  if (storedPlan.subject_type === "provider_resource") {
    materialized = inventory.resources.find((resource) => resource.instance_id === storedPlan.subject.instance_id);
    if (!materialized) throw new Error("provider mutation terminal target is absent from inventory");
    materialized.status = storedPlan.mutation === "provider_cleanup" || outcome === "absent" ? "removed" : "active";
    if (outcome === "effect_observed") {
      if (observedProviderId !== undefined) materialized.provider_id = observedProviderId;
      materialized.provider_identity_sha256 = observedIdentitySha256;
      materialized.configuration_sha256 = observedConfigurationSha256;
      if (observedOwnershipBindingSha256 !== undefined) materialized.ownership_binding_sha256 = observedOwnershipBindingSha256;
      if (observedManagedNetworkId !== undefined) materialized.managed_network_id = observedManagedNetworkId;
      if (observedManagedNetworkIdentitySha256 !== undefined) materialized.managed_network_identity_sha256 = observedManagedNetworkIdentitySha256;
      if (observedManagedNetworkConfigurationSha256 !== undefined) materialized.managed_network_configuration_sha256 = observedManagedNetworkConfigurationSha256;
      if (observedProviderStorageIdentitySha256 !== undefined) materialized.provider_storage_identity_sha256 = observedProviderStorageIdentitySha256;
      if (observedStoragePath !== undefined) materialized.storage_path = observedStoragePath;
      if (observedStorageDevice !== undefined) materialized.storage_device = observedStorageDevice;
      if (observedStorageInode !== undefined) materialized.storage_inode = observedStorageInode;
      if (observedStorageUid !== undefined) materialized.storage_uid = observedStorageUid;
      if (observedStorageGid !== undefined) materialized.storage_gid = observedStorageGid;
    }
    materialized.last_sequence = entry.sequence;
    materialized.updated_at = timestamp;
    if (materialized.status === "removed") materialized.removed_at = timestamp;
  } else {
    materialized = inventory.faults.find((fault) => fault.id === storedPlan.subject.fault_id);
    if (!materialized) throw new Error("fault mutation terminal target is absent from inventory");
    materialized.status = storedPlan.mutation === "fault_heal" || outcome !== "applied" ? "healed" : "applied";
    materialized.last_sequence = entry.sequence;
    materialized.updated_at = timestamp;
    if (materialized.status === "applied") materialized.applied_at = timestamp;
    if (materialized.status === "healed") {
      if (storedPlan.mutation === "fault_heal") materialized.applied_at ??= materialized.planned_at;
      materialized.healed_at = timestamp;
    }
  }
  return { entry, materialized };
}

export function newFaultId() {
  return randomBytes(16).toString("hex");
}
