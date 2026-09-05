import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
    HttpOutcomeError,
    HttpTransport,
    RequestOwnership,
    ResourceState,
    ResumableEventBuffer,
    StaleResponseError,
    UnknownMutationOutcomeError,
    encodedAccessPathSegment,
    normalizeResponse,
    operationFromAccepted,
    parseInstanceCollection,
    projectAccessAuditMetadata,
    projectCredentialLeaseMetadata,
    projectCredentialMetadata,
    projectSshLeaseMetadata,
    reconcileUnknownInstanceOperation,
    resolveWebSocketUrl,
    requireAvailableRuntime,
} from '../modules/index.mjs';

const root = new URL('../../../', import.meta.url);
const read = (path) => readFile(new URL(path, root), 'utf8');
const [dashboardHtml, appSource, styles, fixtureText, routeMatrixText] = await Promise.all([
    read('management/ui/index.html'),
    read('management/ui/app.js'),
    read('management/ui/styles.css'),
    read('management/ui/test/fixtures/http-outcomes.json'),
    read('management/ui/route-matrix.json'),
]);
const fixture = JSON.parse(fixtureText);
const routeMatrix = JSON.parse(routeMatrixText);

function response(item) {
    return new Response(
        typeof item.body === 'string' ? item.body : JSON.stringify(item.body),
        { status: item.status, headers: item.headers },
    );
}

function jsonResponse(status, body, headers = {}) {
    return new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json', ...headers },
    });
}

test('critical management workflows have deterministic transport fixtures', () => {
    const ids = new Set(fixture.operations.map(({ id }) => id));
    const required = [
        'instance.list', 'instance.create', 'instance.start', 'instance.stop',
        'operation.detail', 'activity.timeline', 'activity.export',
        'fleet.inventory', 'fleet.reconcile', 'celld.status', 'celld.command',
        'celld.fleet.preflight', 'celld.fleet.plan_upgrade',
        'startup.list', 'startup.create', 'startup.detail', 'startup.update', 'startup.delete',
        'loadout.list', 'loadout.create', 'storage.read', 'storage.write', 'storage.delete',
        'acceleration.ch.snapshot', 'acceleration.ch.restore', 'acceleration.ch.fork',
        'acceleration.ch.pool_init', 'acceleration.ch.pool_handoff',
        'acceleration.libvirt.checkpoint', 'acceleration.libvirt.restore',
        'acceleration.libvirt.pool_init', 'acceleration.libvirt.pool_handoff', 'mcp.discovery',
        'access.authority', 'access.audit', 'credential.list',
        'credential.lease.issue', 'credential.lease.revoke',
        'ssh.lease.list', 'ssh.lease.issue', 'ssh.lease.revoke',
    ];
    for (const id of required) assert.ok(ids.has(id), `missing fixture for ${id}`);
    const classifiedRoutes = new Set(Object.values(routeMatrix.routes).flat());
    for (const operation of fixture.operations) {
        assert.ok(classifiedRoutes.has(operation.path), `unclassified route ${operation.path}`);
    }
    for (const domain of Object.keys(routeMatrix.dashboard_actions)) {
        assert.ok(routeMatrix.routes[routeMatrix.dashboard_actions[domain]], `unknown route class for ${domain}`);
    }
});

test('HTTP outcomes preserve evidence and distinguish degraded modes', async () => {
    const expected = {
        success: 'success', accepted: 'accepted', problem: 'http_error',
        forbidden: 'forbidden', conflict: 'conflict', rate_limited: 'rate_limited',
        unavailable: 'unavailable', non_json_error: 'non_json_error',
    };
    for (const [name, item] of Object.entries(fixture.outcomes)) {
        const outcome = await normalizeResponse(response(item));
        assert.equal(outcome.kind, expected[name]);
        assert.equal(outcome.requestId, item.headers['x-request-id']);
    }
    const limited = await normalizeResponse(response(fixture.outcomes.rate_limited));
    assert.equal(limited.retryAfterMs, 3000);
    const replayed = await normalizeResponse(jsonResponse(202, { id: 'op-replayed' }, {
        'idempotency-replayed': 'true',
    }));
    assert.equal(replayed.idempotencyReplayed, true);
});

