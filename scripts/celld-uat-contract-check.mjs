#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scriptPath = fileURLToPath(import.meta.url);
export const REQUIRED_AUTHORITY = Object.freeze({
  incarnation_generation: "management",
  lifecycle_effects: "management",
  provider_inventory: "management",
  observed_runtime_state: "management",
  agent_enrollment: "management",
  desired_intent: "instance_cell",
  accepted_commands: "instance_cell",
  retry_schedule: "instance_cell",
  transition_evidence: "instance_cell",
  compute_effects: "runtime_provider",
  cell_bytes: "object_store",
  ownership_leases: "object_store",
  ownership_epochs: "object_store",
  fencing_decisions: "object_store",
});
const AUTHORITY_IDS = Object.freeze(["management", "instance_cell", "runtime_provider", "object_store"]);

export function validateAuthorityMatrix(matrix) {
  const errors = [];
  if (matrix?.document_type !== "celld-authority-matrix") errors.push("document_type must be celld-authority-matrix");
  if (matrix?.schema_version !== "1") errors.push("schema_version must be 1");
  if (matrix?.decision_source !== "ADR-036") errors.push("decision_source must be ADR-036");
  const authorities = Array.isArray(matrix?.authorities) ? matrix.authorities : [];
  const authorityIds = authorities.map((authority) => authority?.id);
  if (new Set(authorityIds).size !== AUTHORITY_IDS.length || AUTHORITY_IDS.some((id) => !authorityIds.includes(id))) {
    errors.push(`authorities must define exactly: ${AUTHORITY_IDS.join(", ")}`);
  }
  if (authorities.some((authority) => typeof authority?.role !== "string" || authority.role.trim() === "")) errors.push("every authority requires a role");

  const fields = Array.isArray(matrix?.fields) ? matrix.fields : [];
  const names = fields.map((entry) => entry?.field);
  if (new Set(names).size !== fields.length) errors.push("authority field names must be unique");
  for (const [field, owner] of Object.entries(REQUIRED_AUTHORITY)) {
    const matches = fields.filter((entry) => entry?.field === field);
    if (matches.length !== 1) {
      errors.push(`${field} must have exactly one authority record`);
      continue;
    }
    const entry = matches[0];
    if (entry.authoritative_owner !== owner) errors.push(`${field} must be owned by ${owner}`);
    if (typeof entry.repair_rule !== "string" || entry.repair_rule.trim() === "") errors.push(`${field} requires an explicit repair rule`);
    const expectedForbidden = AUTHORITY_IDS.filter((id) => id !== owner).sort();
    const forbidden = Array.isArray(entry.forbidden_writers) ? entry.forbidden_writers : [];
    if (new Set(forbidden).size !== forbidden.length || forbidden.includes(owner) || JSON.stringify([...forbidden].sort()) !== JSON.stringify(expectedForbidden)) {
      errors.push(`${field} forbidden_writers must contain every non-owner exactly once`);
    }
  }
  const extras = names.filter((name) => !(name in REQUIRED_AUTHORITY));
  if (extras.length > 0) errors.push(`unexpected authority fields: ${extras.join(", ")}`);
  return errors;
}

export function runContractChecks() {
const schemas = [
  "docs/contracts/celld/instance-cell-v1.schema.json",
  "docs/contracts/celld/worker-bundle-v1.schema.json",
  "docs/contracts/celld/fleet-manifest-v1.schema.json",
];
const uatSchemas = [
  "tests/celld/uat/live-profile-v1.schema.json",
  "tests/celld/uat/live-observation-v1.schema.json",
  "tests/celld/uat/storage-profile-v1.schema.json",
  "tests/celld/uat/storage-evidence-v1.schema.json",
  "tests/celld/uat/seaweedfs-fixture-v1.schema.json",
  "tests/celld/uat/live-orchestration-v1.schema.json",
  "tests/celld/uat/orchestration-inventory-v1.schema.json",
  "tests/celld/uat/dispatch-gate-v1.schema.json",
  "tests/celld/uat/crash-phase-evidence-v1.schema.json",
  "tests/celld/uat/network-auth-evidence-v1.schema.json",
  "tests/celld/uat/network-auth-inventory-v1.schema.json",
];
const parsed = [...schemas, ...uatSchemas].map((path) => [path, JSON.parse(readFileSync(resolve(root, path), "utf8"))]);
for (const [path, schema] of parsed) {
  if (schema.$schema !== "https://json-schema.org/draft/2020-12/schema") throw new Error(`${path}: JSON Schema draft must be 2020-12`);
  if (typeof schema.$id !== "string" || !schema.$id.endsWith("/v1")) throw new Error(`${path}: stable v1 $id required`);
  if (schema.type !== "object" && !Array.isArray(schema.oneOf)) throw new Error(`${path}: root must define an object or a oneOf union`);
}

const fleet = JSON.parse(readFileSync(resolve(root, "deploy/celld/fleet.example.json"), "utf8"));
const bundleBytes = readFileSync(resolve(root, "runtimes/celld/instance-cell/worker.mjs"));
const bundle = JSON.parse(readFileSync(resolve(root, "runtimes/celld/instance-cell/bundle.json"), "utf8"));
const digest = `sha256:${createHash("sha256").update(bundleBytes).digest("hex")}`;
if (bundle.digest !== digest || fleet.application_bundle !== digest) throw new Error("reference Worker digest does not match bundle and fleet manifests");
if (fleet.celld.version !== "v0.2.1" || fleet.celld.protocol_version !== "celld-internal-v1") throw new Error("fleet pin is not the qualified Celld pair");
if (bundle.compatibility?.celld_version !== "v0.2.1") throw new Error("bundle pin is not the qualified Celld version");
const authority = JSON.parse(readFileSync(resolve(root, "docs/contracts/celld/authority-matrix-v1.json"), "utf8"));
const authorityErrors = validateAuthorityMatrix(authority);
if (authorityErrors.length > 0) throw new Error(`authority matrix invalid: ${authorityErrors.join("; ")}`);
const images = JSON.parse(readFileSync(resolve(root, "deploy/celld/qualification/seaweedfs-images.json"), "utf8"));
const seaweedManifest = "sha256:3bbe24f6d5f5818327adcfeda7d85240ed53212dab05f91af14484c6446ec5eb";
if (images.schema_version !== "agentic-sandbox.celld-seaweedfs-images/v1" || images.platform !== "linux/amd64" || images.seaweedfs.version !== "4.41" || images.seaweedfs.manifest_digest !== seaweedManifest) throw new Error("reviewed SeaweedFS image inventory changed");
for (const path of ["deploy/celld/qualification/seaweedfs-protocol.compose.yml", "deploy/celld/qualification/seaweedfs-titan.compose.yml"]) {
  const compose = readFileSync(resolve(root, path), "utf8");
  if (!compose.includes(`docker.io/chrislusf/seaweedfs@${seaweedManifest}`) || /chrislusf\/seaweedfs:(?:latest|4\.)/.test(compose)) throw new Error(`${path}: SeaweedFS must use only the reviewed manifest digest`);
}
return { status: "PASS", schemas: schemas.length, uat_schemas: uatSchemas.length, authority_fields: authority.fields.length, celld_version: fleet.celld.version, seaweedfs_version: images.seaweedfs.version, seaweedfs_manifest: seaweedManifest, worker_digest: digest };
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) console.log(JSON.stringify(runContractChecks()));
