#!/usr/bin/env node

import { chmodSync, mkdirSync, readFileSync, statfsSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { collectTitanResourceBaseline } from "./celld-titan-preflight.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const GIB = 1024 ** 3;

function run(program, args) {
  const result = spawnSync(program, args, { cwd: REPO_ROOT, encoding: "utf8", shell: false, timeout: 15_000 });
  if (result.error || result.status !== 0) throw new Error(`${program} postflight probe failed`);
  return result.stdout.trim();
}

function freeBytes(path) {
  const stats = statfsSync(path, { bigint: true });
  return Number(stats.bavail * stats.bsize);
}

function positiveInteger(environment, key, fallback) {
  const value = environment[key] ?? String(fallback);
  if (!/^\d+$/.test(value) || Number(value) <= 0) throw new Error(`${key} must be a positive integer`);
  return Number(value);
}

function same(left, right) { return JSON.stringify(left) === JSON.stringify(right); }

export function evaluateTitanPostflight(preflight, current, maxRetainedBytes) {
  if (preflight?.evidence_schema !== "agentic-sandbox.celld-titan-preflight/v1" || preflight.status !== "PASS") throw new Error("postflight requires passing preflight evidence");
  if (!preflight.resource_baseline || current?.evidence_schema !== "agentic-sandbox.celld-titan-postflight-snapshot/v1") throw new Error("postflight baseline evidence is incomplete");
  const checks = [
    ["host.identity", current.host.short_hostname === preflight.host.short_hostname, current.host.short_hostname, preflight.host.short_hostname],
    ["git.commit", current.git.commit === preflight.git.commit, current.git.commit, preflight.git.commit],
    ["storage.root.threshold", current.storage.root.free_bytes >= preflight.thresholds.min_root_free_bytes, current.storage.root.free_bytes, `>=${preflight.thresholds.min_root_free_bytes}`],
    ["storage.build.threshold", current.storage.build.free_bytes >= preflight.thresholds.min_build_free_bytes, current.storage.build.free_bytes, `>=${preflight.thresholds.min_build_free_bytes}`],
    ["storage.root.regression", current.storage.root.free_bytes >= preflight.storage.root.free_bytes - maxRetainedBytes, preflight.storage.root.free_bytes - current.storage.root.free_bytes, `<=${maxRetainedBytes} bytes retained`],
    ["storage.build.regression", current.storage.build.free_bytes >= preflight.storage.build.free_bytes - maxRetainedBytes, preflight.storage.build.free_bytes - current.storage.build.free_bytes, `<=${maxRetainedBytes} bytes retained`],
    ["resources.inventory_complete", current.resource_baseline.complete === true, current.resource_baseline.errors, []],
  ];
  for (const key of ["docker_containers", "docker_networks", "docker_volumes", "libvirt_domains", "vm_root_entries", "qualification_agentshare_entries"]) {
    checks.push([`resources.${key}`, same(current.resource_baseline[key], preflight.resource_baseline[key]), current.resource_baseline[key], preflight.resource_baseline[key]]);
  }
  const normalized = checks.map(([id, passed, observed, expected]) => ({ id, status: passed ? "PASS" : "FAIL", observed, expected }));
  const failed = normalized.filter((check) => check.status === "FAIL");
  return { ...current, baseline_collected_at: preflight.collected_at, max_retained_bytes: maxRetainedBytes, status: failed.length ? "FAIL" : "PASS", reason_code: failed.length ? "titan.postflight_baseline_failed" : "titan.postflight_baseline_restored", checks: normalized };
}

export function collectTitanPostflight(environment = process.env) {
  const buildRoot = resolve(environment.CELLD_QUALIFICATION_BUILD_ROOT || "/build");
  return {
    evidence_schema: "agentic-sandbox.celld-titan-postflight-snapshot/v1",
    collected_at: new Date().toISOString(),
    host: { short_hostname: hostname().split(".")[0] },
    git: { commit: run("git", ["rev-parse", "HEAD"]) },
    storage: { root: { path: "/", free_bytes: freeBytes("/") }, build: { path: buildRoot, free_bytes: freeBytes(buildRoot) } },
    resource_baseline: collectTitanResourceBaseline(environment),
  };
}

function argument(argv, name, fallback = null) {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : fallback;
}

export function main(argv = process.argv.slice(2), environment = process.env) {
  const preflightArgument = argument(argv, "--preflight", null);
  if (!preflightArgument) throw new Error("usage: celld-titan-postflight.mjs --preflight FILE [--output FILE]");
  const preflightPath = resolve(preflightArgument);
  const outputPath = resolve(argument(argv, "--output", "artifacts/celld-titan-postflight.json"));
  if (argv.some((value) => value.startsWith("--") && !["--preflight", "--output"].includes(value)) || argv.length % 2 !== 0) throw new Error("usage: celld-titan-postflight.mjs --preflight FILE [--output FILE]");
  const preflight = JSON.parse(readFileSync(preflightPath, "utf8"));
  const maxRetainedBytes = positiveInteger(environment, "CELLD_QUALIFICATION_MAX_RETAINED_GIB", 10) * GIB;
  const result = evaluateTitanPostflight(preflight, collectTitanPostflight(environment), maxRetainedBytes);
  mkdirSync(dirname(outputPath), { recursive: true, mode: 0o700 });
  writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
  chmodSync(outputPath, 0o600);
  console.log(`Titan Celld postflight: ${result.status} (${result.reason_code})`);
  for (const check of result.checks) console.log(`${check.status} ${check.id}`);
  return result.status === "PASS" ? 0 : 4;
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try { process.exitCode = main(); }
  catch (error) { console.error(`Titan Celld postflight error: ${error.message}`); process.exitCode = 4; }
}