test('an acknowledged no-content mutation is a known successful outcome', async () => {
    const transport = new HttpTransport({
        fetchImpl: async () => new Response(null, { status: 204 }),
    });
    const outcome = await transport.request('/api/v2/startup-profiles/unused', {
        method: 'DELETE', owner: 'startup-delete', idempotencyKey: 'startup-delete-1',
    });
    assert.equal(outcome.kind, 'success');
    assert.equal(outcome.bodyKind, 'empty');
});

test('canonical inventory parser accepts items and rejects unknown major versions', () => {
    const items = [{ id: 'worker', name: 'worker', runtime: 'host' }];
    assert.deepEqual(parseInstanceCollection({ items, degraded_providers: [] }), items);
    assert.throws(() => parseInstanceCollection({ schema_version: '2.0', items }));
    assert.throws(() => parseInstanceCollection({ degraded_providers: [] }));
});

test('instance creation fails closed against runtime discovery', () => {
    const runtimes = new Map([
        ['qemu', { id: 'qemu', available: true }],
        ['docker', { id: 'docker', available: false, unavailable_reason: 'daemon offline' }],
    ]);
    assert.equal(requireAvailableRuntime(runtimes, 'vm').id, 'qemu');
    assert.throws(
        () => requireAvailableRuntime(runtimes, 'container'),
        (error) => error.code === 'runtime_unavailable' && error.message.includes('daemon offline'),
    );
    assert.throws(
        () => requireAvailableRuntime(runtimes, 'host'),
        (error) => error.code === 'runtime_unavailable' && error.message.includes('capability discovery'),
    );
});

test('management WebSocket URLs preserve page origin and reject mixed content', () => {
    assert.equal(
        resolveWebSocketUrl('https://console.example/ui/index.html'),
        'wss://console.example/',
    );
    assert.equal(
        resolveWebSocketUrl('https://console.example/ui/', '/management/ws'),
        'wss://console.example/management/ws',
    );
    assert.equal(
        resolveWebSocketUrl('http://localhost:8122/', 'ws://localhost:8121/'),
        'ws://localhost:8121/',
    );
    assert.equal(
        resolveWebSocketUrl('http://console.internal:8122/', 'ws://{host}:8121/'),
        'ws://console.internal:8121/',
    );
    assert.throws(() => resolveWebSocketUrl(
        'https://console.example/', 'ws://console.example:8121/',
    ));
});

test('published management error envelopes preserve actionable failure details', async () => {
    for (const body of [
        { error: { code: 'revision.stale', message: 'Refresh the workload' } },
        { error: 'Refresh the workload' },
    ]) {
        const outcome = await normalizeResponse(jsonResponse(409, body));
        assert.equal(outcome.kind, 'conflict');
        assert.equal(outcome.message, 'Refresh the workload');
        assert.equal(outcome.code, body.error.code || 'http_409');
    }
});

test('malformed successful mutation responses require reconciliation', async () => {
    const transport = new HttpTransport({
        fetchImpl: async () => new Response('truncated JSON', {
            status: 202, headers: { 'content-type': 'application/json' },
        }),
    });
    const error = await transport.request('/api/v2/admin/instances', {
        method: 'POST', expectJson: true,
    }).catch((value) => value);
    assert.ok(error instanceof UnknownMutationOutcomeError);
});

test('empty successful mutation responses require reconciliation', async () => {
    const transport = new HttpTransport({
        fetchImpl: async () => new Response(null, { status: 202 }),
    });
    const error = await transport.request('/api/v2/admin/instances/worker/start', {
        method: 'POST', expectJson: true,
    }).catch((value) => value);
    assert.ok(error instanceof UnknownMutationOutcomeError);
});

