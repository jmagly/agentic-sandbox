#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  accessSync,
  chmodSync,
  constants,
  createReadStream,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  statfsSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { arch, hostname, release, type } from "node:os";
import { fileURLToPath } from "node:url";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const GIB = 1024 ** 3;
const { R_OK, W_OK, X_OK } = constants;
const REQUIRED_TOOLS = Object.freeze([
  "cargo",
  "docker",
  "jq",
  "make",
  "mkfs.xfs",
  "nft",
  "node",
  "nsenter",
  "openssl",
  "python3",
  "qemu-img",
  "rustc",
  "sha256sum",
  "sqlite3",
  "genisoimage",
  "timeout",
  "virsh",
  "xfs_quota",
]);
const OPTIONAL_TOOLS = Object.freeze(["fio", "k6", "stress-ng"]);

function run(program, args, timeout = 15_000) {
  const result = spawnSync(program, args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    shell: false,
    timeout,
  });
  return {
    ok: result.status === 0,
    exit_code: result.status,
    output: `${result.stdout || ""}${result.stderr || ""}`.trim().slice(0, 2_000),
  };
}

function executableAvailable(name) {
  const result = run(name, ["--version"], 5_000);
  if (result.ok) return true;
  // Some tools use a non-zero status for --version but still executed. ENOENT
  // is represented by a null status and an empty child output.
  return result.exit_code !== null;
}

function availableMemoryBytes() {
  const text = readFileSync("/proc/meminfo", "utf8");
  const match = /^MemAvailable:\s+(\d+)\s+kB$/m.exec(text);
  if (!match) throw new Error("/proc/meminfo does not expose MemAvailable");
  return Number(match[1]) * 1024;
}

function freeBytes(path) {
  const stats = statfsSync(path, { bigint: true });
  return Number(stats.bavail * stats.bsize);
}

function canAccess(path, mode) {
  try {
    accessSync(path, mode);
    return true;
  } catch {
    return false;
  }
}

async function sha256File(path) {
  const hash = createHash("sha256");
  await new Promise((resolveHash, rejectHash) => {
    const stream = createReadStream(path);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", rejectHash);
    stream.on("end", resolveHash);
  });
  return hash.digest("hex");
}

function numericSetting(environment, key, fallback) {
  const value = environment[key] ?? String(fallback);
  if (!/^\d+$/.test(value) || Number(value) <= 0) throw new Error(`${key} must be a positive integer`);
  return Number(value);
}

function lines(output) {
  return output.split(/\r?\n/).map((value) => value.trim()).filter(Boolean).sort();
}

function inventory(program, args) {
  const result = spawnSync(program, args, { cwd: REPO_ROOT, encoding: "utf8", shell: false, timeout: 30_000, maxBuffer: 1024 * 1024 });
  return result.error || result.status !== 0 ? { ok: false, values: [] } : { ok: true, values: lines(result.stdout ?? "") };
}

function directoryInventory(path) {
  try { return { ok: true, values: readdirSync(path).sort() }; }
  catch { return { ok: false, values: [] }; }
}

export function collectTitanResourceBaseline(environment = process.env) {
  const vmRoot = resolve(environment.VM_STORAGE_DIR || "/build/agentic-sandbox/vms");
  const libvirtUri = environment.LIBVIRT_DEFAULT_URI || "qemu:///system";
  const observations = {
    docker_containers: inventory("docker", ["ps", "--all", "--format", "{{.Names}}"]),
    docker_networks: inventory("docker", ["network", "ls", "--format", "{{.Name}}"]),
    docker_volumes: inventory("docker", ["volume", "ls", "--format", "{{.Name}}"]),
    libvirt_domains: inventory("virsh", ["-c", libvirtUri, "list", "--all", "--name"]),
    vm_root_entries: directoryInventory(vmRoot),
    qualification_agentshare_entries: directoryInventory("/var/tmp"),
  };
  observations.qualification_agentshare_entries.values = observations.qualification_agentshare_entries.values.filter((name) => /^agentic-celld-qualification-[0-9]+$/.test(name));
  const errors = Object.entries(observations).filter(([, value]) => !value.ok).map(([key]) => key);
  return {
    complete: errors.length === 0,
    errors,
    ...Object.fromEntries(Object.entries(observations).map(([key, value]) => [key, value.values])),
  };
}

