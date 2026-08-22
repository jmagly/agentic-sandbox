#!/usr/bin/env node

import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

export const FIXTURE_SCHEMA = "agentic-sandbox.celld-seaweedfs-fixture/v1";
export const FIXTURE_PROFILES = Object.freeze({
  "single-process-protocol": Object.freeze({
    promoting: false,
    topology: "single-process-protocol",
    compose: "deploy/celld/qualification/seaweedfs-protocol.compose.yml",
    gateway_count: 1,
    tls: false,
    limits: Object.freeze({ create_rounds: 100, overwrite_rounds: 100, contenders: 2, warmups: 0, max_workers: 4, max_connections: 8 }),
  }),
  "titan-single-host-storage": Object.freeze({
    promoting: true,
    topology: "titan-single-host-storage:3-master,3-rack-volume,3-postgres-filer,2-tls-s3-gateway",
    compose: "deploy/celld/qualification/seaweedfs-titan.compose.yml",
    gateway_count: 2,
    tls: true,
    limits: Object.freeze({ create_rounds: 10_000, overwrite_rounds: 10_000, contenders: 2, warmups: 100, max_workers: 16, max_connections: 32 }),
  }),
});

const REPO_ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const SAFE_PROJECT = /^celld-s3-[a-z0-9-]{1,48}$/;
const CONFIG_KEYS = new Set([
  "schema_version", "fixture_profile", "promoting", "run_id", "run_root", "compose_file",
  "project", "bucket", "run_prefix", "region", "ca_file_ref", "identity_file_ref",
  "admin_identity_file_ref", "revoked_identity_file_ref", "backend", "limits", "supported_failures", "unsupported_failures",
]);

export class FixtureCleanupError extends Error {
  constructor(message) {
    super(message);
    this.name = "FixtureCleanupError";
    this.exitCode = 4;
  }
}

function errorDigest(error) {
  return createHash("sha256").update(String(error?.message ?? error)).digest("hex");
}

function privateWrite(path, value) {
  writeFileSync(path, value, { mode: 0o600, flag: "wx" });
  chmodSync(path, 0o600);
}

function randomText(bytes = 24) {
  return randomBytes(bytes).toString("base64url");
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", shell: false, ...options });
  if (result.error || result.status !== 0) throw new Error(`${program} failed: ${(result.error?.message ?? result.stderr ?? "").trim()}`);
  return result.stdout.trim();
}

function compose(config, args, runner = run, timeout = 600_000) {
  return runner("docker", ["compose", "-f", config.compose_file, "-p", config.project, ...args], {
    env: fixtureEnvironment(config),
    timeout,
  });
}