test('unknown instance outcomes need authoritative transition evidence', () => {
    const checkedAt = '2026-09-04T12:00:00Z';
    const provision = reconcileUnknownInstanceOperation({
        kind: 'instance.provision', target: 'worker',
        reconciliation_before: { authoritative: true, instance_present: false },
    }, new Map([['worker', { id: 'instance-1', name: 'worker', state: 'running' }]]), checkedAt);
    assert.equal(provision.reconciled, true);
    assert.equal(provision.evidence.checked_at, checkedAt);

    const ambiguousProvision = reconcileUnknownInstanceOperation({
        kind: 'instance.provision', target: 'worker',
        reconciliation_before: { authoritative: false, instance_present: false },
    }, [{ id: 'instance-1', name: 'worker', state: 'running' }], checkedAt);
    assert.equal(ambiguousProvision.reconciled, false);

    const destroy = reconcileUnknownInstanceOperation({
        kind: 'instance.destroy', target: 'worker', target_id: 'instance-1',
        reconciliation_before: { authoritative: true, instance_present: true, instance_id: 'instance-1' },
    }, [], checkedAt);
    assert.equal(destroy.reconciled, true);

    const ambiguousDestroy = reconcileUnknownInstanceOperation({
        kind: 'instance.destroy', target: 'worker',
    }, [], checkedAt);
    assert.equal(ambiguousDestroy.reconciled, false);
});

