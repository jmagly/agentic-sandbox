#!/usr/bin/env node

import { createHash, createHmac } from "node:crypto";
import { readFileSync, lstatSync } from "node:fs";
import http from "node:http";
import https from "node:https";
import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const STORAGE_PROFILE_SCHEMA = "agentic-sandbox.celld-storage-profile/v1";
export const STORAGE_EVIDENCE_SCHEMA = "agentic-sandbox.celld-storage-evidence/v1";
export const S3_DIALECT = "s3-v1";
export const GCS_RESERVED_DIALECT = "gcs-reserved";
const SHA256 = /^[0-9a-f]{64}$/;
const SAFE_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const FORBIDDEN_VERDICT_KEYS = new Set(["qualified", "passed", "pass", "status", "verdict"]);
const SECRET_LIKE = /(?:authorization|api[-_]?key|password|secret|token|credential)\s*[:=]\s*[^\s,;]+|bearer\s+[A-Za-z0-9._~+\/-]+/i;

function ownObject(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

function rejectUnknown(errors, value, allowed, context) {
  if (!ownObject(value)) {
    errors.push(`${context} must be an object`);
    return;
  }
  for (const key of Object.keys(value)) if (!allowed.has(key)) errors.push(`${context}.${key} is not allowed`);
}

function string(value) {
  return typeof value === "string" && value.trim() !== "";
}

function safeEndpoint(value) {
  try {
    const url = new URL(value);
    const loopback = ["localhost", "127.0.0.1", "::1", "[::1]"].includes(url.hostname);
    return url.username === "" && url.password === "" && (url.protocol === "https:" || (url.protocol === "http:" && loopback));
  } catch {
    return false;
  }
}

export function validateStorageProfile(profile) {
  const errors = [];
  rejectUnknown(errors, profile, new Set([
    "schema_version", "profile_id", "run_id", "dialect", "scope", "endpoint", "region",
    "addressing_mode", "bucket", "run_prefix", "identity_file_ref", "ca_file_ref", "backend", "limits",
  ]), "profile");
  if (profile?.schema_version !== STORAGE_PROFILE_SCHEMA) errors.push(`profile.schema_version must be ${STORAGE_PROFILE_SCHEMA}`);
  if (!SAFE_ID.test(profile?.profile_id ?? "")) errors.push("profile.profile_id is invalid");
  if (!SAFE_ID.test(profile?.run_id ?? "")) errors.push("profile.run_id is invalid");
  if (![S3_DIALECT, GCS_RESERVED_DIALECT].includes(profile?.dialect)) errors.push("profile.dialect is unsupported");
  if (!["fixture_reduced", "live_candidate"].includes(profile?.scope)) errors.push("profile.scope is unsupported");
  if (!safeEndpoint(profile?.endpoint)) errors.push("profile.endpoint must be HTTPS or loopback HTTP and must not contain userinfo");
  if (!string(profile?.region)) errors.push("profile.region is required");
  if (profile?.addressing_mode !== "path") errors.push("profile.addressing_mode must be path for storage profile v1");
  if (!/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(profile?.bucket ?? "") || profile?.bucket?.includes("..")) errors.push("profile.bucket is invalid");
  if (!string(profile?.run_prefix) || profile.run_prefix.startsWith("/") || profile.run_prefix.includes("..") || !profile.run_prefix.split("/").includes(profile.run_id)) errors.push("profile.run_prefix must be relative and contain run_id as an exact segment");
  if (!isAbsolute(profile?.identity_file_ref ?? "")) errors.push("profile.identity_file_ref must be an absolute protected-file reference");
  if (profile?.ca_file_ref !== undefined && !isAbsolute(profile.ca_file_ref)) errors.push("profile.ca_file_ref must be absolute when provided");
  rejectUnknown(errors, profile?.backend, new Set(["product", "version", "artifact_sha256", "gateway_endpoints", "topology"]), "profile.backend");
  if (!string(profile?.backend?.product) || !string(profile?.backend?.version) || !SHA256.test(profile?.backend?.artifact_sha256 ?? "")) errors.push("profile.backend identity is incomplete");
  if (!Array.isArray(profile?.backend?.gateway_endpoints) || profile.backend.gateway_endpoints.length === 0 || profile.backend.gateway_endpoints.some((endpoint) => !safeEndpoint(endpoint))) errors.push("profile.backend.gateway_endpoints are invalid");
  if (!string(profile?.backend?.topology)) errors.push("profile.backend.topology is required");
  rejectUnknown(errors, profile?.limits, new Set(["create_rounds", "overwrite_rounds", "contenders", "warmups", "max_workers", "max_connections"]), "profile.limits");
  for (const key of ["create_rounds", "overwrite_rounds", "contenders", "warmups", "max_workers", "max_connections"]) if (!Number.isSafeInteger(profile?.limits?.[key]) || profile.limits[key] < 0) errors.push(`profile.limits.${key} must be a nonnegative integer`);
  if ((profile?.limits?.contenders ?? 0) < 2) errors.push("profile.limits.contenders must be at least 2");
  if ((profile?.limits?.max_workers ?? 65) > 32) errors.push("profile.limits.max_workers must be at most 32");
  if ((profile?.limits?.max_connections ?? 65) > 64) errors.push("profile.limits.max_connections must be at most 64");
  if (profile?.scope === "live_candidate" && (profile.limits?.create_rounds !== 10_000 || profile.limits?.overwrite_rounds !== 10_000 || profile.limits?.warmups !== 100)) errors.push("live_candidate requires 10000 create rounds, 10000 overwrite rounds, and 100 warmups");
  if (SECRET_LIKE.test(JSON.stringify(profile))) errors.push("profile contains secret-like inline data; use protected file references only");
  return errors;
}

export function resolveStorageProfile({ celldEnabled, profilePath }) {
  if (!celldEnabled) return null;
  if (!isAbsolute(profilePath ?? "")) throw new Error("enabled Celld storage profile path must be absolute");
  const profile = JSON.parse(readFileSync(profilePath, "utf8"));
  const errors = validateStorageProfile(profile);
  if (errors.length > 0) throw new Error(errors.join("; "));
  return profile;
}

function parseAwsIdentity(path) {
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) throw new Error("identity file must be a regular non-symlink file with mode 0600 or stricter");
  const values = {};
  let section = "";
  for (const raw of readFileSync(path, "utf8").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#") || line.startsWith(";")) continue;
    if (line.startsWith("[") && line.endsWith("]")) { section = line.slice(1, -1); continue; }
    if (section !== "default" || !line.includes("=")) continue;
    const [key, ...rest] = line.split("=");
    values[key.trim()] = rest.join("=").trim();
  }
  if (!values.aws_access_key_id || !values.aws_secret_access_key) throw new Error("identity file default profile is incomplete");
  return { accessKeyId: values.aws_access_key_id, secretAccessKey: values.aws_secret_access_key, sessionToken: values.aws_session_token };
}