function createTls(root, runId) {
  const tls = join(root, "tls");
  mkdirSync(tls, { mode: 0o700 });
  const caKey = join(tls, "ca.key");
  const caCrt = join(tls, "ca.crt");
  const serverKey = join(tls, "server.key");
  const serverCsr = join(tls, "server.csr");
  const serverCrt = join(tls, "server.crt");
  const extensions = join(tls, "server-ext.cnf");
  // Loopback clients and Celld nodes on the private Compose network use the
  // same certificate. Pin the service DNS names instead of weakening TLS
  // endpoint verification inside the fleet containers.
  privateWrite(extensions, "subjectAltName=DNS:localhost,DNS:s3gateway1,DNS:s3gateway2,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n");
  run("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", caKey]);
  run("openssl", ["req", "-x509", "-new", "-key", caKey, "-sha256", "-days", "2", "-subj", `/CN=celld-storage-ca-${runId}`, "-out", caCrt]);
  run("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", serverKey]);
  run("openssl", ["req", "-new", "-key", serverKey, "-subj", "/CN=localhost", "-out", serverCsr]);
  run("openssl", ["x509", "-req", "-in", serverCsr, "-CA", caCrt, "-CAkey", caKey, "-CAcreateserial", "-days", "2", "-sha256", "-extfile", extensions, "-out", serverCrt]);
  const managementTls = join(root, "management-tls");
  mkdirSync(managementTls, { mode: 0o700 });
  const managementServerKey = join(managementTls, "management-server.key");
  const managementServerCsr = join(managementTls, "management-server.csr");
  const managementServerCrt = join(managementTls, "management-server.crt");
  const managementServerExtensions = join(managementTls, "management-server-ext.cnf");
  privateWrite(managementServerExtensions, "subjectAltName=DNS:management.internal\nextendedKeyUsage=serverAuth\n");
  run("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", managementServerKey]);
  run("openssl", ["req", "-new", "-key", managementServerKey, "-subj", "/CN=management.internal", "-out", managementServerCsr]);
  run("openssl", ["x509", "-req", "-in", managementServerCsr, "-CA", caCrt, "-CAkey", caKey, "-CAserial", join(tls, "ca.srl"), "-days", "2", "-sha256", "-extfile", managementServerExtensions, "-out", managementServerCrt]);
  const callbackClientKey = join(managementTls, "callback-client.key");
  const callbackClientCsr = join(managementTls, "callback-client.csr");
  const callbackClientCrt = join(managementTls, "callback-client.crt");
  const callbackClientExtensions = join(managementTls, "callback-client-ext.cnf");
  privateWrite(callbackClientExtensions, "extendedKeyUsage=clientAuth\n");
  run("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", callbackClientKey]);
  run("openssl", ["req", "-new", "-key", callbackClientKey, "-subj", "/CN=agentic-celld-worker-callback", "-out", callbackClientCsr]);
  run("openssl", ["x509", "-req", "-in", callbackClientCsr, "-CA", caCrt, "-CAkey", caKey, "-CAserial", join(tls, "ca.srl"), "-days", "2", "-sha256", "-extfile", callbackClientExtensions, "-out", callbackClientCrt]);
  for (const path of [
    caKey, caCrt, serverKey, serverCsr, serverCrt, extensions, join(tls, "ca.srl"),
    managementServerKey, managementServerCsr, managementServerCrt, managementServerExtensions,
    callbackClientKey, callbackClientCsr, callbackClientCrt, callbackClientExtensions,
  ]) chmodSync(path, 0o600);
  return caCrt;
}

function loadImages() {
  const value = JSON.parse(readFileSync(join(REPO_ROOT, "deploy/celld/qualification/seaweedfs-images.json"), "utf8"));
  if (value.schema_version !== "agentic-sandbox.celld-seaweedfs-images/v1" || value.platform !== "linux/amd64") throw new Error("SeaweedFS image inventory is invalid");
  if (value.seaweedfs.version !== "4.41" || value.seaweedfs.manifest_digest !== "sha256:3bbe24f6d5f5818327adcfeda7d85240ed53212dab05f91af14484c6446ec5eb") throw new Error("SeaweedFS reviewed pin changed without fixture review");
  return value;
}

function configurationDigest(profileName, profile) {
  const hash = createHash("sha256");
  hash.update(readFileSync(join(REPO_ROOT, profile.compose)));
  hash.update(readFileSync(join(REPO_ROOT, "deploy/celld/qualification/seaweedfs-images.json")));
  hash.update(JSON.stringify({ profile_name: profileName, topology: profile.topology, gateway_count: profile.gateway_count, tls: profile.tls, limits: profile.limits }));
  return hash.digest("hex");
}