test('critical management workflows complete against deterministic mocked boundaries', async () => {
    const expected = [
        ['POST', '/api/v2/admin/instances', jsonResponse(202, { operation: { id: 'op-provision', state: 'accepted' } }, { 'operation-id': 'op-provision' })],
        ['GET', '/api/v2/admin/operations/op-provision', jsonResponse(200, { id: 'op-provision', state: 'succeeded' })],
        ['GET', '/api/v2/activity/coverage', jsonResponse(200, { complete: true, unsupported_event_classes: [] })],
        ['POST', '/api/v2/activity/export', jsonResponse(200, { artifact_id: 'activity-export-1', signed: true })],
        ['POST', '/api/v2/fleet/workloads/worker-1/observations', jsonResponse(412, { code: 'revision.stale', detail: 'expected revision is stale' })],
        ['POST', '/api/v2/fleet/reconcile', jsonResponse(200, {
            document_type: 'reconciliation', api_version: 'agentic-orchestration/v1',
            rows: [{ child_id: 'worker-1', classification: 're-adopted' }],
        }, { 'operation-id': 'op-fleet', location: '/api/v2/admin/operations/op-fleet' })],
        ['POST', '/api/v2/credentials/cred-1/leases', jsonResponse(201, { id: 'lease-1', state: 'active', expires_at: '2100-01-01T00:00:00Z' })],
        ['DELETE', '/api/v2/credentials/leases/lease-1', jsonResponse(200, { id: 'lease-1', state: 'revoked' })],
        ['POST', '/api/v2/gateway/ssh/leases', jsonResponse(201, { id: 'ssh-1', state: 'active', certificate: 'secret-certificate-body' })],
        ['DELETE', '/api/v2/gateway/ssh/leases/ssh-1', jsonResponse(200, { id: 'ssh-1', state: 'revoked' })],
        ['POST', '/api/v2/celld/fleets/preflight', jsonResponse(200, { valid: true, generation: 7 })],
        ['POST', '/api/v2/celld/fleets/plan-upgrade', jsonResponse(200, { plan_id: 'plan-1', mutating: false })],
    ];
    const observed = [];
    const transport = new HttpTransport({
        fetchImpl: async (url, options) => {
            const [method, expectedUrl, result] = expected.shift();
            observed.push([options.method, String(url)]);
            assert.equal(options.method, method);
            assert.equal(String(url), expectedUrl);
            return result;
        },
    });
    const state = new ResourceState();

    const provision = await transport.request('/api/v2/admin/instances', { method: 'POST', owner: 'provision' });
    const operation = operationFromAccepted(provision, { target: 'agent-01', kind: 'instance.provision' });
    state.trackOperation(operation);
    const reconciledOperation = await transport.request('/api/v2/admin/operations/op-provision', { owner: 'operation' });
    state.trackOperation(reconciledOperation.body);
    assert.equal(state.operations.get('op-provision').state, 'succeeded');

    const coverage = await transport.request('/api/v2/activity/coverage', { owner: 'activity-coverage' });
    const exported = await transport.request('/api/v2/activity/export', { method: 'POST', owner: 'activity-export' });
    assert.equal(coverage.body.complete, true);
    assert.equal(exported.body.signed, true);

    const conflict = await transport.request('/api/v2/fleet/workloads/worker-1/observations', {
        method: 'POST', owner: 'workload-observe',
    }).catch((error) => error);
    assert.ok(conflict instanceof HttpOutcomeError);
    assert.equal(conflict.outcome.kind, 'conflict');
    const fleet = await transport.request('/api/v2/fleet/reconcile', { method: 'POST', owner: 'fleet-reconcile' });
    assert.equal(fleet.kind, 'success');
    assert.equal(fleet.operationId, 'op-fleet');
    assert.equal(fleet.body.rows[0].classification, 're-adopted');

    const credentialIssued = await transport.request('/api/v2/credentials/cred-1/leases', {
        method: 'POST', owner: 'credential-issue',
    });
    const credentialRevoked = await transport.request('/api/v2/credentials/leases/lease-1', {
        method: 'DELETE', owner: 'credential-revoke',
    });
    assert.equal(projectCredentialLeaseMetadata(credentialIssued.body).state, 'active');
    assert.equal(projectCredentialLeaseMetadata(credentialRevoked.body).state, 'revoked');

    const sshIssued = await transport.request('/api/v2/gateway/ssh/leases', { method: 'POST', owner: 'ssh-issue' });
    const sshRevoked = await transport.request('/api/v2/gateway/ssh/leases/ssh-1', { method: 'DELETE', owner: 'ssh-revoke' });
    assert.equal(JSON.stringify(projectSshLeaseMetadata(sshIssued.body)).includes('secret-certificate-body'), false);
    assert.equal(projectSshLeaseMetadata(sshRevoked.body).state, 'revoked');

    const preflight = await transport.request('/api/v2/celld/fleets/preflight', { method: 'POST', owner: 'celld-preflight' });
    assert.equal(preflight.body.valid, true);
    const plan = await transport.request('/api/v2/celld/fleets/plan-upgrade', { method: 'POST', owner: 'celld-plan' });
    assert.equal(plan.body.mutating, false);
    assert.equal(expected.length, 0);
    assert.equal(observed.length, 12);
});

test('selection cancellation prevents stale data replacement', async () => {
    let release;
    const blocked = new Promise((resolve) => { release = resolve; });
    const transport = new HttpTransport({
        ownership: new RequestOwnership(),
        fetchImpl: async (url) => {
            if (url.endsWith('/old')) await blocked;
            return response(fixture.outcomes.success);
        },
    });
    const old = transport.request('/old', { owner: 'detail', timeoutMs: 0 }).catch((error) => error);
    const current = transport.request('/current', { owner: 'detail', timeoutMs: 0 });
    release();
    assert.equal((await current).ok, true);
    const stale = await old;
    assert.ok(stale instanceof StaleResponseError
        || (stale instanceof HttpOutcomeError && stale.outcome.kind === 'aborted'));
});

test('ambiguous mutation executes once and requires reconciliation', async () => {
    let calls = 0;
    const transport = new HttpTransport({
        fetchImpl: async () => { calls += 1; throw new TypeError('response lost'); },
    });
    const error = await transport.request('/api/v2/credentials/cred-1/leases', {
        owner: 'credential-lease-issue', method: 'POST', timeoutMs: 0,
    }).catch((value) => value);
    assert.equal(calls, 1);
    assert.ok(error instanceof UnknownMutationOutcomeError);
    assert.equal(error.outcomeUnknown, true);
    assert.equal(error.shouldReconcile, true);
    assert.equal(error.replayAllowed, false);
});

