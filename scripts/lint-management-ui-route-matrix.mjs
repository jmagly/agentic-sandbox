#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
const matrix = JSON.parse(readFileSync(resolve(root, 'management/ui/route-matrix.json'), 'utf8'));
const sources = [
    ['management/src/http/server.rs', ''],
    ['management/src/http/admin_v2.rs', '/api/v2/admin'],
    ['management/src/http/credentials.rs', '/api/v2/credentials'],
    ['management/src/http/credential_proxy.rs', '/api/v2/credential-proxy'],
    ['management/src/http/startup_profiles.rs', '/api/v2/startup-profiles'],
    ['management/src/http/ssh_gateway.rs', '/api/v2/gateway/ssh'],
    ['management/src/http/activity.rs', '/api/v2/activity'],
    ['management/src/http/fleet.rs', '/api/v2/fleet'],
    ['management/src/http/celld.rs', ''],
    ['management/src/http/mcp.rs', ''],
    ['management/agentic-sandbox-executor/src/bindings/rest.rs', ''],
];

const normalize = (path) => path.replaceAll('{*path}', '{path}');
const implemented = new Set();
for (const [file, mount] of sources) {
    const source = readFileSync(resolve(root, file), 'utf8').split('\n#[cfg(test)]')[0];
    for (const match of source.matchAll(/\.route\(\s*"([^"]+)"/gs)) {
        implemented.add(normalize(mount + match[1]));
    }
}
// This path is registered through a named constant rather than a string
// literal in `.route(...)` so keep the constant itself under drift control.
implemented.add('/api/v2/celld/effects');

const classified = new Map();
for (const [classification, paths] of Object.entries(matrix.routes)) {
    for (const path of paths) {
        if (classified.has(path)) {
            throw new Error(`${path} is classified twice (${classified.get(path)}, ${classification})`);
        }
        classified.set(path, classification);
    }
}

const missing = [...implemented].filter((path) => !classified.has(path)).sort();
const unavailable = new Set(matrix.routes.capability_unavailable || []);
const stale = [...classified.keys()]
    .filter((path) => !implemented.has(path) && !unavailable.has(path))
    .sort();
if (missing.length || stale.length) {
    if (missing.length) console.error('Unclassified server routes:\n' + missing.map((p) => `  ${p}`).join('\n'));
    if (stale.length) console.error('Matrix routes absent from server:\n' + stale.map((p) => `  ${p}`).join('\n'));
    process.exit(1);
}

for (const [action, classification] of Object.entries(matrix.dashboard_actions)) {
    if (!Object.hasOwn(matrix.routes, classification)) {
        console.error(`Dashboard action ${action} names unknown classification ${classification}`);
        process.exit(1);
    }
}
console.log(`management UI route matrix covers ${implemented.size} server routes and ${Object.keys(matrix.dashboard_actions).length} action domains`);