function hash(value) {
  return createHash("sha256").update(value).digest("hex");
}

function hmac(key, value, encoding) {
  return createHmac("sha256", key).update(value).digest(encoding);
}

function canonicalUri(pathname) {
  return pathname.split("/").map((segment) => encodeURIComponent(decodeURIComponent(segment)).replace(/[!'()*]/g, (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`)).join("/");
}

export function signS3Request({ method, url, headers = {}, body = Buffer.alloc(0), region, credentials, now = new Date() }) {
  const target = new URL(url);
  const amzDate = now.toISOString().replace(/[:-]|\.\d{3}/g, "");
  const shortDate = amzDate.slice(0, 8);
  const payloadHash = hash(body);
  const normalized = new Map(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), String(value).trim().replace(/\s+/g, " ")]));
  normalized.set("host", target.host);
  normalized.set("x-amz-content-sha256", payloadHash);
  normalized.set("x-amz-date", amzDate);
  if (credentials.sessionToken) normalized.set("x-amz-security-token", credentials.sessionToken);
  const names = [...normalized.keys()].sort();
  const canonicalHeaders = names.map((name) => `${name}:${normalized.get(name)}\n`).join("");
  const query = [...target.searchParams.entries()].sort(([ak, av], [bk, bv]) => ak.localeCompare(bk) || av.localeCompare(bv)).map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`).join("&");
  const canonicalRequest = [method.toUpperCase(), canonicalUri(target.pathname), query, canonicalHeaders, names.join(";"), payloadHash].join("\n");
  const scope = `${shortDate}/${region}/s3/aws4_request`;
  const stringToSign = `AWS4-HMAC-SHA256\n${amzDate}\n${scope}\n${hash(canonicalRequest)}`;
  const dateKey = hmac(`AWS4${credentials.secretAccessKey}`, shortDate);
  const regionKey = hmac(dateKey, region);
  const serviceKey = hmac(regionKey, "s3");
  const signingKey = hmac(serviceKey, "aws4_request");
  const signature = hmac(signingKey, stringToSign, "hex");
  return Object.fromEntries([
    ...normalized.entries(),
    ["authorization", `AWS4-HMAC-SHA256 Credential=${credentials.accessKeyId}/${scope}, SignedHeaders=${names.join(";")}, Signature=${signature}`],
  ]);
}

function defaultRequest({ method, url, headers, body, ca }) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    const transport = target.protocol === "https:" ? https : http;
    const request = transport.request(target, { method, headers, ca, timeout: 30_000 }, (response) => {
      const chunks = [];
      response.on("data", (chunk) => { if (chunks.reduce((sum, item) => sum + item.length, 0) < 1024 * 1024) chunks.push(chunk); });
      response.on("end", () => resolve({ status: response.statusCode, headers: response.headers, body: Buffer.concat(chunks) }));
    });
    request.on("timeout", () => request.destroy(new Error("request deadline exceeded")));
    request.on("error", reject);
    if (body.length > 0) request.write(body);
    request.end();
  });
}

export class S3V1Client {
  constructor(profile, { identityLoader = parseAwsIdentity, requestImpl = defaultRequest, now = () => new Date() } = {}) {
    const errors = validateStorageProfile(profile);
    if (errors.length > 0 || profile.dialect !== S3_DIALECT) throw new Error(errors[0] ?? "S3V1Client requires the s3-v1 dialect");
    this.profile = profile;
    this.identityLoader = identityLoader;
    this.requestImpl = requestImpl;
    this.now = now;
  }

  async request(method, key, { body = Buffer.alloc(0), ifNoneMatch, ifMatch } = {}) {
    const bytes = Buffer.isBuffer(body) ? body : Buffer.from(body);
    const base = this.profile.endpoint.replace(/\/$/, "");
    const path = [this.profile.bucket, this.profile.run_prefix, key].flatMap((value) => value.split("/")).map(encodeURIComponent).join("/");
    const url = `${base}/${path}`;
    const headers = { "content-length": String(bytes.length) };
    if (ifNoneMatch !== undefined) headers["if-none-match"] = ifNoneMatch;
    if (ifMatch !== undefined) headers["if-match"] = ifMatch;
    const credentials = this.identityLoader(this.profile.identity_file_ref);
    const signed = signS3Request({ method, url, headers, body: bytes, region: this.profile.region, credentials, now: this.now() });
    const ca = this.profile.ca_file_ref ? readFileSync(this.profile.ca_file_ref) : undefined;
    const response = await this.requestImpl({ method, url, headers: signed, body: bytes, ca });
    const errorCode = response.status >= 300 ? /<Code>([^<]+)<\/Code>/.exec(response.body.toString("utf8", 0, 8192))?.[1] : undefined;
    return { status: response.status, headers: response.headers, body: response.body, error_code: errorCode };
  }

  put(key, body, conditions = {}) { return this.request("PUT", key, { body, ...conditions }); }
  get(key) { return this.request("GET", key); }
  head(key) { return this.request("HEAD", key); }
  delete(key) { return this.request("DELETE", key); }
}

export function classifyS3Outcome({ status, error_code: errorCode, reconciliation }, expected = {}) {
  if (Number.isInteger(status) && status >= 200 && status < 300) return { kind: "commit", reason: "S3_2XX_COMMIT" };
  if (status === 412 && errorCode === "PreconditionFailed") return { kind: "conditional_loser", reason: "S3_412_PRECONDITION_FAILED" };
  if (status === 409 && errorCode === "ConditionalRequestConflict") {
    const reconciled = ownObject(reconciliation)
      && reconciliation.winner_visible === true
      && reconciliation.winner_bytes_match === true
      && reconciliation.current_validator_match === true
      && reconciliation.losing_bytes_absent === true
      && (!expected.winner_sha256 || reconciliation.winner_sha256 === expected.winner_sha256);
    return reconciled ? { kind: "conditional_loser", reason: "S3_409_RECONCILED_LOSER" } : { kind: "ambiguous", reason: "S3_409_AMBIGUOUS" };
  }
  if ([401, 403].includes(status) || ["AccessDenied", "InvalidAccessKeyId", "ExpiredToken", "SignatureDoesNotMatch"].includes(errorCode)) return { kind: "auth_denied", reason: "S3_AUTH_DENIED" };
  return { kind: "unknown", reason: "S3_UNKNOWN_OUTCOME" };
}

export function nearestRank(values, percentile) {
  if (!Array.isArray(values) || values.length === 0 || values.some((value) => !Number.isFinite(value) || value < 0)) throw new Error("latency samples must be nonempty nonnegative numbers");
  const ordered = [...values].sort((a, b) => a - b);
  return ordered[Math.max(0, Math.ceil(percentile * ordered.length) - 1)];
}

export function validateStorageEvidence(evidence) {
  const errors = [];
  rejectUnknown(errors, evidence, new Set(["schema_version", "dialect", "scope", "profile_id", "run_id", "identity", "parameters", "races", "reads", "latencies_ms", "retries", "denials", "broken_store_variants", "cleanup"]), "evidence");
  if (evidence?.schema_version !== STORAGE_EVIDENCE_SCHEMA) errors.push(`evidence.schema_version must be ${STORAGE_EVIDENCE_SCHEMA}`);
  if (![S3_DIALECT, GCS_RESERVED_DIALECT].includes(evidence?.dialect)) errors.push("evidence.dialect is unsupported");
  if (!["fixture_reduced", "live_candidate"].includes(evidence?.scope)) errors.push("evidence.scope is unsupported");
  if (!SAFE_ID.test(evidence?.profile_id ?? "") || !SAFE_ID.test(evidence?.run_id ?? "")) errors.push("evidence profile/run identity is invalid");
  const walk = (value, path = "evidence") => {
    if (!ownObject(value)) return;
    for (const [key, child] of Object.entries(value)) {
      if (FORBIDDEN_VERDICT_KEYS.has(key)) errors.push(`${path}.${key} is a forbidden self-declared verdict`);
      if (ownObject(child)) walk(child, `${path}.${key}`);
    }
  };
  walk(evidence);
  if (SECRET_LIKE.test(JSON.stringify(evidence))) errors.push("evidence contains secret-like data");
  for (const key of ["identity", "parameters", "races", "reads", "latencies_ms", "retries", "denials", "cleanup"]) if (!ownObject(evidence?.[key])) errors.push(`evidence.${key} must be an object`);
  rejectUnknown(errors, evidence?.identity, new Set(["backend_product", "backend_version", "artifact_sha256", "gateway_endpoints", "topology", "gateway_count"]), "evidence.identity");
  if (!string(evidence?.identity?.backend_product) || !string(evidence?.identity?.backend_version) || !SHA256.test(evidence?.identity?.artifact_sha256 ?? "")) errors.push("evidence.identity backend identity is incomplete");
  if (!Array.isArray(evidence?.identity?.gateway_endpoints) || evidence.identity.gateway_endpoints.length === 0 || evidence.identity.gateway_endpoints.some((value) => !safeEndpoint(value)) || evidence.identity.gateway_count !== evidence.identity.gateway_endpoints.length) errors.push("evidence.identity gateway identity is invalid");
  if (!string(evidence?.identity?.topology)) errors.push("evidence.identity.topology is required");
  rejectUnknown(errors, evidence?.parameters, new Set(["create_rounds", "overwrite_rounds", "contenders", "warmups", "max_workers", "max_connections"]), "evidence.parameters");
  for (const key of ["create_rounds", "overwrite_rounds", "contenders", "warmups", "max_workers", "max_connections"]) if (!Number.isSafeInteger(evidence?.parameters?.[key]) || evidence.parameters[key] < 0) errors.push(`evidence.parameters.${key} is invalid`);
  rejectUnknown(errors, evidence?.races, new Set(["create", "overwrite"]), "evidence.races");
  for (const family of ["create", "overwrite"]) {
    const race = evidence?.races?.[family];
    rejectUnknown(errors, race, new Set(["rounds", "commits", "conditional_losers", "ambiguous", "unknown", "total_outcomes"]), `evidence.races.${family}`);
    for (const key of ["rounds", "commits", "conditional_losers", "ambiguous", "unknown", "total_outcomes"]) if (!Number.isSafeInteger(race?.[key]) || race[key] < 0) errors.push(`evidence.races.${family}.${key} is invalid`);
  }
  rejectUnknown(errors, evidence?.reads, new Set(["first_read_mismatches", "hash_mismatches", "gateways_observed"]), "evidence.reads");
  for (const key of ["first_read_mismatches", "hash_mismatches", "gateways_observed"]) if (!Number.isSafeInteger(evidence?.reads?.[key]) || evidence.reads[key] < 0) errors.push(`evidence.reads.${key} is invalid`);
  rejectUnknown(errors, evidence?.latencies_ms, new Set(["create_winner", "overwrite_winner", "get", "head"]), "evidence.latencies_ms");
  for (const family of ["create_winner", "overwrite_winner", "get", "head"]) if (!Array.isArray(evidence?.latencies_ms?.[family]) || evidence.latencies_ms[family].some((value) => !Number.isFinite(value) || value < 0)) errors.push(`evidence.latencies_ms.${family} is invalid`);
  rejectUnknown(errors, evidence?.retries, new Set(["logical_requests", "total_attempts", "safety_retries", "auth_cases", "auth_attempts", "max_attempts_per_transient", "max_elapsed_ms_per_transient"]), "evidence.retries");
  for (const key of ["logical_requests", "total_attempts", "safety_retries", "auth_cases", "auth_attempts", "max_attempts_per_transient", "max_elapsed_ms_per_transient"]) if (!Number.isSafeInteger(evidence?.retries?.[key]) || evidence.retries[key] < 0) errors.push(`evidence.retries.${key} is invalid`);
  rejectUnknown(errors, evidence?.denials, new Set(["invalid_identity_attempts", "invalid_identity_denied", "expired_identity_attempts", "expired_identity_denied", "wrong_bucket_attempts", "wrong_bucket_denied", "cross_bucket_attempts", "cross_bucket_denied"]), "evidence.denials");
  for (const key of ["invalid_identity_attempts", "invalid_identity_denied", "expired_identity_attempts", "expired_identity_denied", "wrong_bucket_attempts", "wrong_bucket_denied", "cross_bucket_attempts", "cross_bucket_denied"]) if (!Number.isSafeInteger(evidence?.denials?.[key]) || evidence.denials[key] < 0) errors.push(`evidence.denials.${key} is invalid`);
  if (!Array.isArray(evidence?.broken_store_variants)) errors.push("evidence.broken_store_variants must be an array");
  else for (const [index, variant] of evidence.broken_store_variants.entries()) {
    rejectUnknown(errors, variant, new Set(["id", "measurements"]), `evidence.broken_store_variants[${index}]`);
    rejectUnknown(errors, variant?.measurements, new Set(["unexpected_commits", "split_commits", "first_read_mismatches", "ambiguous", "unknown"]), `evidence.broken_store_variants[${index}].measurements`);
    if (!string(variant?.id)) errors.push(`evidence.broken_store_variants[${index}].id is invalid`);
    for (const key of ["unexpected_commits", "split_commits", "first_read_mismatches", "ambiguous", "unknown"]) if (!Number.isSafeInteger(variant?.measurements?.[key]) || variant.measurements[key] < 0) errors.push(`evidence.broken_store_variants[${index}].measurements.${key} is invalid`);
  }
  rejectUnknown(errors, evidence?.cleanup, new Set(["created_keys", "deleted_keys", "enumerated_remaining", "run_prefix_absent"]), "evidence.cleanup");
  for (const key of ["created_keys", "deleted_keys", "enumerated_remaining"]) if (!Number.isSafeInteger(evidence?.cleanup?.[key]) || evidence.cleanup[key] < 0) errors.push(`evidence.cleanup.${key} is invalid`);
  if (typeof evidence?.cleanup?.run_prefix_absent !== "boolean") errors.push("evidence.cleanup.run_prefix_absent is invalid");
  return errors;
}

function racePass(race, expectedRounds, contenders) {
  return ownObject(race)
    && race.rounds === expectedRounds
    && race.commits === expectedRounds
    && race.conditional_losers === expectedRounds * (contenders - 1)
    && race.ambiguous === 0
    && race.unknown === 0
    && race.total_outcomes === expectedRounds * contenders;
}

export function evaluateStorageEvidence(evidence) {
  const errors = validateStorageEvidence(evidence);
  if (errors.length > 0) return { status: "ERROR", reason_code: "CELLD_STORAGE_EVIDENCE_INVALID", errors, live_qualification: false, checks: [] };
  if (evidence.dialect === GCS_RESERVED_DIALECT) return { status: "NOT_RUN", reason_code: "CELLD_STORAGE_GCS_RESERVED", live_qualification: false, checks: [] };
  const p = evidence.parameters;
  const checks = [];
  const check = (id, passed, observed) => checks.push({ id, passed, observed });
  check("s3.parameters", evidence.scope !== "live_candidate" || (p.create_rounds === 10_000 && p.overwrite_rounds === 10_000 && p.warmups === 100 && p.contenders >= 2 && p.max_workers <= 32 && p.max_connections <= 64), p);
  check("s3.create_races", racePass(evidence.races.create, p.create_rounds, p.contenders), evidence.races.create);
  check("s3.overwrite_races", racePass(evidence.races.overwrite, p.overwrite_rounds, p.contenders), evidence.races.overwrite);
  check("s3.cross_gateway_reads", evidence.identity.gateway_count >= 2 && evidence.reads.first_read_mismatches === 0 && evidence.reads.hash_mismatches === 0 && evidence.reads.gateways_observed === evidence.identity.gateway_count, evidence.reads);
  for (const family of ["create_winner", "overwrite_winner", "get", "head"]) {
    let observed;
    let passed = false;
    try {
      const values = evidence.latencies_ms[family];
      const p99 = nearestRank(values, 0.99);
      observed = { samples: values.length, p99 };
      passed = values.length >= (evidence.scope === "live_candidate" ? 10_000 : 1) && p99 <= 250;
    } catch (error) { observed = { error: error.message }; }
    check(`s3.latency.${family}`, passed, observed);
  }
  const amplification = evidence.retries.logical_requests === 0 ? Infinity : evidence.retries.total_attempts / evidence.retries.logical_requests;
  check("s3.retry_policy", evidence.retries.safety_retries === 0 && evidence.retries.auth_attempts === evidence.retries.auth_cases && evidence.retries.max_attempts_per_transient <= 3 && evidence.retries.max_elapsed_ms_per_transient <= 30_000 && amplification <= 3, { ...evidence.retries, amplification });
  const denialFamilies = ["invalid_identity", "expired_identity", "wrong_bucket", "cross_bucket"];
  check("s3.denials", denialFamilies.every((family) => evidence.denials[`${family}_attempts`] > 0 && evidence.denials[`${family}_denied`] === evidence.denials[`${family}_attempts`]), evidence.denials);
  const requiredBroken = new Set(["ignored-if-none-match", "ignored-or-stale-if-match", "gateway-local-locking", "stale-first-read", "misleading-outcome"]);
  const brokenDetected = (item) => {
    if (!requiredBroken.delete(item.id)) return false;
    if (["ignored-if-none-match", "ignored-or-stale-if-match"].includes(item.id)) return item.measurements.unexpected_commits > 0;
    if (item.id === "gateway-local-locking") return item.measurements.split_commits > 0;
    if (item.id === "stale-first-read") return item.measurements.first_read_mismatches > 0;
    return item.measurements.ambiguous + item.measurements.unknown > 0;
  };
  check("s3.broken_store_detection", evidence.broken_store_variants.length === requiredBroken.size && evidence.broken_store_variants.every(brokenDetected) && requiredBroken.size === 0, evidence.broken_store_variants);
  check("s3.cleanup", evidence.cleanup.enumerated_remaining === 0 && evidence.cleanup.deleted_keys === evidence.cleanup.created_keys && evidence.cleanup.run_prefix_absent === true, evidence.cleanup);
  const failed = checks.some((item) => !item.passed);
  if (failed) return { status: "FAIL", reason_code: "CELLD_STORAGE_GATE_FAILED", live_qualification: false, checks };
  if (evidence.scope === "fixture_reduced") return { status: "NOT_RUN", reason_code: "CELLD_STORAGE_FIXTURE_NON_PROMOTING", live_qualification: false, checks };
  return { status: "PASS", reason_code: "CELLD_STORAGE_S3_V1_QUALIFIED", live_qualification: true, checks };
}

function inputPath(args) {
  const index = args.indexOf("--input");
  if (index < 0 || !args[index + 1]) throw new Error("--input FILE is required");
  return args[index + 1];
}

function main(args) {
  const command = args[0];
  const input = JSON.parse(readFileSync(inputPath(args), "utf8"));
  if (command === "profile-check") {
    const errors = validateStorageProfile(input);
    console.log(JSON.stringify({ status: errors.length === 0 ? "PASS" : "ERROR", errors }));
    return errors.length === 0 ? 0 : 3;
  }
  if (command === "evaluate") {
    const result = evaluateStorageEvidence(input);
    console.log(JSON.stringify(result));
    return result.status === "PASS" ? 0 : result.status === "FAIL" ? 1 : result.status === "NOT_RUN" ? 2 : 3;
  }
  throw new Error("usage: celld-storage-qualifier.mjs <profile-check|evaluate> --input FILE");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  try { process.exitCode = main(process.argv.slice(2)); }
  catch (error) { console.error(JSON.stringify({ status: "ERROR", reason_code: "CELLD_STORAGE_QUALIFIER_ERROR", error: error.message })); process.exitCode = 3; }
}