export function validateFixtureConfig(config) {
  const errors = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) return ["config must be an object"];
  for (const key of Object.keys(config)) if (!CONFIG_KEYS.has(key)) errors.push(`config.${key} is not allowed`);
  if (config.schema_version !== FIXTURE_SCHEMA) errors.push(`config.schema_version must be ${FIXTURE_SCHEMA}`);
  const profile = FIXTURE_PROFILES[config.fixture_profile];
  if (!profile) errors.push("config.fixture_profile is unsupported");
  if (profile && config.promoting !== profile.promoting) errors.push("config.promoting does not match the named profile");
  if (!RUN_ID.test(config.run_id ?? "")) errors.push("config.run_id is invalid");
  if (!isAbsolute(config.run_root ?? "") || !config.run_root.split("/").includes(config.run_id)) errors.push("config.run_root must be absolute and contain run_id as a path segment");
  if (profile?.promoting && !config.run_root.startsWith("/dev/shm/")) errors.push("promoting fixture run_root must use the /dev/shm tmpfs");
  if (!isAbsolute(config.compose_file ?? "") || config.compose_file !== join(REPO_ROOT, profile?.compose ?? "invalid")) errors.push("config.compose_file must match the named repository fixture");
  if (!SAFE_PROJECT.test(config.project ?? "")) errors.push("config.project is invalid");
  if (!/^celld-[a-z0-9]{24}$/.test(config.bucket ?? "")) errors.push("config.bucket is invalid");
  if (config.run_prefix !== `qualification/${config.run_id}`) errors.push("config.run_prefix must bind the exact run ID");
  if (config.region !== "us-east-1") errors.push("config.region must be us-east-1 for the fixture");
  const expectedFiles = { identity_file_ref: "identity", admin_identity_file_ref: "identity-admin", revoked_identity_file_ref: "identity-revoked" };
  for (const [key, name] of Object.entries(expectedFiles)) if (!isAbsolute(config[key] ?? "") || config[key] !== join(config.run_root ?? "", name)) errors.push(`config.${key} must be the fixed run-root path`);
  if (profile?.tls !== Boolean(config.ca_file_ref) || (config.ca_file_ref && config.ca_file_ref !== join(config.run_root ?? "", "tls/ca.crt"))) errors.push("config.ca_file_ref does not match profile TLS mode");
  if (config.ca_file_ref && !isAbsolute(config.ca_file_ref)) errors.push("config.ca_file_ref must be absolute");
  if (config.backend?.product !== "SeaweedFS" || config.backend?.version !== "4.41" || config.backend?.artifact_sha256 !== "3bbe24f6d5f5818327adcfeda7d85240ed53212dab05f91af14484c6446ec5eb" || !SHA256.test(config.backend?.artifact_sha256 ?? "") || config.backend?.configuration_sha256 !== (profile ? configurationDigest(config.fixture_profile, profile) : "") || config.backend?.topology !== profile?.topology || config.backend?.gateway_count !== profile?.gateway_count) errors.push("config.backend does not match the reviewed profile");
  if (JSON.stringify(config.limits) !== JSON.stringify(profile?.limits)) errors.push("config.limits do not match the named profile ceilings");
  if (!Array.isArray(config.supported_failures) || !Array.isArray(config.unsupported_failures)) errors.push("config failure boundaries must be arrays");
  return errors;
}