export function evaluateTitanPreflight(snapshot) {
  const t = snapshot.thresholds;
  const checks = [
    ["host.identity", snapshot.host.short_hostname === snapshot.host.expected_short_hostname, snapshot.host.short_hostname, snapshot.host.expected_short_hostname],
    ["host.cpu", snapshot.host.cpu_count >= t.min_cpu_count, snapshot.host.cpu_count, `>=${t.min_cpu_count}`],
    ["host.memory", snapshot.host.memory_available_bytes >= t.min_memory_available_bytes, snapshot.host.memory_available_bytes, `>=${t.min_memory_available_bytes}`],
    ["storage.root", snapshot.storage.root.free_bytes >= t.min_root_free_bytes, snapshot.storage.root.free_bytes, `>=${t.min_root_free_bytes}`],
    ["storage.build", snapshot.storage.build.free_bytes >= t.min_build_free_bytes, snapshot.storage.build.free_bytes, `>=${t.min_build_free_bytes}`],
    ["storage.vm_root", snapshot.storage.vm_root.readable && snapshot.storage.vm_root.writable, snapshot.storage.vm_root, "readable+writable"],
    ["resources.baseline", snapshot.resource_baseline?.complete === true, snapshot.resource_baseline?.errors ?? ["missing"], []],
    ["git.clean", snapshot.git.clean, snapshot.git.clean, true],
    ["git.commit", /^[0-9a-f]{40}$/.test(snapshot.git.commit), snapshot.git.commit, "40-character commit"],
    ["git.expected_commit", snapshot.git.expected_commit === null || snapshot.git.commit === snapshot.git.expected_commit, snapshot.git.commit, snapshot.git.expected_commit ?? "not constrained"],
    ["kvm.access", snapshot.capabilities.kvm_readable && snapshot.capabilities.kvm_writable, { readable: snapshot.capabilities.kvm_readable, writable: snapshot.capabilities.kvm_writable }, "readable+writable"],
    ["libvirt.connection", snapshot.capabilities.libvirt, snapshot.capabilities.libvirt, true],
    ["docker.connection", snapshot.capabilities.docker, snapshot.capabilities.docker, true],
    ["sudo.noninteractive", snapshot.capabilities.sudo_noninteractive, snapshot.capabilities.sudo_noninteractive, true],
    ["virtiofsd.executable", snapshot.capabilities.virtiofsd.executable, snapshot.capabilities.virtiofsd, "executable"],
    ["uefi.firmware", snapshot.capabilities.uefi.code_readable && snapshot.capabilities.uefi.vars_readable, snapshot.capabilities.uefi, "readable code+vars"],
    ["base_image.provenance", snapshot.base_image.verified, snapshot.base_image, "manifest size and sha256 match"],
  ];
  for (const tool of REQUIRED_TOOLS) {
    checks.push([`tool.${tool}`, snapshot.capabilities.tools[tool] === true, snapshot.capabilities.tools[tool] === true, true]);
  }
  const normalized = checks.map(([id, passed, observed, expected]) => ({
    id,
    status: passed ? "PASS" : "FAIL",
    observed,
    expected,
  }));
  const failed = normalized.filter((check) => check.status === "FAIL");
  return {
    ...snapshot,
    status: failed.length === 0 ? "PASS" : "FAIL",
    reason_code: failed.length === 0 ? "titan.preflight_passed" : "titan.prerequisite_failed",
    checks: normalized,
  };
}

