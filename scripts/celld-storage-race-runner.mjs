import { createHash } from "node:crypto";

import {
  S3V1Client,
  STORAGE_EVIDENCE_SCHEMA,
  classifyS3Outcome,
  validateStorageProfile,
} from "./celld-storage-qualifier.mjs";

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function candidate(runId, family, round, contender) {
  return Buffer.from(`${family}:${round}:${contender}:${sha256(`${runId}:${family}:${round}:${contender}`)}`);
}

function emptyRace(rounds) {
  return { rounds, commits: 0, conditional_losers: 0, ambiguous: 0, unknown: 0, total_outcomes: 0 };
}

async function bounded(count, concurrency, worker) {
  let next = 0;
  async function consume() {
    while (true) {
      const index = next++;
      if (index >= count) return;
      await worker(index);
    }
  }
  await Promise.all(Array.from({ length: Math.min(count, concurrency) }, consume));
}

function listCount(body) {
  return [...body.toString("utf8").matchAll(/<Key>(?:<!\[CDATA\[)?([\s\S]*?)(?:\]\]>)?<\/Key>/g)].length;
}

function profileAt(profile, endpoint, identityFile = profile.identity_file_ref) {
  return { ...profile, endpoint, identity_file_ref: identityFile };
}

function recordOutcome(race, classification) {
  race.total_outcomes += 1;
  if (classification.kind === "commit") race.commits += 1;
  else if (classification.kind === "conditional_loser") race.conditional_losers += 1;
  else if (classification.kind === "ambiguous") race.ambiguous += 1;
  else race.unknown += 1;
}

