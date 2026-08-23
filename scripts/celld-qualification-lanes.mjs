#!/usr/bin/env node

import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const ALL_AUTOMATED_IDS = Object.freeze(Array.from({ length: 13 }, (_value, index) => `UAT-CELLD-${String(index + 3).padStart(3, "0")}`));
const DRIVER_KEYS = Object.freeze(["orchestration", "worker", "networkAuth", "credentialProvenance", "rollout", "observability", "recovery", "storageTopology"]);

function drivers(enabled = []) {
  const selected = new Set(enabled);
  return Object.freeze(Object.fromEntries(DRIVER_KEYS.map((key) => [key, selected.has(key)])));
}

export const QUALIFICATION_LANES = Object.freeze({
  complete: Object.freeze({
    issue: "#772",
    selectedIds: ALL_AUTOMATED_IDS,
    runCatalog: true,
    runMigration: true,
    drivers: drivers(DRIVER_KEYS),
  }),
  "issue-764": Object.freeze({
    issue: "#764",
    selectedIds: Object.freeze(["UAT-CELLD-003", "UAT-CELLD-004", "UAT-CELLD-005", "UAT-CELLD-006"]),
    runCatalog: true,
    runMigration: false,
    drivers: drivers(["orchestration"]),
  }),
  "issue-765": Object.freeze({
    issue: "#765",
    selectedIds: Object.freeze(["UAT-CELLD-010", "UAT-CELLD-012"]),
    runCatalog: true,
    runMigration: false,
    drivers: drivers(["networkAuth", "storageTopology"]),
  }),
  "issue-767": Object.freeze({
    issue: "#767",
    selectedIds: Object.freeze(["UAT-CELLD-007", "UAT-CELLD-008", "UAT-CELLD-009"]),
    runCatalog: true,
    runMigration: false,
    drivers: drivers(["worker"]),
  }),
  "issue-771": Object.freeze({
    issue: "#771",
    selectedIds: Object.freeze([]),
    runCatalog: false,
    runMigration: true,
    drivers: drivers(),
  }),
});

export function resolveQualificationLane(name) {
  const lane = QUALIFICATION_LANES[name];
  if (!lane) throw new Error(`unknown qualification lane: ${name}`);
  const selectedIds = [...lane.selectedIds];
  if (lane.runCatalog && selectedIds.length === 0) throw new Error("catalog lane has no selected UAT IDs");
  if (!lane.runCatalog && selectedIds.length !== 0) throw new Error("non-catalog lane cannot select UAT IDs");
  if (new Set(selectedIds).size !== selectedIds.length || selectedIds.some((id) => !ALL_AUTOMATED_IDS.includes(id))) throw new Error("qualification lane contains an invalid or duplicate UAT ID");
  return { name, issue: lane.issue, selectedIds, runCatalog: lane.runCatalog, runMigration: lane.runMigration, drivers: { ...lane.drivers } };
}

export function renderQualificationGithubEnv(name) {
  const lane = resolveQualificationLane(name);
  const values = {
    CELLD_QUALIFICATION_SELECTED_IDS: lane.selectedIds.join(","),
    CELLD_QUALIFICATION_EXPECTED_COUNT: String(lane.selectedIds.length),
    CELLD_QUALIFICATION_RUN_CATALOG: String(lane.runCatalog),
    CELLD_QUALIFICATION_RUN_MIGRATION: String(lane.runMigration),
    CELLD_QUALIFICATION_ENABLE_ORCHESTRATION: String(lane.drivers.orchestration),
    CELLD_QUALIFICATION_ENABLE_WORKER: String(lane.drivers.worker),
    CELLD_QUALIFICATION_ENABLE_NETWORK_AUTH: String(lane.drivers.networkAuth),
    CELLD_QUALIFICATION_ENABLE_CREDENTIAL_PROVENANCE: String(lane.drivers.credentialProvenance),
    CELLD_QUALIFICATION_ENABLE_ROLLOUT: String(lane.drivers.rollout),
    CELLD_QUALIFICATION_ENABLE_OBSERVABILITY: String(lane.drivers.observability),
    CELLD_QUALIFICATION_ENABLE_RECOVERY: String(lane.drivers.recovery),
    CELLD_QUALIFICATION_ENABLE_STORAGE_TOPOLOGY: String(lane.drivers.storageTopology),
  };
  return `${Object.entries(values).map(([key, value]) => `${key}=${value}`).join("\n")}\n`;
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function main(args) {
  const lane = argument(args, "--lane");
  const format = argument(args, "--format");
  if (format === "github-env") process.stdout.write(renderQualificationGithubEnv(lane));
  else if (format === "json") process.stdout.write(`${JSON.stringify(resolveQualificationLane(lane))}\n`);
  else throw new Error("--format must be github-env or json");
}

if (process.argv[1] && SCRIPT_PATH === resolve(process.argv[1])) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`CELLD_QUALIFICATION_LANE_ERROR ${error.message}\n`);
    process.exitCode = 3;
  }
}