export function prepareFixture({ fixtureProfile, runId, root }) {
  const profile = FIXTURE_PROFILES[fixtureProfile];
  if (!profile) throw new Error(`unsupported fixture profile: ${fixtureProfile}`);
  if (!RUN_ID.test(runId ?? "")) throw new Error("run ID is invalid");
  const runRoot = resolve(root ?? "");
  if (!isAbsolute(root ?? "") || !runRoot.split("/").includes(runId)) throw new Error("run root must be absolute and contain run ID as a path segment");
  if (profile.promoting && !runRoot.startsWith("/dev/shm/")) throw new Error("promoting fixture run root must use the /dev/shm tmpfs");
  if (existsSync(runRoot)) throw new Error("run root already exists");
  mkdirSync(runRoot, { recursive: true, mode: 0o700 });
  chmodSync(runRoot, 0o700);
  try {
    const images = loadImages();
    const suffix = randomBytes(12).toString("hex");
    const bucket = `celld-${suffix}`;
    const project = `celld-s3-${suffix.slice(0, 16)}`;
    const userAccess = `CELLD${randomBytes(10).toString("hex").toUpperCase()}`;
    const userSecret = randomText(32);
    const adminAccess = `ADMIN${randomBytes(10).toString("hex").toUpperCase()}`;
    const adminSecret = randomText(32);
    const postgresPassword = randomText(32);
    privateWrite(join(runRoot, "identity"), `[default]\naws_access_key_id = ${userAccess}\naws_secret_access_key = ${userSecret}\n`);
    privateWrite(join(runRoot, "identity-admin"), `[default]\naws_access_key_id = ${adminAccess}\naws_secret_access_key = ${adminSecret}\n`);
    privateWrite(join(runRoot, "identity-revoked"), `[default]\naws_access_key_id = REVOKED${randomBytes(8).toString("hex").toUpperCase()}\naws_secret_access_key = ${randomText(32)}\n`);
    privateWrite(join(runRoot, "access-key"), `${userAccess}\n`);
    privateWrite(join(runRoot, "secret-key"), `${userSecret}\n`);
    privateWrite(join(runRoot, "postgres-password"), `${postgresPassword}\n`);
    privateWrite(join(runRoot, "s3.json"), `${JSON.stringify({ identities: [
      { name: "fixture-administrator", credentials: [{ accessKey: adminAccess, secretKey: adminSecret }], actions: ["Admin", "Read", "List", "Tagging", "Write"] },
      { name: "run-bucket", credentials: [{ accessKey: userAccess, secretKey: userSecret }], actions: [`Read:${bucket}`, `List:${bucket}`, `Tagging:${bucket}`, `Write:${bucket}`] },
    ] }, null, 2)}\n`);
    privateWrite(join(runRoot, "filer.toml"), `[postgres2]\nenabled = true\nhostname = "postgres"\nport = 5432\nusername = "seaweedfs"\npassword = ${JSON.stringify(postgresPassword)}\ndatabase = "seaweedfs"\nschema = ""\nsslmode = "disable"\nconnection_max_idle = 10\nconnection_max_open = 50\nconnection_max_lifetime_seconds = 300\nenableUpsert = true\n`);
    const caFile = profile.tls ? createTls(runRoot, runId) : null;
    const config = {
      schema_version: FIXTURE_SCHEMA,
      fixture_profile: fixtureProfile,
      promoting: profile.promoting,
      run_id: runId,
      run_root: runRoot,
      compose_file: join(REPO_ROOT, profile.compose),
      project,
      bucket,
      run_prefix: `qualification/${runId}`,
      region: "us-east-1",
      ca_file_ref: caFile,
      identity_file_ref: join(runRoot, "identity"),
      admin_identity_file_ref: join(runRoot, "identity-admin"),
      revoked_identity_file_ref: join(runRoot, "identity-revoked"),
      backend: {
        product: images.seaweedfs.product,
        version: images.seaweedfs.version,
        artifact_sha256: images.seaweedfs.manifest_digest.slice("sha256:".length),
        configuration_sha256: configurationDigest(fixtureProfile, profile),
        topology: profile.topology,
        gateway_count: profile.gateway_count,
      },
      limits: { ...profile.limits },
      supported_failures: fixtureProfile === "titan-single-host-storage" ? ["one-master", "one-volume-server", "one-s3-gateway"] : [],
      unsupported_failures: fixtureProfile === "titan-single-host-storage" ? ["postgres-metadata-service", "physical-host", "rack", "availability-zone"] : ["any-component", "cross-gateway"],
    };
    const errors = validateFixtureConfig(config);
    if (errors.length) throw new Error(errors.join("; "));
    privateWrite(join(runRoot, "fixture.json"), `${JSON.stringify(config, null, 2)}\n`);
    privateWrite(join(runRoot, ".agentic-celld-seaweedfs-run"), `${JSON.stringify({ run_id: runId, project, bucket, fixture_profile: fixtureProfile })}\n`);
    return config;
  } catch (error) {
    rmSync(runRoot, { recursive: true, force: true });
    throw error;
  }
}

export function fixtureEnvironment(config) {
  const errors = validateFixtureConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  return {
    ...process.env,
    CELLD_SEAWEED_PROJECT: config.project,
    CELLD_SEAWEED_RUN_ID: config.run_id,
    CELLD_SEAWEED_RUN_ROOT: config.run_root,
    CELLD_SEAWEED_BUCKET: config.bucket,
  };
}

