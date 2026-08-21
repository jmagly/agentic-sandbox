#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
export const CELLD_ROLLOUT_CANDIDATES_PATH = resolve(REPO_ROOT, "deploy/celld/qualification/celld-rollout-candidates.json");
const EXPECTED_DOCUMENT = Object.freeze({
  schema_version: "agentic-sandbox.celld-rollout-candidates/v1",
  platform: "linux/amd64",
  candidates: [{
    product: "Celld",
    version: "0.3.0",
    commit: "89e4ffc53a14ecb496d2ca5014ff9d19b0061ad9",
    image: "ghcr.io/denoland/celld",
    index_digest: "sha256:f47d97c2980aa98aef1d9c42205a313442f48acb606c5987dbb9b32983a23aaf",
    manifest_digest: "sha256:e2983741d4733a537dcdb399671d3ce2f6968bfe4f15ce0a70c0279e10a930d1",
    config_digest: "sha256:3c00ae8451c239dfb9a00447ccab4146160dc3136c74f56338dd839477b74725",
    qualification_status: "reviewed_unqualified",
    compatible_from_versions: ["0.2.1"],
    release: { tag: "v0.3.0", published_at: "2026-08-20T11:58:04Z", url: "https://github.com/denoland/celld/releases/tag/v0.3.0" },
    provenance: {
      predicate_type: "https://slsa.dev/provenance/v1",
      subject_name: "ghcr.io/denoland/celld",
      subject_digest: "sha256:f47d97c2980aa98aef1d9c42205a313442f48acb606c5987dbb9b32983a23aaf",
      signer_identity: "https://github.com/denoland/celld/.github/workflows/release.yml@refs/tags/v0.3.0",
      source_repository: "https://github.com/denoland/celld",
      source_ref: "refs/tags/v0.3.0",
      source_commit: "89e4ffc53a14ecb496d2ca5014ff9d19b0061ad9",
      runner_environment: "github-hosted",
      transparency_log: "https://rekor.sigstore.dev",
    },
  }],
});

function sha256(value) { return createHash("sha256").update(value).digest("hex"); }
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

export function validateRolloutCandidates(document) {
  return canonical(document) === canonical(EXPECTED_DOCUMENT) ? [] : ["reviewed rollout candidate inventory does not match the exact approved research record"];
}

export function loadReviewedRolloutCandidates(path = CELLD_ROLLOUT_CANDIDATES_PATH) {
  if (resolve(path) !== CELLD_ROLLOUT_CANDIDATES_PATH) throw new Error("rollout candidate inventory path is not the reviewed repository path");
  const document = JSON.parse(readFileSync(path, "utf8"));
  const errors = validateRolloutCandidates(document);
  if (errors.length) throw new Error(errors.join("; "));
  return structuredClone(document.candidates);
}

function argument(args, name) {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`${name} is required`);
  return args[index + 1];
}

function main(args) {
  if (args[0] !== "check") throw new Error("usage: celld-rollout-candidate.mjs check --input FILE");
  const candidates = loadReviewedRolloutCandidates(resolve(argument(args, "--input")));
  process.stdout.write(`${JSON.stringify({ status: "PASS", candidates: candidates.length, versions: candidates.map((candidate) => candidate.version), qualification_statuses: candidates.map((candidate) => candidate.qualification_status) })}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try { main(process.argv.slice(2)); }
  catch (error) { process.stderr.write(`CELLD_ROLLOUT_CANDIDATE_ERROR ${sha256(error.message)}\n`); process.exitCode = 3; }
}