export async function collectTitanSnapshot(environment = process.env) {
  const buildRoot = resolve(environment.CELLD_QUALIFICATION_BUILD_ROOT || "/build");
  const vmRoot = resolve(environment.VM_STORAGE_DIR || "/build/agentic-sandbox/vms");
  const baseImage = resolve(environment.BASE_IMAGES_DIR || "/build/agentic-sandbox/base-images", "ubuntu-server-24.04-agent.qcow2");
  const manifestPath = resolve(dirname(baseImage), "manifest.json");
  const expectedHost = environment.CELLD_QUALIFICATION_EXPECTED_HOST || "titan";
  const actualHostname = hostname();
  const gitCommit = run("git", ["rev-parse", "HEAD"]).output.split(/\s+/)[0] || "";
  const gitStatus = run("git", ["status", "--porcelain=v1", "--untracked-files=all"]);
  const tools = Object.fromEntries([...REQUIRED_TOOLS, ...OPTIONAL_TOOLS].map((tool) => [tool, executableAvailable(tool)]));
  const virsh = run("virsh", ["-c", environment.LIBVIRT_DEFAULT_URI || "qemu:///system", "uri"]);
  const docker = run("docker", ["version", "--format", "client={{.Client.Version}} server={{.Server.Version}}"]) ;
  const sudoNoninteractive = run("sudo", ["-n", "true"], 5_000);
  const virtiofsdCandidates = ["/usr/libexec/virtiofsd", "/usr/lib/virtiofsd", "/usr/bin/virtiofsd"];
  const virtiofsdPath = virtiofsdCandidates.find((path) => canAccess(path, X_OK)) || null;
  const ovmfCodeCandidates = ["/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_CODE.fd"];
  const ovmfVarsCandidates = ["/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"];
  const ovmfCode = ovmfCodeCandidates.find((path) => canAccess(path, R_OK)) || null;
  const ovmfVars = ovmfVarsCandidates.find((path) => canAccess(path, R_OK)) || null;

  let manifestRecord = null;
  let actualDigest = null;
  let actualSize = null;
  if (canAccess(baseImage, R_OK) && canAccess(manifestPath, R_OK)) {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    manifestRecord = manifest[basename(baseImage)] || null;
    actualSize = statSync(baseImage).size;
    actualDigest = await sha256File(baseImage);
  }

  const minRootFreeGiB = numericSetting(environment, "CELLD_QUALIFICATION_MIN_ROOT_FREE_GIB", 100);
  const minBuildFreeGiB = numericSetting(environment, "CELLD_QUALIFICATION_MIN_BUILD_FREE_GIB", 400);
  const minMemoryGiB = numericSetting(environment, "CELLD_QUALIFICATION_MIN_MEMORY_AVAILABLE_GIB", 32);
  const minCpuCount = numericSetting(environment, "CELLD_QUALIFICATION_MIN_CPU_COUNT", 8);

  return {
    evidence_schema: "agentic-sandbox.celld-titan-preflight/v1",
    collected_at: new Date().toISOString(),
    host: {
      hostname: actualHostname,
      short_hostname: actualHostname.split(".")[0],
      expected_short_hostname: expectedHost.split(".")[0],
      os: type(),
      kernel: release(),
      architecture: arch(),
      cpu_count: Number(run("nproc", []).output.split(/\s+/)[0]) || 0,
      memory_available_bytes: availableMemoryBytes(),
    },
    git: {
      commit: gitCommit,
      expected_commit: environment.CELLD_QUALIFICATION_EXPECTED_COMMIT || null,
      clean: gitStatus.ok && gitStatus.output.length === 0,
    },
    storage: {
      root: { path: "/", free_bytes: freeBytes("/") },
      build: { path: buildRoot, free_bytes: freeBytes(buildRoot) },
      vm_root: { path: vmRoot, readable: canAccess(vmRoot, R_OK), writable: canAccess(vmRoot, W_OK) },
    },
    resource_baseline: collectTitanResourceBaseline(environment),
    capabilities: {
      kvm_readable: canAccess("/dev/kvm", R_OK),
      kvm_writable: canAccess("/dev/kvm", W_OK),
      libvirt: virsh.ok,
      docker: docker.ok,
      sudo_noninteractive: sudoNoninteractive.ok,
      virtiofsd: { path: virtiofsdPath, executable: virtiofsdPath !== null },
      uefi: {
        code_path: ovmfCode,
        vars_path: ovmfVars,
        code_readable: ovmfCode !== null,
        vars_readable: ovmfVars !== null,
      },
      tools,
      optional_tools: Object.fromEntries(OPTIONAL_TOOLS.map((tool) => [tool, tools[tool]])),
    },
    base_image: {
      path: baseImage,
      manifest_path: manifestPath,
      actual_size_bytes: actualSize,
      expected_size_bytes: manifestRecord?.size_bytes ?? null,
      actual_sha256: actualDigest,
      expected_sha256: manifestRecord?.sha256 ?? null,
      verified: Boolean(
        manifestRecord
        && actualSize === manifestRecord.size_bytes
        && actualDigest === manifestRecord.sha256
      ),
    },
    thresholds: {
      min_root_free_bytes: minRootFreeGiB * GIB,
      min_build_free_bytes: minBuildFreeGiB * GIB,
      min_memory_available_bytes: minMemoryGiB * GIB,
      min_cpu_count: minCpuCount,
    },
  };
}

function parseArgs(argv) {
  let output = resolve(REPO_ROOT, "artifacts/celld-titan-preflight.json");
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--output" && argv[index + 1]) {
      output = resolve(argv[index + 1]);
      index += 1;
    } else if (argv[index] === "--help") {
      return { help: true, output };
    } else {
      throw new Error(`unknown argument: ${argv[index]}`);
    }
  }
  return { help: false, output };
}

export async function main(argv = process.argv.slice(2), environment = process.env) {
  const options = parseArgs(argv);
  if (options.help) {
    console.log("Usage: node scripts/celld-titan-preflight.mjs [--output PATH]");
    return 0;
  }
  const result = evaluateTitanPreflight(await collectTitanSnapshot(environment));
  mkdirSync(dirname(options.output), { recursive: true, mode: 0o700 });
  writeFileSync(options.output, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
  chmodSync(options.output, 0o600);
  console.log(`Titan Celld preflight: ${result.status} (${result.reason_code})`);
  for (const check of result.checks) console.log(`${check.status} ${check.id}`);
  console.log(`Evidence: ${options.output}`);
  return result.status === "PASS" ? 0 : 2;
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try {
    process.exitCode = await main();
  } catch (error) {
    console.error(`Titan Celld preflight error: ${error.message}`);
    process.exitCode = 3;
  }
}