test('mutation cancellation after dispatch preserves the intent for reconciliation', async () => {
    for (const cancellation of ['caller', 'timeout', 'superseded']) {
        const controller = new AbortController();
        let calls = 0;
        const ownership = new RequestOwnership();
        const transport = new HttpTransport({
            ownership,
            fetchImpl: async (_url, { signal }) => {
                calls += 1;
                const result = new Promise((_resolve, reject) => {
                    signal.addEventListener('abort', () => reject(signal.reason), { once: true });
                });
                if (cancellation === 'caller') controller.abort();
                if (cancellation === 'superseded') ownership.cancel('mutation');
                return result;
            },
        });
        const error = await transport.request('/api/v2/admin/instances/worker/start', {
            owner: 'mutation', method: 'POST', signal: controller.signal,
            timeoutMs: cancellation === 'timeout' ? 1 : 0,
            headers: { 'Idempotency-Key': 'intent-start-worker' },
        }).catch((value) => value);
        assert.ok(error instanceof UnknownMutationOutcomeError, cancellation);
        assert.equal(error.idempotencyKey, 'intent-start-worker');
        assert.equal(error.replayAllowed, false);
        assert.equal(calls, 1);
        assert.equal(ownership.entries.size, 0);
    }
});

test('pre-aborted mutations never dispatch and remain ordinary cancellations', async () => {
    const controller = new AbortController();
    controller.abort();
    let calls = 0;
    const transport = new HttpTransport({ fetchImpl: async () => { calls += 1; } });
    const error = await transport.request('/api/v2/admin/instances', {
        method: 'POST', signal: controller.signal,
    }).catch((value) => value);
    assert.ok(error instanceof HttpOutcomeError);
    assert.equal(error.outcome.kind, 'aborted');
    assert.equal(calls, 0);
});

test('late mutation responses after selection changes still require reconciliation', async () => {
    let release;
    const ownership = new RequestOwnership();
    const transport = new HttpTransport({
        ownership,
        fetchImpl: () => new Promise((resolve) => { release = resolve; }),
    });
    const pending = transport.request('/api/v2/admin/instances/worker/start', {
        owner: 'mutation', method: 'POST', timeoutMs: 0,
    }).catch((value) => value);
    ownership.cancel('mutation');
    release(jsonResponse(202, { operation_id: 'op-late' }));
    assert.ok(await pending instanceof UnknownMutationOutcomeError);
});

test('high-rate stream state remains bounded and duplicate safe', () => {
    const stream = new ResumableEventBuffer({ limit: 200 });
    for (let index = 0; index < 10_000; index += 1) stream.append({ id: `event-${index}` });
    assert.equal(stream.items.length, 200);
    assert.equal(stream.seen.size, 200);
    assert.equal(stream.lastEventId, 'event-9999');
    assert.equal(stream.append({ id: 'event-9999' }), false);
    stream.markGap({ reason: 'cursor_before_retained_window' });
    assert.equal(stream.incomplete, true);
});