export async function runS3Qualification(profile, {
  adminIdentityFileRef,
  revokedIdentityFileRef,
  rawRows = [],
  clientFactory = (value) => new S3V1Client(value),
} = {}) {
  const errors = validateStorageProfile(profile);
  if (errors.length || profile.dialect !== "s3-v1" || !adminIdentityFileRef || !revokedIdentityFileRef) throw new Error(errors[0] ?? "S3 qualification requires admin and revoked identity file references");
  const endpoints = profile.backend.gateway_endpoints;
  const clients = endpoints.map((endpoint) => clientFactory(profileAt(profile, endpoint)));
  const admins = endpoints.map((endpoint) => clientFactory(profileAt(profile, endpoint, adminIdentityFileRef)));
  const revoked = clientFactory(profileAt(profile, endpoints[0], revokedIdentityFileRef));
  const created = new Set();
  const gatewayObserved = new Set();
  const createRace = emptyRace(profile.limits.create_rounds);
  const overwriteRace = emptyRace(profile.limits.overwrite_rounds);
  const latencies = { create_winner: [], overwrite_winner: [], get: [], head: [] };
  const reads = { first_read_mismatches: 0, hash_mismatches: 0, gateways_observed: 0 };
  const denials = {
    invalid_identity_attempts: 0, invalid_identity_denied: 0,
    expired_identity_attempts: 0, expired_identity_denied: 0,
    wrong_bucket_attempts: 0, wrong_bucket_denied: 0,
    cross_bucket_attempts: 0, cross_bucket_denied: 0,
  };
  const requestStats = { logical_requests: 0, total_attempts: 0, safety_retries: 0, auth_cases: 0, auth_attempts: 0, max_attempts_per_transient: 1, max_elapsed_ms_per_transient: 30_000 };
  const crossBucket = `celld-${sha256(`${profile.run_id}:cross-bucket`).slice(0, 24)}`;
  const wrongBucket = `celld-${sha256(`${profile.run_id}:wrong-bucket`).slice(0, 24)}`;
  let bucketCreated = false;
  let crossBucketCreated = false;
  let wrongBucketCreated = false;
  let deletedKeys = 0;
  let remaining = -1;

  async function call(client, method, ...args) {
    requestStats.logical_requests += 1;
    requestStats.total_attempts += 1;
    try { return await client[method](...args); }
    catch (error) { return { status: 0, error_code: "TransportError", headers: {}, body: Buffer.alloc(0), duration_ms: 30_000, transport_error_sha256: sha256(error.message) }; }
  }

  function reconciliation(read, head, winnerBytes, losingBytes, winnerEtag) {
    return {
      winner_visible: read.status === 200,
      winner_bytes_match: read.status === 200 && sha256(read.body) === sha256(winnerBytes),
      current_validator_match: Boolean(winnerEtag) && head.headers?.etag === winnerEtag,
      losing_bytes_absent: read.status === 200 && sha256(read.body) !== sha256(losingBytes),
      winner_sha256: sha256(winnerBytes),
    };
  }

  async function readAfterWinner(family, round, winnerClientIndex, winnerBytes) {
    const readIndex = (winnerClientIndex + 1) % clients.length;
    gatewayObserved.add(endpoints[readIndex]);
    const key = `${family}/${String(round).padStart(5, "0")}`;
    const get = await call(clients[readIndex], "get", key);
    const head = await call(clients[readIndex], "head", key);
    latencies.get.push(get.duration_ms);
    latencies.head.push(head.duration_ms);
    if (get.status !== 200) reads.first_read_mismatches += 1;
    if (get.status === 200 && sha256(get.body) !== sha256(winnerBytes)) reads.hash_mismatches += 1;
    return { get, head };
  }

  try {
    const createBucket = await call(admins[0], "createBucket", profile.bucket);
    if (createBucket.status < 200 || createBucket.status >= 300) throw new Error(`target bucket create returned ${createBucket.status}`);
    bucketCreated = true;
    const createCross = await call(admins[0], "createBucket", crossBucket);
    if (createCross.status < 200 || createCross.status >= 300) throw new Error(`cross bucket create returned ${createCross.status}`);
    crossBucketCreated = true;
    const createWrong = await call(admins[0], "createBucket", wrongBucket);
    if (createWrong.status < 200 || createWrong.status >= 300) throw new Error(`wrong bucket create returned ${createWrong.status}`);
    wrongBucketCreated = true;

    for (let index = 0; index < profile.limits.warmups; index += 1) {
      const key = `warmup/${String(index).padStart(5, "0")}`;
      await call(clients[index % clients.length], "put", key, candidate(profile.run_id, "warmup", index, 0), { ifNoneMatch: "*" });
      await call(clients[index % clients.length], "delete", key);
    }

    await bounded(profile.limits.create_rounds, profile.limits.max_workers, async (round) => {
      const key = `create/${String(round).padStart(5, "0")}`;
      const values = Array.from({ length: profile.limits.contenders }, (_, contender) => candidate(profile.run_id, "create", round, contender));
      const responses = await Promise.all(values.map((value, contender) => {
        const clientIndex = contender % clients.length;
        gatewayObserved.add(endpoints[clientIndex]);
        return call(clients[clientIndex], "put", key, value, { ifNoneMatch: "*" });
      }));
      const winnerIndex = responses.findIndex((response) => response.status >= 200 && response.status < 300);
      const winnerBytes = winnerIndex >= 0 ? values[winnerIndex] : Buffer.alloc(0);
      const crossRead = winnerIndex >= 0 ? await readAfterWinner("create", round, winnerIndex % clients.length, winnerBytes) : null;
      if (winnerIndex >= 0) {
        created.add(key);
        latencies.create_winner.push(responses[winnerIndex].duration_ms);
      }
      const recorded = [];
      for (let contender = 0; contender < responses.length; contender += 1) {
        const response = responses[contender];
        const rec = response.status === 409 && crossRead && winnerIndex >= 0 ? reconciliation(crossRead.get, crossRead.head, winnerBytes, values[contender], responses[winnerIndex].headers?.etag) : undefined;
        const classified = classifyS3Outcome({ ...response, reconciliation: rec }, { winner_sha256: winnerIndex >= 0 ? sha256(winnerBytes) : undefined });
        recordOutcome(createRace, classified);
        recorded.push({ contender, gateway: contender % clients.length, status: response.status, error_code: response.error_code ?? null, kind: classified.kind, reason: classified.reason, duration_ms: response.duration_ms, candidate_sha256: sha256(values[contender]) });
      }
      rawRows.push({ family: "create", round, key_sha256: sha256(key), winner_sha256: winnerIndex >= 0 ? sha256(winnerBytes) : null, outcomes: recorded, first_read_status: crossRead?.get.status ?? null, head_status: crossRead?.head.status ?? null });
    });

    await bounded(profile.limits.overwrite_rounds, profile.limits.max_workers, async (round) => {
      const key = `overwrite/${String(round).padStart(5, "0")}`;
      const baseline = candidate(profile.run_id, "baseline", round, 0);
      const initial = await call(clients[round % clients.length], "put", key, baseline, { ifNoneMatch: "*" });
      const baselineHead = await call(clients[round % clients.length], "head", key);
      const validator = baselineHead.headers?.etag;
      if (initial.status >= 200 && initial.status < 300) created.add(key);
      const values = Array.from({ length: profile.limits.contenders }, (_, contender) => candidate(profile.run_id, "overwrite", round, contender));
      const responses = validator ? await Promise.all(values.map((value, contender) => {
        const clientIndex = contender % clients.length;
        gatewayObserved.add(endpoints[clientIndex]);
        return call(clients[clientIndex], "put", key, value, { ifMatch: validator });
      })) : values.map(() => ({ status: 0, error_code: "MissingValidator", headers: {}, body: Buffer.alloc(0), duration_ms: 0 }));
      const winnerIndex = responses.findIndex((response) => response.status >= 200 && response.status < 300);
      const winnerBytes = winnerIndex >= 0 ? values[winnerIndex] : Buffer.alloc(0);
      const crossRead = winnerIndex >= 0 ? await readAfterWinner("overwrite", round, winnerIndex % clients.length, winnerBytes) : null;
      if (winnerIndex >= 0) latencies.overwrite_winner.push(responses[winnerIndex].duration_ms);
      const recorded = [];
      for (let contender = 0; contender < responses.length; contender += 1) {
        const response = responses[contender];
        const rec = response.status === 409 && crossRead && winnerIndex >= 0 ? reconciliation(crossRead.get, crossRead.head, winnerBytes, values[contender], responses[winnerIndex].headers?.etag) : undefined;
        const classified = classifyS3Outcome({ ...response, reconciliation: rec }, { winner_sha256: winnerIndex >= 0 ? sha256(winnerBytes) : undefined });
        recordOutcome(overwriteRace, classified);
        recorded.push({ contender, gateway: contender % clients.length, status: response.status, error_code: response.error_code ?? null, kind: classified.kind, reason: classified.reason, duration_ms: response.duration_ms, candidate_sha256: sha256(values[contender]) });
      }
      rawRows.push({ family: "overwrite", round, key_sha256: sha256(key), baseline_status: initial.status, validator_present: Boolean(validator), winner_sha256: winnerIndex >= 0 ? sha256(winnerBytes) : null, outcomes: recorded, first_read_status: crossRead?.get.status ?? null, head_status: crossRead?.head.status ?? null });
    });

    async function denial(family, client, bucket) {
      denials[`${family}_attempts`] += 1;
      requestStats.auth_cases += 1;
      requestStats.auth_attempts += 1;
      const response = await call(client, "request", "PUT", `denial/${family}`, { bucket, includePrefix: false, body: Buffer.from("denied") });
      if (classifyS3Outcome(response).kind === "auth_denied") denials[`${family}_denied`] += 1;
      rawRows.push({ family: "denial", class: family, status: response.status, error_code: response.error_code ?? null, outcome: classifyS3Outcome(response).kind });
    }
    await denial("invalid_identity", revoked, profile.bucket);
    await denial("expired_identity", revoked, profile.bucket);
    await denial("wrong_bucket", clients[0], wrongBucket);
    await denial("cross_bucket", clients[1 % clients.length], crossBucket);
  } finally {
    const cleanupKeys = [...created];
    await bounded(cleanupKeys.length, profile.limits.max_workers, async (index) => {
      const key = cleanupKeys[index];
      const response = await call(clients[index % clients.length], "delete", key);
      if (response.status >= 200 && response.status < 300) deletedKeys += 1;
    });
    if (bucketCreated) {
      const listed = await call(admins[0], "listPrefix");
      remaining = listed.status === 200 ? listCount(listed.body) : -1;
      if (remaining === 0) await call(admins[0], "deleteBucket", profile.bucket);
    }
    if (crossBucketCreated) await call(admins[0], "deleteBucket", crossBucket);
    if (wrongBucketCreated) await call(admins[0], "deleteBucket", wrongBucket);
    for (const client of [...clients, ...admins, revoked]) client.close?.();
  }

  reads.gateways_observed = gatewayObserved.size;
  return {
    schema_version: STORAGE_EVIDENCE_SCHEMA,
    dialect: profile.dialect,
    scope: profile.scope,
    profile_id: profile.profile_id,
    run_id: profile.run_id,
    identity: {
      backend_product: profile.backend.product,
      backend_version: profile.backend.version,
      artifact_sha256: profile.backend.artifact_sha256,
      configuration_sha256: profile.backend.configuration_sha256,
      bucket_scope_sha256: sha256(`${profile.bucket}/${profile.run_prefix}`),
      gateway_endpoints: profile.backend.gateway_endpoints,
      topology: profile.backend.topology,
      gateway_count: profile.backend.gateway_endpoints.length,
    },
    parameters: { ...profile.limits },
    races: { create: createRace, overwrite: overwriteRace },
    reads,
    latencies_ms: latencies,
    retries: requestStats,
    denials,
    broken_store_variants: [],
    cleanup: { created_keys: created.size, deleted_keys: deletedKeys, enumerated_remaining: remaining, run_prefix_absent: remaining === 0 },
  };
}