export function startFixture(config, { runner = run } = {}) {
  const errors = validateFixtureConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  compose(config, ["pull", "--quiet"], runner, 900_000);
  compose(config, ["up", "-d", "--wait", "--wait-timeout", "240"], runner, 600_000);
  const running = compose(config, ["ps", "--services", "--status", "running"], runner)
    .split(/\r?\n/)
    .filter(Boolean);
  const required = config.fixture_profile === "titan-single-host-storage"
    ? ["postgres", "master1", "master2", "master3", "volume1", "volume2", "volume3", "filer1", "filer2", "filer3", "s3gateway1", "s3gateway2"]
    : ["seaweedfs"];
  const missing = required.filter((service) => !running.includes(service));
  if (missing.length) throw new Error(`storage fixture services are not running: ${missing.join(",")}`);
  return {
    status: "READY",
    run_id: config.run_id,
    fixture_profile: config.fixture_profile,
    scope: config.promoting ? "live_candidate" : "fixture_reduced",
    services: required,
  };
}

export function parseComposePs(value) {
  const text = value.trim();
  if (!text) return [];
  try {
    const parsed = JSON.parse(text);
    return Array.isArray(parsed) ? parsed : [parsed];
  } catch {
    return text.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  }
}

function redactFixtureText(config, value) {
  let redacted = value;
  for (const name of ["access-key", "secret-key", "postgres-password"]) {
    const path = join(config.run_root, name);
    if (!existsSync(path)) continue;
    const secret = readFileSync(path, "utf8").trim();
    if (secret) redacted = redacted.replaceAll(secret, "[REDACTED]");
  }
  return redacted;
}

export function collectFixtureDiagnostics(config, { runner = run } = {}) {
  const errors = validateFixtureConfig(config);
  if (errors.length) throw new Error(errors.join("; "));
  const services = parseComposePs(compose(config, ["ps", "--all", "--format", "json"], runner));
  const primaryFailures = services
    .filter((service) => service.State === "exited" || service.Health === "unhealthy")
    .map((service) => service.Service)
    .filter((service) => typeof service === "string" && service.length > 0);
  const affected = primaryFailures.length ? primaryFailures : services
    .filter((service) => service.State !== "running" || (service.Health && service.Health !== "healthy"))
    .map((service) => service.Service)
    .filter((service) => typeof service === "string" && service.length > 0);
  const selected = affected.length ? [...new Set(affected)].sort() : services.map((service) => service.Service).filter(Boolean).sort();
  const rawLogs = selected.length
    ? compose(config, ["logs", "--no-color", "--tail", "200", ...selected], runner)
    : "";
  const redactedLogs = redactFixtureText(config, rawLogs);
  const logs = redactedLogs.slice(-131_072);
  return {
    schema_version: "agentic-sandbox.celld-seaweedfs-startup-diagnostics/v1",
    run_id: config.run_id,
    fixture_profile: config.fixture_profile,
    affected_services: selected,
    services,
    logs,
    truncated: logs.length < redactedLogs.length,
  };
}

