import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");

test("Celld service calls the packageable read-only preflight helper", () => {
  const unit = read("deploy/celld/celld.service");
  const packaging = read("scripts/package-linux.sh");
  const helper = read("management/src/bin/agentic-celld-preflight.rs");

  assert.match(unit, /ExecStartPre=\/usr\/libexec\/agentic-sandbox\/agentic-celld-preflight/);
  assert.match(unit, /ExecStart=\/opt\/agentic-sandbox\/celld\/v0\.2\.1\/celld$/m);
  assert.match(packaging, /release\/agentic-celld-preflight/);
  assert.match(packaging, /agentic-celld\.service/);
  assert.match(helper, /scope: "local_prestart"/);
  assert.match(helper, /live_qualification: false/);
  assert.match(helper, /mutating: false/);
  assert.doesNotMatch(helper, /read_to_string\([^)]*credential/i);
});

test("node, endpoint, and credential templates fail closed without real secrets", () => {
  const node = read("deploy/celld/node.env.example");
  const endpoint = read("deploy/celld/endpoint.env.example");
  const credential = read("deploy/celld/object-store.credentials.example");

  assert.match(node, /^CELLD_STORAGE_PROBE=1$/m);
  assert.match(node, /^AGENTIC_CELLD_EXPECTED_BINARY_SHA256=sha256:REPLACE_/m);
  assert.match(endpoint, /^S3_ENDPOINT=https:\/\/s3\.example\.invalid$/m);
  assert.match(credential, /REPLACE_WITH_SCOPED_ACCESS_KEY/);
  assert.match(credential, /REPLACE_WITH_SCOPED_SECRET_KEY/);
  assert.doesNotMatch(`${node}\n${endpoint}\n${credential}`, /AKIA[0-9A-Z]{16}/);
});

test("diagnose example is strict fixture evidence and cannot claim live success", () => {
  const example = JSON.parse(read("deploy/celld/fleet-diagnose.example.json"));
  assert.equal(example.schema_version, "agentic-sandbox.celld-fleet-diagnose/v1");
  assert.equal(example.source, "fixture");
  assert.equal(example.manifest.nodes.count, 3);
  assert.equal(example.nodes.length, 3);
  assert.equal(Object.hasOwn(example, "status"), false);
  assert.equal(Object.hasOwn(example, "live_qualification"), false);
});

test("CLI, API, and OpenAPI expose diagnose and non-mutating upgrade plans", () => {
  const cli = read("cli/src/main.rs");
  const command = read("cli/src/cmd/celld.rs");
  const http = read("management/src/http/celld.rs");
  const openapi = read("docs/contracts/celld-api.openapi.yaml");
  const validation = read("management/src/celld/validation.rs");

  assert.match(cli, /CelldCommands::Diagnose/);
  assert.match(command, /fleets\/diagnose/);
  assert.match(http, /route\("\/api\/v2\/celld\/fleets\/diagnose", post\(diagnose\)\)/);
  assert.match(openapi, /\/fleets\/diagnose:/);
  assert.match(openapi, /operationId: diagnoseCelldFleet/);
  assert.match(openapi, /mutating: \{ const: false \}/);
  assert.match(openapi, /execution_controller: \{ type: 'null' \}/);
  assert.match(validation, /mutating: false/);
  assert.match(validation, /execution_controller: None/);
});
