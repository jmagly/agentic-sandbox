import assert from "node:assert/strict";
import test from "node:test";

import { evaluateTitanPreflight } from "../../../scripts/celld-titan-preflight.mjs";

function passingSnapshot() {
  const tools = Object.fromEntries([
    "cargo",
    "docker",
    "jq",
    "make",
    "mkfs.xfs",
    "node",
    "python3",
    "qemu-img",
    "rustc",
    "sha256sum",
    "genisoimage",
    "timeout",
    "virsh",
    "xfs_quota",
  ].map((tool) => [tool, true]));
  return {
    evidence_schema: "agentic-sandbox.celld-titan-preflight/v1",
    collected_at: "2026-08-16T00:00:00.000Z",
    host: {
      hostname: "titan.s9.internal",
      short_hostname: "titan",
      expected_short_hostname: "titan",
      os: "Linux",
      kernel: "test",
      architecture: "x64",
      cpu_count: 32,
      memory_available_bytes: 128 * 1024 ** 3,
    },
    git: { commit: "a".repeat(40), expected_commit: "a".repeat(40), clean: true },
    storage: {
      root: { path: "/", free_bytes: 200 * 1024 ** 3 },
      build: { path: "/build", free_bytes: 800 * 1024 ** 3 },
      vm_root: { path: "/build/agentic-sandbox/vms", readable: true, writable: true },
    },
    capabilities: {
      kvm_readable: true,
      kvm_writable: true,
      libvirt: true,
      docker: true,
      sudo_noninteractive: true,
      virtiofsd: { path: "/usr/libexec/virtiofsd", executable: true },
      uefi: {
        code_path: "/usr/share/OVMF/OVMF_CODE_4M.fd",
        vars_path: "/usr/share/OVMF/OVMF_VARS_4M.fd",
        code_readable: true,
        vars_readable: true,
      },
      tools,
      optional_tools: { fio: true, k6: true, sqlite3: true, "stress-ng": true },
    },
    base_image: {
      path: "/build/agentic-sandbox/base-images/ubuntu-server-24.04-agent.qcow2",
      manifest_path: "/build/agentic-sandbox/base-images/manifest.json",
      actual_size_bytes: 10,
      expected_size_bytes: 10,
      actual_sha256: "b".repeat(64),
      expected_sha256: "b".repeat(64),
      verified: true,
    },
    thresholds: {
      min_root_free_bytes: 100 * 1024 ** 3,
      min_build_free_bytes: 400 * 1024 ** 3,
      min_memory_available_bytes: 32 * 1024 ** 3,
      min_cpu_count: 8,
    },
  };
}

test("Titan preflight passes a qualified clean host snapshot", () => {
  const result = evaluateTitanPreflight(passingSnapshot());
  assert.equal(result.status, "PASS");
  assert.equal(result.reason_code, "titan.preflight_passed");
  assert.ok(result.checks.every((check) => check.status === "PASS"));
});

test("Titan preflight fails closed on the wrong runner or inadequate build space", () => {
  const snapshot = passingSnapshot();
  snapshot.host.short_hostname = "build01";
  snapshot.storage.build.free_bytes = 399 * 1024 ** 3;
  const result = evaluateTitanPreflight(snapshot);
  assert.equal(result.status, "FAIL");
  assert.equal(result.reason_code, "titan.prerequisite_failed");
  assert.deepEqual(
    result.checks.filter((check) => check.status === "FAIL").map((check) => check.id),
    ["host.identity", "storage.build"],
  );
});

test("Titan preflight treats provenance, KVM, and required tooling as hard gates", () => {
  const snapshot = passingSnapshot();
  snapshot.base_image.verified = false;
  snapshot.capabilities.kvm_writable = false;
  snapshot.capabilities.tools.virsh = false;
  const result = evaluateTitanPreflight(snapshot);
  const failures = result.checks.filter((check) => check.status === "FAIL").map((check) => check.id);
  assert.deepEqual(failures, ["kvm.access", "base_image.provenance", "tool.virsh"]);
});

test("Titan preflight binds the run to the dispatched commit", () => {
  const snapshot = passingSnapshot();
  snapshot.git.expected_commit = "c".repeat(40);
  const result = evaluateTitanPreflight(snapshot);
  assert.deepEqual(
    result.checks.filter((check) => check.status === "FAIL").map((check) => check.id),
    ["git.expected_commit"],
  );
});