export function cleanupFixture(config, { removeRoot = true, runner = run } = {}) {
  let errors;
  try { errors = validateFixtureConfig(config); } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup config could not be validated: ${errorDigest(error)}`);
  }
  if (errors.length) throw new FixtureCleanupError(`fixture cleanup config is invalid: ${errorDigest(errors.join("; "))}`);
  const marker = join(config.run_root, ".agentic-celld-seaweedfs-run");
  if (!existsSync(config.run_root)) throw new FixtureCleanupError("fixture cleanup cannot prove the exact run directory exists");
  let rootMetadata;
  try { rootMetadata = lstatSync(config.run_root); } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup cannot inspect the exact run directory: ${errorDigest(error)}`);
  }
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink() || !existsSync(marker)) throw new FixtureCleanupError("fixture cleanup cannot prove the exact run directory and marker");
  let markerMetadata;
  try { markerMetadata = lstatSync(marker); } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup cannot inspect the ownership marker: ${errorDigest(error)}`);
  }
  if (!markerMetadata.isFile() || markerMetadata.isSymbolicLink() || (markerMetadata.mode & 0o077) !== 0) throw new FixtureCleanupError("fixture cleanup marker is not a protected regular file");
  let markerValue;
  try { markerValue = JSON.parse(readFileSync(marker, "utf8")); } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup marker is unreadable: ${errorDigest(error)}`);
  }
  if (markerValue.run_id !== config.run_id || markerValue.project !== config.project || markerValue.bucket !== config.bucket || markerValue.fixture_profile !== config.fixture_profile) throw new FixtureCleanupError("fixture cleanup marker does not match the exact fixture");
  try {
    compose(config, ["down", "--volumes", "--remove-orphans", "--timeout", "30"], runner, 120_000);
  } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup command failed: ${errorDigest(error)}`);
  }
  const label = `label=com.docker.compose.project=${config.project}`;
  const residue = [];
  try {
    for (const [kind, args] of [
      ["container", ["ps", "--all", "--filter", label, "--format", "{{.Names}}"]],
      ["network", ["network", "ls", "--filter", label, "--format", "{{.Name}}"]],
      ["volume", ["volume", "ls", "--filter", label, "--format", "{{.Name}}"]],
    ]) {
      const names = runner("docker", args, { timeout: 30_000 }).split(/\r?\n/).filter(Boolean);
      residue.push(...names.map((name) => `${kind}:${name}`));
    }
  } catch (error) {
    throw new FixtureCleanupError(`fixture cleanup could not prove absence: ${errorDigest(error)}`);
  }
  if (residue.length) throw new FixtureCleanupError(`fixture cleanup residue count ${residue.length}: ${errorDigest([...residue].sort().join("\n"))}`);
  if (removeRoot) {
    try { rmSync(config.run_root, { recursive: true, force: false }); } catch (error) {
      throw new FixtureCleanupError(`fixture run-root cleanup failed: ${errorDigest(error)}`);
    }
    if (existsSync(config.run_root)) throw new FixtureCleanupError("fixture run-root cleanup left residue");
  }
  return { status: "PASS", run_id: config.run_id, project: config.project, compose_residue: [], run_root_removed: removeRoot };
}

function argument(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

function main(args) {
  const command = args[0];
  if (command === "prepare") {
    const root = resolve(argument(args, "--root") ?? "");
    const output = resolve(argument(args, "--output") ?? join(root, "fixture.json"));
    if (output !== join(root, "fixture.json")) throw new Error("fixture config output must remain at the fixed run-root path");
    const config = prepareFixture({ fixtureProfile: argument(args, "--profile"), runId: argument(args, "--run-id"), root });
    console.log(JSON.stringify({ status: "PASS", config_path: resolve(output), fixture_profile: config.fixture_profile, promoting: config.promoting }));
    return 0;
  }
  if (command === "validate") {
    const path = resolve(argument(args, "--config") ?? "");
    const errors = validateFixtureConfig(JSON.parse(readFileSync(path, "utf8")));
    console.log(JSON.stringify({ status: errors.length ? "ERROR" : "PASS", errors }));
    return errors.length ? 3 : 0;
  }
  if (command === "start") {
    const path = resolve(argument(args, "--config") ?? "");
    const result = startFixture(JSON.parse(readFileSync(path, "utf8")));
    console.log(JSON.stringify(result));
    return 0;
  }
  if (command === "diagnose") {
    const path = resolve(argument(args, "--config") ?? "");
    const outputValue = argument(args, "--output");
    if (!outputValue) throw new Error("diagnostic output path is required");
    const output = resolve(outputValue);
    const result = collectFixtureDiagnostics(JSON.parse(readFileSync(path, "utf8")));
    writeFileSync(output, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600, flag: "wx" });
    console.log(JSON.stringify({ status: "PASS", output, affected_services: result.affected_services }));
    return 0;
  }
  if (command === "cleanup") {
    const path = resolve(argument(args, "--config") ?? "");
    console.log(JSON.stringify(cleanupFixture(JSON.parse(readFileSync(path, "utf8")))));
    return 0;
  }
  throw new Error("usage: celld-seaweedfs-fixture.mjs <prepare|validate|start|diagnose|cleanup> [options]");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  try { process.exitCode = main(process.argv.slice(2)); }
  catch (error) {
    const cleanupFailure = error instanceof FixtureCleanupError;
    console.error(JSON.stringify({
      status: "ERROR",
      reason_code: cleanupFailure ? "CELLD_SEAWEEDFS_CLEANUP_RESIDUE" : "CELLD_SEAWEEDFS_FIXTURE_ERROR",
      error_sha256: errorDigest(error),
    }));
    process.exitCode = error.exitCode ?? 3;
  }
}