test('credential, SSH, and audit views are metadata-only under hostile input', () => {
    const hostile = '<img src=x onerror=globalThis.executed=true> sk-hostile';
    const projected = {
        credential: projectCredentialMetadata({
            id: hostile, provider: 'openai', type: 'api_key', configured: true,
            backend: { kind: 'file', reference: '/run/secrets/provider' },
            value: 'sk-value', plaintext: 'sk-plaintext',
        }),
        credentialLease: projectCredentialLeaseMetadata({
            id: 'lease-1', state: 'active', expires_at: '2000-01-01T00:00:00Z',
            proxy_policy: { authorization: 'Bearer secret' },
        }, Date.parse('2026-01-01T00:00:00Z')),
        sshLease: projectSshLeaseMetadata({
            id: 'ssh-1', public_key: hostile, certificate: 'ssh-certificate secret',
        }),
        audit: projectAccessAuditMetadata({
            id: 'audit-1', details: { secret: hostile }, action: 'credential_lease_issue',
        }),
    };
    const serialized = JSON.stringify(projected);
    for (const forbidden of [
        '/run/secrets/provider', 'sk-value', 'sk-plaintext', 'Bearer secret', 'ssh-certificate',
    ]) assert.equal(serialized.includes(forbidden), false, `projection leaked ${forbidden}`);
    assert.equal(projected.credentialLease.state, 'expired');
    assert.equal(Object.hasOwn(projected.sshLease, 'public_key'), false);
    assert.equal(Object.hasOwn(projected.audit, 'details'), false);

    const revoked = projectCredentialLeaseMetadata({ id: 'lease-2', state: 'revoked' });
    assert.equal(revoked.state, 'revoked');
    assert.deepEqual(projectCredentialMetadata({}).scopes, []);

    const accessSource = appSource.slice(
        appSource.indexOf('    hasAccessPermission('),
        appSource.indexOf('    setWorkspaceStatus('),
    );
    assert.equal(accessSource.includes('.innerHTML'), false);
    assert.equal(accessSource.includes('navigator.clipboard'), false);
    assert.equal(accessSource.includes('localStorage'), false);
    assert.equal(accessSource.includes('console.'), false);
    assert.equal(accessSource.includes('return error.message'), false);
    const encoded = encodedAccessPathSegment('id/../?secret=sk-url#fragment');
    assert.equal(encoded.includes('/'), false);
    assert.equal(encoded.includes('?'), false);
    assert.equal(encoded.includes('#'), false);
});

test('CSP and dialog keyboard contracts are enforceable from the shipped page', () => {
    const csp = dashboardHtml.match(/Content-Security-Policy" content="([^"]+)"/)?.[1] || '';
    for (const directive of [
        "default-src 'self'", "script-src 'self'", "object-src 'none'",
        "frame-ancestors 'none'", "form-action 'self'",
    ]) assert.ok(csp.includes(directive), `missing CSP directive ${directive}`);
    assert.equal(csp.includes("'unsafe-eval'"), false);

    for (const id of [
        'deprecation-modal', 'detail-modal', 'oauth-modal', 'confirm-modal',
        'create-vm-modal', 'create-session-modal',
    ]) {
        const tag = dashboardHtml.match(new RegExp(`<div id="${id}"[^>]+>`))?.[0] || '';
        assert.match(tag, /role="(?:dialog|alertdialog)"/);
        assert.match(tag, /aria-modal="true"/);
        assert.match(tag, /aria-labelledby=/);
    }
    assert.ok(appSource.includes("if (event.key !== 'Tab') return;"));
    assert.ok(appSource.includes('event.shiftKey && document.activeElement === first'));
    assert.ok(appSource.includes('document.activeElement === last'));
    assert.ok(appSource.includes("if (event.key === 'Escape')"));
    assert.ok(appSource.includes('modal.__returnFocus'));
    assert.ok(appSource.includes('restoreWorkspaceListFocus'));
    assert.match(dashboardHtml, /id="toast-container"[^>]+aria-live="polite"/);
    assert.match(dashboardHtml, /class="sidebar-tabs" role="tablist"/);
    assert.ok(appSource.includes("event.key === 'ArrowRight'"));
});

test('terminal exposes interactive/read-only posture, focus escape, and reduced motion', () => {
    assert.ok(appSource.includes("setAttribute('aria-readonly'"));
    assert.ok(appSource.includes("setAttribute('role', 'region')"));
    assert.ok(appSource.includes('_ptyV2UpdateConnection'));
    assert.equal(appSource.includes("xtermWrapper.setAttribute('aria-live'"), false);
    assert.ok(appSource.includes("ev.ctrlKey && ev.shiftKey && ev.key === 'Escape'"));
    assert.ok(appSource.includes('terminal-focus-exit'));
    assert.match(styles, /@media\s*\(prefers-reduced-motion:\s*reduce\)/);
});
