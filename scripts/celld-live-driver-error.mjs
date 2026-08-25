import { createHash } from "node:crypto";

const SAFE_DRIVER_FIELD = /^[A-Za-z0-9_.:/-]{1,160}$/;

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function safeDriverField(value) {
  if (value === undefined || value === null) return null;
  const text = String(value);
  return SAFE_DRIVER_FIELD.test(text) ? text : `sha256:${sha256(text)}`;
}

export function annotateDriverError(error, fields = {}) {
  const target = error instanceof Error ? error : new Error(String(error));
  for (const [key, value] of Object.entries(fields)) {
    if (value === undefined || value === null) continue;
    if (target[key] === undefined || target[key] === null) target[key] = value;
  }
  return target;
}

export async function withDriverOperation(operation, fields, callback) {
  try {
    return await callback();
  } catch (error) {
    throw annotateDriverError(error, { operation, ...fields });
  }
}

export function driverOperationError(operation, fields, message) {
  return annotateDriverError(new Error(message), { operation, ...fields });
}

export function driverErrorDocument(error) {
  const exitCode = [3, 4].includes(error?.exitCode) ? error.exitCode : 3;
  const document = {
    schema_version: "agentic-sandbox.celld-live-driver-error/v1",
    name: safeDriverField(error?.name ?? "Error"),
    message_sha256: sha256(String(error?.message ?? "")),
    exit_code: exitCode,
  };
  for (const [outputKey, inputKey] of [
    ["operation", "operation"],
    ["scenario_id", "scenarioId"],
    ["error_code", "errorCode"],
    ["node_code", "code"],
    ["signal", "signal"],
    ["evidence_sha256", "evidenceSha256"],
    ["stdout_sha256", "stdoutSha256"],
    ["stderr_sha256", "stderrSha256"],
  ]) {
    const value = safeDriverField(error?.[inputKey]);
    if (value !== null) document[outputKey] = value;
  }
  if (Number.isInteger(error?.exitStatus)) document.exit_status = error.exitStatus;
  if (error?.timedOut === true) document.timed_out = true;
  return document;
}

export function emitDriverError(prefix, error) {
  const document = driverErrorDocument(error);
  process.stderr.write(`${prefix} ${JSON.stringify(document)}\n`);
  process.exitCode = document.exit_code;
}
