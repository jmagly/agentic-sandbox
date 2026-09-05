/**
 * Agentic Sandbox Control Plane
 * Per-agent pane dashboard with independent output tracking
 */

// Build-free modular boundary (#804). New contract-driven work imports this
// promise instead of adding transport/state/view responsibilities to this
// legacy compatibility entry point. Keeping the promise on `window` lets the
// existing classic-script self-tests continue to load without a bundler.
if (typeof window !== 'undefined') {
    window.ManagementUIReady = import('./modules/index.mjs').then((boundary) => {
        window.ManagementUI = Object.freeze({ ...boundary });
        return window.ManagementUI;
    });
}

// ============================================================================
// ApiClient (#244) — v1→v2 admin migration wrapper with Sunset-fallback.
// Tries the v2 admin path first; on 404, falls back to v1 and surfaces the
// Sunset header so the UI can warn the operator. Unmapped v1 paths go
// straight to v1 (also surfacing Sunset). Mirror of compat_v1.rs::path_map().
// ============================================================================
const ApiClient = {
    // Static prefix map: v1 prefix → v2 prefix. Order-independent; the
    // longest-matching prefix wins so /api/v1/vms/{name}/start maps onto
    // /api/v2/admin/instances/{name}/start (single-resource instance ops),
    // while bare /api/v1/vms maps onto /api/v2/admin/instances (list).
    // Mirrors path_map() in management/src/http/compat_v1.rs.
    V2_PREFIX_MAP: [
        ['/api/v1/agents', '/api/v2/admin/instances'],
        ['/api/v1/vms', '/api/v2/admin/instances'],
        ['/api/v1/operations', '/api/v2/admin/operations'],
        ['/api/v1/storage', '/api/v2/admin/storage'],
        ['/api/v1/container-images', '/api/v2/admin/container-images'],
        // Paths with no v2 admin equivalent — intentionally absent so toV2
        // returns null and the wrapper goes straight to v1:
        //   /api/v1/containers, /api/v1/loadouts, /api/v1/loadout/registry,
        //   /api/v1/aiwg/*, /api/v1/events, /api/v1/logs,
        //   /api/v1/sessions/{id}/dispatch (semantic A2A shift),
        //   /api/v1/hitl/{id} (A2A input-required), /api/v1/ws/* (SSE shift).
    ],

    // A 404 is only an unambiguous "v2 route unavailable" signal for these
    // collection reads. A resource/detail 404 means the canonical v2 route
    // exists but the resource does not, so falling through to v1 would hide
    // the real outcome. Network failures are never a route-availability
    // signal (#802).
    V1_FALLBACK_READS: new Set([
        '/api/v1/agents',
        '/api/v1/vms',
        '/api/v1/container-images',
    ]),

    isSafeMethod(method) {
        return ['GET', 'HEAD', 'OPTIONS'].includes(String(method || 'GET').toUpperCase());
    },

    pathWithoutQuery(path) {
        const qIdx = String(path).indexOf('?');
        return qIdx === -1 ? String(path) : String(path).slice(0, qIdx);
    },

    newIntentId() {
        if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
            return globalThis.crypto.randomUUID();
        }
        return `ui-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    },

    withMutationIntent(opts = {}) {
        const request = { ...opts };
        const headers = opts.headers instanceof Headers
            ? new Headers(opts.headers)
            : { ...(opts.headers || {}) };
        const method = String(request.method || 'GET').toUpperCase();
        if (!ApiClient.isSafeMethod(method) && !ApiClient.headerValue(headers, 'Idempotency-Key')) {
            const value = request.idempotencyKey || ApiClient.newIntentId();
            if (headers instanceof Headers) headers.set('Idempotency-Key', value);
            else headers['Idempotency-Key'] = value;
        }
        delete request.idempotencyKey;
        request.headers = headers;
        return request;
    },

    headerValue(headers, name) {
        if (!headers) return null;
        if (headers instanceof Headers) return headers.get(name);
        const match = Object.keys(headers).find(key => key.toLowerCase() === name.toLowerCase());
        return match ? headers[match] : null;
    },

    /**
     * Translate a v1 path to its v2 admin equivalent. Returns null if no
     * mapping exists (caller should go straight to v1).
     *
     * Matching is longest-prefix: '/api/v1/vms/abc/start' → '/api/v2/admin/instances/abc/start'.
     * Query strings are preserved unchanged.
     */
    toV2(v1Path) {
        if (typeof v1Path !== 'string' || !v1Path.startsWith('/api/v1/')) {
            return null;
        }
        // Split path and query so we don't accidentally match across '?'.
        const qIdx = v1Path.indexOf('?');
        const pathOnly = qIdx === -1 ? v1Path : v1Path.slice(0, qIdx);
        const query = qIdx === -1 ? '' : v1Path.slice(qIdx);

        // Find longest matching prefix.
        let bestMatch = null;
        for (const [v1Prefix, v2Prefix] of ApiClient.V2_PREFIX_MAP) {
            // Exact match OR prefix followed by '/' (avoid /api/v1/vms matching /api/v1/vmsfoo).
            if (pathOnly === v1Prefix || pathOnly.startsWith(v1Prefix + '/')) {
                if (!bestMatch || v1Prefix.length > bestMatch[0].length) {
                    bestMatch = [v1Prefix, v2Prefix];
                }
            }
        }
        if (!bestMatch) return null;
        const [v1Prefix, v2Prefix] = bestMatch;
        const rest = pathOnly.slice(v1Prefix.length); // '' or '/...'
        return v2Prefix + rest + query;
    },

    _sunsetListeners: [],
    onSunset(cb) { ApiClient._sunsetListeners.push(cb); },
    _notifySunset(path, date, link) {
        for (const cb of ApiClient._sunsetListeners) {
            try { cb(path, date, link); } catch (e) { console.error('sunset listener error', e); }
        }
    },

    /**
     * Make a request. Mapped mutations have one canonical v2 destination and
     * never cross-version replay. Compatibility fallback is limited to known
     * collection reads whose v2 endpoint positively returns 404.
     *
     * Returns { response, viaV1: bool, sunsetDate: string | null }.
     *
     * NOTE: This wrapper does NOT consume the response body — the caller
     * still calls resp.json()/resp.text() as before.
     */
    async request(path, opts = {}) {
        const v2Path = ApiClient.toV2(path);
        const request = ApiClient.withMutationIntent(opts);
        const method = String(request.method || 'GET').toUpperCase();
        if (v2Path) {
            try {
                const r = await fetch(v2Path, request);
                const mayFallback = ApiClient.isSafeMethod(method)
                    && r.status === 404
                    && ApiClient.V1_FALLBACK_READS.has(ApiClient.pathWithoutQuery(path));
                if (!mayFallback) {
                    return { response: r, viaV1: false, sunsetDate: null };
                }
                // Exact collection 404 is the sole positive compatibility
                // signal. Preserve Sunset telemetry on the v1 response.
            } catch (e) {
                if (!ApiClient.isSafeMethod(method)) {
                    const idempotencyKey = ApiClient.headerValue(request.headers, 'Idempotency-Key');
                    throw new UnknownMutationOutcomeError({
                        method,
                        path: v2Path,
                        idempotencyKey,
                        cause: e,
                    });
                }
                throw e;
            }
        }
        let r;
        try {
            r = await fetch(path, request);
        } catch (e) {
            const isCanonicalV2Mutation = !ApiClient.isSafeMethod(method)
                && String(path).startsWith('/api/v2/');
            if (isCanonicalV2Mutation) {
                throw new UnknownMutationOutcomeError({
                    method,
                    path,
                    idempotencyKey: ApiClient.headerValue(request.headers, 'Idempotency-Key'),
                    cause: e,
                });
            }
            throw e;
        }
        const sunset = r.headers ? r.headers.get('Sunset') : null;
        const link = r.headers ? r.headers.get('Link') : null;
        if (sunset) {
            console.warn('v1 fallback in use', { path, sunset, link });
            ApiClient._notifySunset(path, sunset, link);
        }
        return { response: r, viaV1: true, sunsetDate: sunset };
    },

    // Convenience: GET and JSON-parse. Throws on non-OK. Returns the parsed body.
    async getJson(path) {
        const { response } = await ApiClient.request(path);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return response.json();
    },

    // Convenience: POST JSON. Returns the raw response (caller decides body).
    async postJson(path, body) {
        const { response } = await ApiClient.request(path, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        return response;
    },

    // Convenience: bare POST with no body. Returns the raw response.
    async post(path) {
        const { response } = await ApiClient.request(path, { method: 'POST' });
        return response;
    },

    // Convenience: DELETE. Returns the raw response.
    async del(path) {
        const { response } = await ApiClient.request(path, { method: 'DELETE' });
        return response;
    },
};

class UnknownMutationOutcomeError extends Error {
    constructor({ method, path, idempotencyKey, cause }) {
        super(`Outcome unknown for ${method} ${path}; reconcile authoritative state before retrying.`);
        this.name = 'UnknownMutationOutcomeError';
        this.code = 'mutation_outcome_unknown';
        this.method = method;
        this.path = path;
        this.idempotencyKey = idempotencyKey || null;
        this.cause = cause;
        this.outcomeUnknown = true;
    }
}

// Expose for console diagnostics + unit-test page.
if (typeof window !== 'undefined') {
    window.ApiClient = ApiClient;
    window.UnknownMutationOutcomeError = UnknownMutationOutcomeError;
}

// === #245 AgentCard panel ===
// A2A Identity inspector: fetches a signed AgentCard per instance and
// renders name/version, signature status, extensions, supported interfaces,
// and raw JSON. Best-effort Ed25519 verification in-browser; falls back to
// "server-trusted" when SubtleCrypto can't verify EdDSA on this platform.

const EXT_DOC_LINKS = {
    'https://agentic-sandbox.aiwg.io/extensions/runtime/v1':
        '/docs/contracts/extensions/runtime/v1/spec.md',
    'https://agentic-sandbox.aiwg.io/extensions/idempotency/v1':
        '/docs/contracts/extensions/idempotency/v1/spec.md',
    'https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1':
        '/docs/contracts/extensions/hitl-prompt/v1/spec.md',
    'https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1':
        '/docs/contracts/extensions/multi-tenant/v1/spec.md',
    'https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1':
        '/docs/contracts/extensions/pty-extensions/v1/spec.md',
    'https://agentic-sandbox.aiwg.io/extensions/agent-output/v1':
        '/docs/contracts/extensions/agent-output/v1/spec.md',
};

function escAttr(s) {
    return String(s == null ? '' : s)
        .replace(/&/g, '&amp;').replace(/"/g, '&quot;')
        .replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function cssToken(s) {
    return String(s == null ? '' : s)
        .toLowerCase()
        .replace(/[^a-z0-9_-]+/g, '-') || 'unknown';
}

function summarizeExtParams(uri, params) {
    if (!params || typeof params !== 'object') return '';
    if (uri.endsWith('/extensions/runtime/v1')) {
        const bits = [];
        if (params.kind) bits.push(`runtime=${params.kind}`);
        if (params.loadout) bits.push(`loadout=${params.loadout}`);
        if (params.imageRef) bits.push(`image=${params.imageRef}`);
        return bits.join(', ');
    }
    if (uri.endsWith('/extensions/idempotency/v1')) {
        const bits = [];
        if (params.ttl != null) bits.push(`ttl=${params.ttl}s`);
        if (params.max_entries != null) bits.push(`max_entries=${params.max_entries}`);
        return bits.join(', ') || '…';
    }
    // Default: compact JSON, truncated.
    try {
        const s = JSON.stringify(params);
        return s.length > 80 ? s.slice(0, 77) + '…' : s;
    } catch (_) {
        return '…';
    }
}

async function verifyCardSignature(card, instanceId) {
    if (!card || !Array.isArray(card.signatures) || !card.signatures[0]) {
        return { status: 'unsigned', message: 'No signature in card' };
    }
    const sig = card.signatures[0];
    if (!window.crypto || !window.crypto.subtle || !window.crypto.subtle.importKey) {
        return { status: 'server-trusted', message: 'SubtleCrypto unavailable' };
    }
    // Best-effort: try to import an Ed25519 public key. Many browsers still
    // don't expose Ed25519 in SubtleCrypto; treat ImportKey rejection as a
    // signal to fall back to "server-trusted".
    try {
        let jwksResp;
        try {
            jwksResp = await fetch(`/agents/${encodeURIComponent(instanceId)}/.well-known/jwks.json`);
        } catch (e) {
            return { status: 'server-trusted', message: `JWKS fetch failed: ${e.message}` };
        }
        if (!jwksResp.ok) {
            return { status: 'server-trusted', message: `JWKS HTTP ${jwksResp.status}` };
        }
        const jwks = await jwksResp.json();
        const kid = sig.header && sig.header.kid;
        const jwk = (jwks.keys || []).find(k => !kid || k.kid === kid) || (jwks.keys || [])[0];
        if (!jwk) {
            return { status: 'server-trusted', message: 'No matching JWK' };
        }
        // Attempt Ed25519 import. If unsupported, exception bubbles to catch.
        let key;
        try {
            key = await window.crypto.subtle.importKey(
                'jwk', jwk, { name: 'Ed25519' }, false, ['verify']
            );
        } catch (_) {
            return { status: 'server-trusted', message: 'Ed25519 not supported in this browser' };
        }
        // JWS compact: header.payload.signature (all base64url).
        const compact = sig.signature || '';
        const parts = compact.split('.');
        if (parts.length !== 3) {
            return { status: 'failed', message: 'Malformed JWS compact serialization' };
        }
        const b64urlDecode = (s) => {
            const pad = s.length % 4 === 0 ? '' : '='.repeat(4 - (s.length % 4));
            const b = (s + pad).replace(/-/g, '+').replace(/_/g, '/');
            const bin = atob(b);
            const out = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
            return out;
        };
        const signingInput = new TextEncoder().encode(parts[0] + '.' + parts[1]);
        const signature = b64urlDecode(parts[2]);
        const ok = await window.crypto.subtle.verify(
            { name: 'Ed25519' }, key, signature, signingInput
        );
        return ok
            ? { status: 'verified', message: 'Ed25519 signature verified' }
            : { status: 'failed', message: 'Ed25519 verification failed' };
    } catch (e) {
        return { status: 'server-trusted', message: `Verify error: ${e.message}` };
    }
}

async function renderAgentCardPanel(instanceId, container) {
    let card;
    try {
        const resp = await fetch(
            `/agents/${encodeURIComponent(instanceId)}/.well-known/agent-card.json`
        );
        if (!resp.ok) {
            container.innerHTML =
                `<h3>A2A Identity</h3>` +
                `<p class="agentcard-error">No AgentCard available (HTTP ${resp.status})</p>`;
            return;
        }
        card = await resp.json();
    } catch (e) {
        container.innerHTML =
            `<h3>A2A Identity</h3>` +
            `<p class="agentcard-error">Failed to fetch AgentCard: ${escAttr(e.message)}</p>`;
        return;
    }

    const sigInfo = await verifyCardSignature(card, instanceId);
    const sigLabel = ({
        verified: '✓ verified',
        'server-trusted': 'ℹ server-trusted',
        unsigned: '⚠ unsigned',
        failed: '✗ failed',
    })[sigInfo.status] || sigInfo.status;

    const extensions = (card.capabilities && card.capabilities.extensions) || [];
    const extRows = extensions.map(ext => {
        const uri = ext.uri || '';
        const docHref = EXT_DOC_LINKS[uri];
        const uriCell = docHref
            ? `<a href="${escAttr(docHref)}" target="_blank" rel="noopener">${escAttr(uri)}</a>`
            : escAttr(uri);
        const required = ext.required
            ? `<span class="ext-required">yes</span>`
            : `<span class="ext-optional">no</span>`;
        const paramsSummary = escAttr(summarizeExtParams(uri, ext.params));
        return `<tr><td>${uriCell}</td><td>${required}</td><td>${paramsSummary}</td></tr>`;
    }).join('') || `<tr><td colspan="3" class="ext-empty">No extensions</td></tr>`;

    const interfaces = card.supportedInterfaces || [];
    const ifRows = interfaces.map(iface => {
        const url = escAttr(iface.url || '');
        const transport = escAttr(iface.transport || '');
        const version = escAttr(iface.version || iface.extension || '');
        return `<tr><td>${url}</td><td>${transport}</td><td>${version}</td></tr>`;
    }).join('') || `<tr><td colspan="3" class="ext-empty">No interfaces</td></tr>`;

    const rawJson = escAttr(JSON.stringify(card, null, 2));

    container.innerHTML = `
        <h3>A2A Identity</h3>
        <div class="agentcard-summary">
            <span class="card-name">${escAttr(card.name || '(unnamed)')}</span>
            <span class="card-version">v${escAttr(card.version || '?')}</span>
            <span class="signature-status" data-status="${escAttr(sigInfo.status)}" title="${escAttr(sigInfo.message)}">${sigLabel}</span>
        </div>
        <details class="card-extensions">
            <summary>Extensions (<span class="ext-count">${extensions.length}</span>)</summary>
            <table class="ext-table">
                <thead><tr><th>URI</th><th>Required</th><th>Params</th></tr></thead>
                <tbody>${extRows}</tbody>
            </table>
        </details>
        <details class="card-interfaces">
            <summary>Supported interfaces (<span class="if-count">${interfaces.length}</span>)</summary>
            <table class="if-table">
                <thead><tr><th>URL</th><th>Transport</th><th>Version / Ext</th></tr></thead>
                <tbody>${ifRows}</tbody>
            </table>
        </details>
        <details class="card-raw">
            <summary>Raw card JSON</summary>
            <pre class="card-json">${rawJson}</pre>
        </details>
    `;
}

if (typeof window !== 'undefined') {
    window.renderAgentCardPanel = renderAgentCardPanel;
}
// === end #245 ===

// === #628/#629 Sessions & structured-output panel ===
// Surfaces an agent's sessions with their delivery capabilities
// (chat_source, backend/class, screen), a read-only SSE Chat viewer over
// /api/v1/agent-output/chat (#600), and a transcript view. Mirrors the
// AgentCard panel pattern: a self-contained render function populated
// asynchronously into a placeholder section in the agent detail modal.

// The live chat EventSource, tracked so it can be closed when the viewer is
// replaced or the detail modal closes (avoids leaked SSE connections).
let activeSessionsStream = null;
function closeActiveSessionsStream() {
    if (activeSessionsStream) {
        try { activeSessionsStream.close(); } catch (_) {}
        activeSessionsStream = null;
    }
}

async function renderSessionsPanel(agentId, container) {
    try {
        const resp = await fetch(`/api/v1/agents/${encodeURIComponent(agentId)}/sessions`);
        if (!resp.ok) {
            container.innerHTML =
                `<h3>Sessions &amp; Output</h3><p class="sessions-error">No sessions (HTTP ${resp.status})</p>`;
            return;
        }
        const data = await resp.json();
        const sessions = (data && data.sessions) || [];
        if (sessions.length === 0) {
            container.innerHTML =
                `<h3>Sessions &amp; Output</h3><p class="sessions-empty">No active sessions.</p>`;
            return;
        }
        container.innerHTML = `
            <h3>Sessions &amp; Output</h3>
            <div class="sessions-list">${sessions.map(renderSessionRow).join('')}</div>
            <div class="session-viewer" hidden></div>
        `;
        wireSessionPanelActions(container);
    } catch (e) {
        container.innerHTML =
            `<h3>Sessions &amp; Output</h3><p class="sessions-error">Failed to load sessions: ${escAttr(e.message)}</p>`;
    }
}

function renderSessionRow(s) {
    const chatSource = s.chat_source || 'none';
    const chatAvail = chatSource !== 'none';
    const badges = [
        `<span class="sess-badge sess-chat sess-chat-${cssToken(chatSource)}" title="Structured chat source (#600)">chat: ${escAttr(chatSource)}</span>`,
        `<span class="sess-badge" title="Session backend">${escAttr(s.session_backend || '?')}</span>`,
        `<span class="sess-badge" title="Session class">${escAttr(s.session_class || '?')}</span>`,
        s.has_screen ? `<span class="sess-badge sess-screen" title="Screen snapshot available">screen</span>` : '',
    ].join('');
    const chatBtn = chatAvail
        ? `<button class="sess-btn sess-chat-btn" data-url="${escAttr(s.chat_stream_url || '')}" data-name="${escAttr(s.session_name || s.session_id)}">Chat</button>`
        : '';
    return `
      <div class="sess-row">
        <div class="sess-head">
          <span class="sess-name">${escAttr(s.session_name || s.session_id)}</span>
          ${badges}
        </div>
        <div class="sess-actions">
          ${chatBtn}
          <button class="sess-btn sess-transcript-btn" data-sid="${escAttr(s.session_id)}" data-name="${escAttr(s.session_name || s.session_id)}">Transcript</button>
          ${s.has_screen ? `<button class="sess-btn sess-screen-btn" data-sid="${escAttr(s.session_id)}" data-name="${escAttr(s.session_name || s.session_id)}">Screen</button>` : ''}
        </div>
      </div>`;
}

function wireSessionPanelActions(container) {
    const viewer = container.querySelector('.session-viewer');
    container.querySelectorAll('.sess-chat-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            openChatViewer(viewer, btn.dataset.url, btn.dataset.name);
        });
    });
    container.querySelectorAll('.sess-transcript-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            openTranscript(viewer, btn.dataset.sid, btn.dataset.name);
        });
    });
    container.querySelectorAll('.sess-screen-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            openScreen(viewer, btn.dataset.sid, btn.dataset.name);
        });
    });
}

function openChatViewer(viewer, url, name) {
    closeActiveSessionsStream();
    if (!url) return;
    viewer.hidden = false;
    viewer.innerHTML = `
        <div class="viewer-head">
            <span class="viewer-title">Chat — ${escAttr(name)}</span>
            <button class="viewer-close" title="Close stream" aria-label="Close chat stream">✕</button>
        </div>
        <div class="chat-log" aria-live="polite"></div>
    `;
    const log = viewer.querySelector('.chat-log');
    viewer.querySelector('.viewer-close').addEventListener('click', () => {
        closeActiveSessionsStream();
        viewer.hidden = true;
        viewer.innerHTML = '';
    });

    // EventSource auto-sends Last-Event-ID on reconnect; our backend resumes
    // after the {session}-{seq} cursor (#600), so drops are recovered.
    const es = new EventSource(url);
    activeSessionsStream = es;
    const KINDS = ['delta', 'tool_call', 'tool_result', 'status', 'done', 'error', 'raw'];
    for (const kind of KINDS) {
        es.addEventListener(kind, ev => {
            appendChatFrame(log, kind, ev.data);
            if (kind === 'done' || kind === 'error') closeActiveSessionsStream();
        });
    }
    es.onerror = () => {
        // Transient network errors auto-reconnect; surface a subtle marker.
        appendChatLine(log, 'chat-line chat-neterr', '· stream reconnecting…');
    };
}

function appendChatFrame(log, kind, dataStr) {
    let d = {};
    try { d = JSON.parse(dataStr); } catch (_) { d = { content: dataStr }; }
    switch (kind) {
        case 'delta':
            appendChatLine(log, 'chat-line chat-assistant', d.content || '');
            break;
        case 'tool_call':
            appendChatLine(log, 'chat-line chat-tool-call',
                `→ ${d.name || 'tool'}(${compactJson(d.input)})`);
            break;
        case 'tool_result':
            appendChatLine(log, `chat-line chat-tool-result${d.status === 'error' ? ' chat-err' : ''}`,
                `← [${d.status || 'ok'}] ${d.content || ''}`);
            break;
        case 'status':
            appendChatLine(log, 'chat-line chat-status', `· ${d.status || ''} ${d.content || ''}`.trim());
            break;
        case 'done':
            appendChatLine(log, 'chat-line chat-done',
                `✓ done (${d.finish_reason || 'stop'}${d.model ? ', ' + d.model : ''})`);
            break;
        case 'error':
            appendChatLine(log, 'chat-line chat-err', `✗ ${d.error || 'error'} [${d.code || ''}]`);
            break;
        case 'raw':
            appendChatLine(log, 'chat-line chat-raw', d.content || '');
            break;
    }
}

function appendChatLine(log, cls, text) {
    const el = document.createElement('div');
    el.className = cls;
    el.textContent = text;
    log.appendChild(el);
    log.scrollTop = log.scrollHeight;
}

function compactJson(v) {
    if (v == null) return '';
    let s;
    try { s = JSON.stringify(v); } catch (_) { s = String(v); }
    return s.length > 120 ? s.slice(0, 117) + '…' : s;
}

async function openTranscript(viewer, sessionId, name) {
    closeActiveSessionsStream();
    viewer.hidden = false;
    viewer.innerHTML = `
        <div class="viewer-head">
            <span class="viewer-title">Transcript — ${escAttr(name)}</span>
            <button class="viewer-close" title="Close" aria-label="Close transcript">✕</button>
        </div>
        <div class="transcript-log">Loading…</div>
    `;
    viewer.querySelector('.viewer-close').addEventListener('click', () => {
        viewer.hidden = true;
        viewer.innerHTML = '';
    });
    const logEl = viewer.querySelector('.transcript-log');
    try {
        const resp = await fetch(
            `/api/v1/sessions/${encodeURIComponent(sessionId)}/transcript?limit=200`);
        if (!resp.ok) {
            logEl.textContent = `No transcript (HTTP ${resp.status})`;
            return;
        }
        const data = await resp.json();
        const items = (data && data.items) || [];
        if (items.length === 0) { logEl.textContent = '(empty)'; return; }
        logEl.innerHTML = items.map(it =>
            `<div class="transcript-line transcript-${cssToken(it.stream || 'stdout')}">` +
            `<span class="transcript-seq">${escAttr(it.seq)}</span>` +
            `<span class="transcript-text">${escAttr(it.text || '')}</span></div>`
        ).join('');
    } catch (e) {
        logEl.textContent = `Failed to load transcript: ${e.message}`;
    }
}

async function openScreen(viewer, sessionId, name) {
    closeActiveSessionsStream();
    viewer.hidden = false;
    viewer.innerHTML = `
        <div class="viewer-head">
            <span class="viewer-title">Screen — ${escAttr(name)}</span>
            <button class="viewer-close" title="Close" aria-label="Close screen view">✕</button>
        </div>
        <div class="screen-meta"></div>
        <div class="transcript-log screen-log">Loading…</div>
    `;
    viewer.querySelector('.viewer-close').addEventListener('click', () => {
        viewer.hidden = true;
        viewer.innerHTML = '';
    });
    const logEl = viewer.querySelector('.screen-log');
    const metaEl = viewer.querySelector('.screen-meta');
    try {
        const resp = await fetch(
            `/api/v1/sessions/${encodeURIComponent(sessionId)}/screen`);
        if (!resp.ok) {
            logEl.textContent = `No screen state (HTTP ${resp.status})`;
            return;
        }
        const d = await resp.json();
        metaEl.textContent =
            `${d.rows || '?'}×${d.cols || '?'}` +
            (d.prompt_detected ? ` · prompt: ${d.prompt_text || 'detected'}` : '');
        logEl.textContent = d.text || '(blank screen)';
    } catch (e) {
        logEl.textContent = `Failed to load screen: ${e.message}`;
    }
}

if (typeof window !== 'undefined') {
    window.renderSessionsPanel = renderSessionsPanel;
    window.closeActiveSessionsStream = closeActiveSessionsStream;
}
// === end #628/#629 ===

const OAUTH_PATTERNS = [
    /https:\/\/[a-z0-9.-]*\.anthropic\.com\/[^\s"'<>]+/gi,
    /https:\/\/console\.anthropic\.com\/[^\s"'<>]+/gi,
    /https:\/\/github\.com\/login\/oauth\/authorize\?[^\s"'<>]+/gi,
    /https:\/\/github\.com\/login\/device[^\s"'<>]*/gi,
    /https:\/\/accounts\.google\.com\/o\/oauth2\/[^\s"'<>]+/gi,
    /https:\/\/login\.microsoftonline\.com\/[^\s"'<>]+/gi,
    /(?:open|visit|go to|navigate to|click|authorize at)[:\s]+["']?(https?:\/\/[^\s"'<>]+)/gi,
    /(?:please|now)\s+(?:open|visit|go to)[:\s]+["']?(https?:\/\/[^\s"'<>]+)/gi,
];

const DEVICE_CODE_PATTERNS = [
    /enter(?:ing)?\s+(?:the\s+)?code[:\s]+([A-Z0-9]{4,}-?[A-Z0-9]{4,})/gi,
    /user\s*code[:\s]+([A-Z0-9]{4,}-?[A-Z0-9]{4,})/gi,
];

class AgenticDashboard {
    constructor() {
        this.ws = null;
        this.agents = new Map();         // agentId -> agent info
        this.panes = new Map();          // agentId -> DOM elements
        this.activeCommandIds = new Map(); // agentId -> last command_id
        this.shellCommandIds = new Map();  // agentId -> shell session command_id
        this.pendingFirstOutput = new Set(); // command_ids awaiting first output (for resize-on-first-output)
        this.pendingStartupAttach = new Set(); // agentIds awaiting list_sessions response before attach
        this.sessionIdToAgentId = new Map();   // session_id -> agentId (for session_frame routing)
        this.lastSeqPerSession = new Map();     // session_id -> last received seq (for incremental replay)
        this.reconnectAttempts = 0;
        this.reconnectDelay = 1000;
        this.maxReconnectDelay = 30000;
        this.reconnectTimer = null;
        this.connectionAttempt = 0;
        this.helloTimer = null;
        this.intentionalWsClose = false;
        this.terminalWsFailure = null;
        this.connectionState = 'disconnected';
        this.serverCapabilities = null;
        this.advertisedManagementWsUrl = null;
        this.maxWsBufferedAmount = 512 * 1024;
        this.currentOAuthPrompt = null;

        // Log sidebar state
        this.logEvents = [];
        this.systemLogs = [];            // System log messages
        this.maxLogEvents = 100;  // Limit UI to 100 events
        this.maxSystemLogs = 200;
        this.eventFilter = 'all';
        this.eventLevelFilter = 'all';
        this.systemLevelFilter = 'all';
        this.systemTargetFilter = 'all';
        this._knownEventTypes = new Set();  // observed event_types for dropdown
        this._knownTargets = new Set();     // observed log targets for dropdown
        this.autoScroll = true;
        this.lastEventId = 0;  // For change detection
        this.eventStreamState = 'unknown';
        this.lastSystemLogId = 0;
        this.activityNextCursor = null;
        this.activityQuery = null;
        this.activityScope = null;

        // Canonical resource state is owned by the build-free module boundary.
        this.resourceState = new window.ManagementUI.ResourceState();
        this.resourceState.resources.set('instances', new Map());
        this.instances = this.resourceState.resources.get('instances');
        this.operations = this.resourceState.operations;
        this.instanceInventoryObservedAt = null;
        this.instanceMutationIntents = new Map();
        this.fleetWorkloads = new Map();
        this.fleetInventoryRevision = null;
        this.reviewedFleetReconcile = null;
        this.celldCapabilities = new Set();
        this.reviewedCelldCommand = null;
        this.reviewedCelldReconcile = null;
        this.celldHistory = [];
        this.startupProfiles = new Map();
        this.selectedStartupProfileId = null;
        this.reviewedStartupProfile = null;
        this.configLoadouts = [];
        this.reviewedConfigLoadout = null;
        this.storageObjectExists = false;
        this.reviewedStorageWrite = null;
        this.accelerationProviders = new Map();
        this.reviewedAcceleration = null;
        this.accessAuthority = null;
        this.accessCredentials = new Map();
        this.accessCredentialLeases = new Map();
        this.accessSshLeases = new Map();
        this.accessAuditEvents = new Map();
        this.reviewedCredentialLease = null;
        this.reviewedSshLease = null;
        this.runtimeAvailability = new Map(); // runtime id -> additive discovery descriptor
        this.bootstrapReadiness = null;

        // Selected agent for single-pane display
        this.selectedAgent = null;

        // Sessions blade state
        this.selectedVmForSessions = null; // Which VM's sessions are shown
        this.vmSessions = new Map(); // vmName -> sessions array
        this.lastSelectedSession = new Map(); // vmName -> last selected session command_id

        // Per-session output buffers for live thumbnails
        // command_id -> { lines: string[], dirty: bool }
        this.sessionBuffers = new Map();
        this.maxSessionBufferLines = 50;

        // command_ids whose PTY chunks arrive on the formal SessionFrame path
        // already. The legacy `output` message for these is suppressed at the
        // terminal write layer to avoid double-rendering when a client is
        // simultaneously legacy-subscribed and formally joined.
        this.formallyJoinedCommandIds = new Set();

        // Loadout profiles cache
        this.loadouts = [];
        this.loadoutsLoaded = false;

        this.init();
    }

    init() {
        this.setupModalAccessibility();
        this.setupGlobalListeners();
        this.setupLogSidebar();
        this.setupBladeNav();
        this.setupManagementWorkspaces();
        this.connect();
        this.fetchAgents();
        this.fetchEvents().then(() => this.startEventStream());
        this.restoreTrackedOperations();
        this.fetchInstances();
        this.fetchRuntimeAvailability();
        this.fetchLoadouts();
        this.fetchLoadoutRegistry();
        this.fetchSystemLogs().then(() => this.startSystemLogStream());

        // Refresh session thumbnails every second
        setInterval(() => this.updateSessionThumbs(), 1000);

        // Poll AIWG serve connection status every 5 s
        this.pollAiwgStatus();
        setInterval(() => this.pollAiwgStatus(), 5000);

        // Reconnect button
        document.getElementById('aiwg-reconnect-btn')?.addEventListener('click', () => this.triggerAiwgReconnect());
        document.getElementById('connection-retry')?.addEventListener('click', () => this.retryManagementConnection());
        document.getElementById('reconcile-operations')?.addEventListener('click', () => this.reconcileInstancesAndOperations());
        window.addEventListener('online', () => this.resumeManagementConnection());
        window.addEventListener('offline', () => {
            if (!this.intentionalWsClose && !this.terminalWsFailure) this.setConnectionState('degraded');
        });
        document.addEventListener('visibilitychange', () => this.resumeManagementConnection());
    }

    // =========================================================================
    // WebSocket
    // =========================================================================

    managementWsUrl() {
        const configured = (typeof window !== 'undefined' && window.AGENTIC_MANAGEMENT_WS_URL)
            || document.querySelector('meta[name="agentic-management-ws-url"]')?.content
            || this.advertisedManagementWsUrl;
        return window.ManagementUI.resolveWebSocketUrl(window.location.href, configured);
    }

    async discoverManagementWsUrl() {
        if ((typeof window !== 'undefined' && window.AGENTIC_MANAGEMENT_WS_URL)
            || document.querySelector('meta[name="agentic-management-ws-url"]')?.content
            || this.advertisedManagementWsUrl) return;
        try {
            const readiness = await this.managementJson('/api/v2/admin/bootstrap/readiness', {
                owner: 'management-ws-discovery',
            });
            const endpoint = readiness?.management_websocket?.endpoint;
            if (typeof endpoint === 'string' && endpoint.trim()) {
                this.advertisedManagementWsUrl = endpoint;
            }
        } catch (error) {
            console.warn('Management WebSocket discovery unavailable; using same-origin endpoint', error);
        }
    }

    setConnectionState(state) {
        this.connectionState = state;
        this.updateConnectionStatus(state);
        for (const terminal of document.querySelectorAll('.xterm-wrapper:not([data-transport="pty-v2"])')) {
            terminal.dataset.connection = state;
            _ptyV2RefreshTerminalLabel(terminal);
        }
    }

    async connect() {
        const attempt = ++this.connectionAttempt;
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }
        this.intentionalWsClose = false;
        this.terminalWsFailure = null;
        this.serverCapabilities = null;
        this.setConnectionState('connecting');

        try {
            await this.discoverManagementWsUrl();
            if (attempt !== this.connectionAttempt || this.intentionalWsClose) return;
            const wsUrl = this.managementWsUrl();
            console.log(`Connecting to WebSocket at ${wsUrl}`);
            this.ws = new WebSocket(wsUrl);
            this.ws.onopen = () => this.onOpen();
            this.ws.onmessage = (e) => this.onMessage(e);
            this.ws.onclose = (e) => this.onClose(e);
            this.ws.onerror = (e) => console.error('WebSocket error:', e);
        } catch (error) {
            if (attempt !== this.connectionAttempt) return;
            console.error('WebSocket connection failed:', error);
            this.terminalWsFailure = `Management WebSocket configuration is invalid: ${error.message}`;
            this.setConnectionState('terminal');
            this.showToast(this.terminalWsFailure, 'error');
        }
    }

    onOpen() {
        console.log('WebSocket transport open; awaiting server_hello');
        this.setConnectionState('negotiating');
        // Clear stale PTY state — server's in-memory command registry resets on restart.
        // Existing panes will rediscover sessions via list_sessions when the agent list arrives.
        this.shellCommandIds.clear();
        this.activeCommandIds.clear();
        this.pendingFirstOutput.clear();
        this.pendingStartupAttach.clear();
        clearTimeout(this.helloTimer);
        this.helloTimer = setTimeout(() => {
            if (this.connectionState !== 'negotiating') return;
            this.failManagementProtocol(
                'Management connection did not negotiate capabilities. Verify server protocol support, then retry.',
                'server_hello timeout',
            );
        }, 5000);
    }

    onMessage(event) {
        try {
            const msg = JSON.parse(event.data);
            this.handleMessage(msg);
        } catch (e) {
            console.error('Failed to parse message:', e);
            if (this.connectionState === 'negotiating') {
                this.failManagementProtocol(
                    'Management connection sent an invalid capability frame. Update the dashboard or server, then retry.',
                    'invalid server_hello',
                );
            }
        }
    }

    onClose(event) {
        console.log('WebSocket closed:', event.code, event.reason || '');
        clearTimeout(this.helloTimer);
        this.helloTimer = null;
        this.serverCapabilities = null;
        if (this.terminalWsFailure || [1002, 1008].includes(event.code)) {
            const detail = this.terminalWsFailure || this.describeWsClose(event);
            this.terminalWsFailure = detail;
            this.setConnectionState('terminal');
            this.showToast(detail, 'error');
            return;
        }
        this.setConnectionState(this.intentionalWsClose ? 'disconnected' : 'reconnecting');
        if (!this.intentionalWsClose) {
            const detail = this.describeWsClose(event);
            if (event.code && event.code !== 1000) this.showToast(detail, 'error');
            this.scheduleReconnect();
        }
    }

    scheduleReconnect() {
        if (this.reconnectTimer || this.intentionalWsClose || this.terminalWsFailure) return;
        if ((typeof navigator !== 'undefined' && navigator.onLine === false)
            || (typeof document !== 'undefined' && document.hidden)) {
            this.setConnectionState('degraded');
            return;
        }
        const base = Math.min(
            this.maxReconnectDelay,
            this.reconnectDelay * Math.pow(2, this.reconnectAttempts)
        );
        const delay = Math.round(base * (0.75 + Math.random() * 0.5));
        this.reconnectAttempts++;
        console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);
        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = null;
            if ((typeof navigator !== 'undefined' && navigator.onLine === false)
                || (typeof document !== 'undefined' && document.hidden)) {
                this.scheduleReconnect();
                return;
            }
            this.connect();
        }, delay);
    }

    send(msg) {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false;
        if (this.connectionState !== 'ready') return false;
        const supported = this.serverCapabilities?.supportedClientMessages;
        if (supported && !supported.has(msg.type)) {
            this.showToast(`Server does not advertise WebSocket operation: ${msg.type}`, 'error');
            return false;
        }
        if (this.ws.bufferedAmount > this.maxWsBufferedAmount) {
            this.setConnectionState('degraded');
            this.showToast('Management connection is congested; command was not sent.', 'error');
            return false;
        }
        this.ws.send(JSON.stringify(msg));
        return true;
    }

    describeWsClose(event) {
        const labels = {
            1000: 'Management connection closed normally.',
            1001: 'Management server is going away; reconnecting.',
            1002: 'Management protocol mismatch. Update the dashboard or server, then retry.',
            1008: 'Management connection was rejected by policy. Verify operator authority before retrying.',
            1011: 'Management server encountered an internal error; reconnecting.',
            1013: 'Management server is temporarily overloaded; reconnecting.',
        };
        return event?.reason || labels[event?.code]
            || `Management transport closed (${event?.code || 'no close code'}); reconnecting.`;
    }

    failManagementProtocol(message, reason) {
        this.terminalWsFailure = message;
        this.setConnectionState('terminal');
        this.showToast(message, 'error');
        try { this.ws?.close(4002, reason); } catch (_) {}
    }

    retryManagementConnection() {
        const previous = this.ws;
        if (previous) {
            previous.onopen = null;
            previous.onmessage = null;
            previous.onerror = null;
            previous.onclose = null;
            try { previous.close(1000, 'manual retry'); } catch (_) {}
        }
        this.ws = null;
        this.terminalWsFailure = null;
        this.intentionalWsClose = false;
        this.reconnectAttempts = 0;
        this.connect();
    }

    resumeManagementConnection() {
        if (this.intentionalWsClose || this.terminalWsFailure) return;
        if (typeof navigator !== 'undefined' && navigator.onLine === false) return;
        if (typeof document !== 'undefined' && document.hidden) return;
        if (['degraded', 'reconnecting', 'disconnected'].includes(this.connectionState)) {
            this.scheduleReconnect();
        }
    }

    // =========================================================================
    // Message dispatch
    // =========================================================================

    handleMessage(msg) {
        if (this.connectionState !== 'ready' && msg?.type !== 'server_hello') {
            this.failManagementProtocol(
                'Management connection sent data before capability negotiation completed.',
                'frame before server_hello',
            );
            return;
        }
        switch (msg.type) {
            case 'server_hello':
                this.handleServerHello(msg);
                break;
            case 'output':
                this.handleOutput(msg);
                break;
            case 'agent_list':
                this.handleAgentList(msg);
                break;
            case 'metrics_update':
                this.handleMetricsUpdate(msg);
                break;
            case 'command_started':
                this.activeCommandIds.set(msg.agent_id, msg.command_id);
                break;
            case 'shell_started':
                this.handleShellStarted(msg);
                break;
            case 'subscribed':
            case 'unsubscribed':
            case 'pong':
                break;
            case 'input_sent':
                break;
            case 'vm_event':
                this.handleVmEvent(msg);
                break;
            case 'system_log':
                this.handleSystemLog(msg);
                break;
            case 'session_list':
                this.handleSessionsList(msg);
                break;
            case 'session_attached':
                // Server confirmed legacy attach — update command_id and session label
                if (msg.command_id && msg.agent_id) {
                    const entry = this.panes.get(msg.agent_id);
                    if (entry) {
                        entry.attachedSession = msg.command_id;
                        this.shellCommandIds.set(msg.agent_id, msg.command_id);
                        this.activeCommandIds.set(msg.agent_id, msg.command_id);
                        this.updatePaneSessionLabel(msg.agent_id, msg.session_name);
                        this.updateShellButton(msg.agent_id, true);
                    }
                }
                break;
            case 'session_joined':
                // Formal join confirmed — server will stream session_frame messages
                this.handleSessionJoined(msg);
                break;
            case 'session_frame':
                // Streamed frame from a joined session (output, resize, closed, etc.)
                this.handleSessionFrame(msg);
                break;
            case 'session_detached':
                break;
            case 'session_created':
                this.handleSessionCreated(msg);
                break;
            case 'session_killed':
                this.showToast(`Session ${msg.session_name || msg.session_id?.slice(0, 8)} killed`, 'success');
                // Drop persisted seq so a future attach with the same id
                // (e.g. after server restart with the same UUID) doesn't
                // skip frames we never saw (#144).
                if (msg.session_id) this.forgetLastSeq(msg.session_id);
                // Refresh sessions blade if showing this agent
                if (msg.agent_id && this.selectedVmForSessions === msg.agent_id) {
                    this.fetchSessionsForBlade(msg.agent_id);
                }
                break;
            case 'reconciliation_triggered':
                this.showToast(`Reconciliation started for ${msg.agent_id}`, 'success');
                break;
            case 'error':
                console.error('Server error:', msg.message);
                this.showToast(msg.message, 'error');
                break;
            default:
                console.log('Unknown message:', msg.type, msg);
        }
    }

    handleServerHello(msg) {
        if (this.connectionState !== 'negotiating') {
            this.failManagementProtocol(
                'Unexpected management capability frame. Update the dashboard or server, then retry.',
                'unexpected server_hello',
            );
            return;
        }
        const messages = Array.isArray(msg.supported_client_messages)
            ? msg.supported_client_messages.filter(value => typeof value === 'string')
            : null;
        const features = Array.isArray(msg.features)
            ? msg.features.filter(value => typeof value === 'string')
            : null;
        const protocolMajor = String(msg.protocol_version || '').split('.')[0];
        if (protocolMajor !== '1' || !messages || !features || !messages.includes('list_agents')) {
            this.failManagementProtocol(
                `Management capability advertisement is incompatible (protocol ${msg.protocol_version || 'missing'}).`,
                'incompatible server_hello',
            );
            return;
        }
        clearTimeout(this.helloTimer);
        this.helloTimer = null;
        this.serverCapabilities = {
            serverVersion: msg.server_version || null,
            protocolVersion: msg.protocol_version,
            supportedClientMessages: new Set(messages),
            features: new Set(features),
        };
        this.reconnectAttempts = 0;
        this.setConnectionState('ready');
        // Inventory is scoped and contains no terminal bytes. Session output
        // attaches separately through join_session / pty-ws.v1.
        this.send({ type: 'list_agents' });
    }

    handleOutput(msg) {
        // Always buffer output per command_id for session thumbnails and replay
        if (msg.command_id) {
            let buf = this.sessionBuffers.get(msg.command_id);
            if (!buf) {
                buf = { text: '', raw: '', dirty: true };
                this.sessionBuffers.set(msg.command_id, buf);
            }
            // Store raw output for replay (keep last ~32KB)
            buf.raw += msg.data;
            if (buf.raw.length > 32768) {
                buf.raw = buf.raw.slice(-32768);
            }
            // Accumulate stripped text for thumbnails
            buf.text += this.stripAnsi(msg.data);
            // Limit buffer size (keep last ~4KB)
            if (buf.text.length > 4096) {
                buf.text = buf.text.slice(-4096);
            }
            buf.dirty = true;
        }

        // Only write to main terminal if this is the attached session (or default shell)
        const entry = this.panes.get(msg.agent_id);
        const attachedId = entry?.attachedSession;
        const shellId = this.shellCommandIds.get(msg.agent_id);

        // Show in main terminal if:
        //  - No explicit session attached and this is the shell session, OR
        //  - This command_id matches the attached session
        // Also: if this command_id is being delivered via the formal
        // SessionFrame path, skip the legacy write — otherwise the same
        // chunk renders twice when a client is both legacy-subscribed and
        // formally joined to the session.
        const formallyJoined = this.formallyJoinedCommandIds.has(msg.command_id);
        if (!formallyJoined &&
            ((!attachedId && msg.command_id === shellId) || msg.command_id === attachedId)) {
            this.appendToPane(msg.agent_id, msg.stream, msg.data, msg.ts);
        }

        // On first output from a freshly started shell, send a follow-up resize.
        // This handles the case where tmux took longer than the 600ms timer to attach
        // and didn't receive the initial resize (blank terminal symptom).
        if (this.pendingFirstOutput.has(msg.command_id)) {
            this.pendingFirstOutput.delete(msg.command_id);
            if (entry && entry.term && entry.fitAddon) {
                try { entry.fitAddon.fit(); } catch (_) {}
                this._sendPtyResize(msg.agent_id, msg.command_id, entry.term.cols, entry.term.rows);
            }
        }

        this.detectOAuth(msg.agent_id, msg.command_id, msg.data);
    }

    handleMetricsUpdate(msg) {
        const entry = this.panes.get(msg.agent_id);
        if (!entry) return;

        const cpuEl = entry.pane.querySelector('.stat-cpu .stat-value');
        const memEl = entry.pane.querySelector('.stat-mem .stat-value');
        const diskEl = entry.pane.querySelector('.stat-disk .stat-value');

        if (cpuEl) {
            const cpu = msg.cpu_percent;
            cpuEl.textContent = `${cpu.toFixed(0)}%`;
            cpuEl.parentElement.className = `stat stat-cpu ${this.statLevel(cpu)}`;
        }

        if (memEl && msg.memory_total_bytes > 0) {
            const memPct = (msg.memory_used_bytes / msg.memory_total_bytes) * 100;
            const memMB = Math.round(msg.memory_used_bytes / 1024 / 1024);
            const totalMB = Math.round(msg.memory_total_bytes / 1024 / 1024);
            memEl.textContent = `${memMB}/${totalMB}M`;
            memEl.parentElement.className = `stat stat-mem ${this.statLevel(memPct)}`;
        }

        if (diskEl && msg.disk_total_bytes > 0) {
            const diskPct = (msg.disk_used_bytes / msg.disk_total_bytes) * 100;
            const diskGB = (msg.disk_used_bytes / 1024 / 1024 / 1024).toFixed(1);
            const totalGB = (msg.disk_total_bytes / 1024 / 1024 / 1024).toFixed(0);
            diskEl.textContent = `${diskGB}/${totalGB}G`;
            diskEl.parentElement.className = `stat stat-disk ${this.statLevel(diskPct)}`;
        }

        // Store system info for tooltip
        if (msg.os || msg.cpu_cores) {
            const agent = this.agents.get(msg.agent_id);
            if (agent) {
                agent._sysinfo = {
                    os: msg.os, kernel: msg.kernel,
                    cpu_cores: msg.cpu_cores,
                    uptime: msg.uptime_seconds,
                    load_avg: msg.load_avg,
                };
            }
        }
    }

    statLevel(pct) {
        if (pct >= 85) return 'stat-critical';
        if (pct >= 60) return 'stat-warning';
        return 'stat-ok';
    }

    handleVmEvent(msg) {
        // Add event to log sidebar
        const event = {
            event_type: msg.event_type,
            vm_name: msg.vm_name,
            timestamp: msg.timestamp || new Date().toISOString(),
            details: msg.details || {},
        };
        this.addEvent(event);

        // Show toast for important events
        if (msg.event_type === 'vm.crashed') {
            this.showToast(`VM ${msg.vm_name} crashed!`, 'error');
        } else if (msg.event_type === 'vm.started') {
            this.showToast(`VM ${msg.vm_name} started`, 'success');
        }

        // Refresh VM list after events
        setTimeout(() => this.fetchVms(), 500);
    }

    handleAgentList(msg) {
        if (!msg.agents) return;

        const currentIds = new Set(this.agents.keys());
        const incomingIds = new Set(msg.agents.map(a => a.id));

        // Add or update agents
        msg.agents.forEach(agent => {
            this.agents.set(agent.id, agent);
            if (!this.panes.has(agent.id)) {
                this.createPane(agent);
            } else {
                this.updatePaneHeader(agent);
                // Pane exists but shell state was cleared (reconnect after server restart).
                // Rediscover sessions via list_sessions before attaching.
                const statusClass = (agent.status || '').toLowerCase();
                if (!this.shellCommandIds.has(agent.id) && !statusClass.includes('provisioning')) {
                    const entry = this.panes.get(agent.id);
                    if (entry && entry.term) {
                        this.discoverAndAttach(agent.id);
                    }
                }
            }
            // Populate metrics from REST API data (if present)
            if (agent.metrics) {
                this.handleMetricsUpdate({
                    agent_id: agent.id,
                    ...agent.metrics,
                });
            }
        });

        // Remove panes for disconnected agents
        for (const id of currentIds) {
            if (!incomingIds.has(id)) {
                this.agents.delete(id);
                this.removePane(id);
            }
        }

        this.updateAgentCount();
        this.updateEmptyState();
    }

    // =========================================================================
    // Shell management
    // =========================================================================

    startShell(agentId) {
        const entry = this.panes.get(agentId);
        if (!entry) return;

        const cols = entry.term.cols || 80;
        const rows = entry.term.rows || 24;

        console.log(`Starting shell on ${agentId} (${cols}x${rows})`);
        this.send({
            type: 'start_shell',
            agent_id: agentId,
            cols: cols,
            rows: rows,
        });
    }

    handleShellStarted(msg) {
        const { agent_id, command_id } = msg;
        this.shellCommandIds.set(agent_id, command_id);
        this.activeCommandIds.set(agent_id, command_id);
        // Mark as pending first output so we send a follow-up resize when tmux
        // actually starts writing — this handles slow attach cases reliably.
        this.pendingFirstOutput.add(command_id);
        console.log(`Shell started on ${agent_id}: ${command_id}`);

        // Focus the terminal and send resize after tmux has time to initialize
        const entry = this.panes.get(agent_id);
        if (entry && entry.term) {
            entry.term.focus();
            // Delay resize to give the agent time to exec tmux and attach.
            // 600ms covers the gRPC round-trip + tmux exec under normal load.
            setTimeout(() => {
                try { entry.fitAddon.fit(); } catch (_) {}
                this._sendPtyResize(agent_id, command_id, entry.term.cols, entry.term.rows);
            }, 600);
        }

        // Update shell button and session label
        this.updateShellButton(agent_id, true);
        this.updatePaneSessionLabel(agent_id, 'main');
    }

    updateShellButton(agentId, active) {
        const entry = this.panes.get(agentId);
        if (!entry) return;
        const btn = entry.pane.querySelector('.pane-shell-btn');
        if (btn) {
            btn.classList.toggle('active', active);
        }
    }

    // =========================================================================
    // Pane management
    // =========================================================================

    createPane(agent) {
        console.log('createPane called for agent:', agent.id);
        const container = document.getElementById('pane-container');
        const pane = document.createElement('div');
        pane.className = 'agent-pane';
        pane.dataset.agentId = agent.id;

        // Auto-select first agent if none selected
        if (!this.selectedAgent) {
            this.selectedAgent = agent.id;
            console.log('Auto-selected agent:', agent.id);
        }

        // Hide pane if not the selected agent
        if (this.selectedAgent !== agent.id) {
            pane.style.display = 'none';
        }

        const statusClass = agent.status.toLowerCase().replace('agent_status_', '');

        pane.innerHTML = `
            <div class="pane-header">
                <div class="pane-header-left">
                    <span class="pane-status-dot ${statusClass}"></span>
                    <span class="pane-agent-name">${this.esc(agent.id)}</span>
                    <span class="pane-session-label" title="Active session"></span>
                    <span class="pane-agent-host">${this.esc(agent.hostname || agent.ip_address || '')}</span>
                    ${agent.loadout ? `<span class="pane-loadout-badge" title="Loadout: ${this.esc(agent.loadout)}">${this.esc(agent.loadout)}</span>` : ''}
                </div>
                <div class="pane-stats">
                    <span class="stat stat-cpu" title="CPU"><span class="stat-label">CPU</span> <span class="stat-value">--</span></span>
                    <span class="stat stat-mem" title="Memory"><span class="stat-label">MEM</span> <span class="stat-value">--</span></span>
                    <span class="stat stat-disk" title="Disk"><span class="stat-label">DSK</span> <span class="stat-value">--</span></span>
                </div>
                <div class="pane-controls">
                    <button class="pane-vm-btn pane-vm-restart" title="Restart VM (graceful reboot)" aria-label="Restart ${this.esc(agent.id)}" data-action="restart">&#10227;</button>
                    <button class="pane-vm-btn pane-vm-stop" title="Stop VM (graceful shutdown — restart from VM list)" aria-label="Stop ${this.esc(agent.id)}" data-action="stop">&#9208;</button>
                    <button class="pane-vm-btn pane-vm-kill" title="Force off (hard power off — VM stays defined)" aria-label="Force off ${this.esc(agent.id)}" data-action="force-off">&#9211;</button>
                    <button class="pane-shell-btn pane-resync-btn" title="Resync terminal — reset xterm state and re-attach (#180 escape hatch)" aria-label="Resync ${this.esc(agent.id)} terminal" data-action="resync">⟳</button>
                    <button class="pane-shell-btn" title="Reconnect to tmux session">Reconnect</button>
                </div>
            </div>
            <div class="pane-setup-progress" style="display:none">
                <div class="setup-progress-header">
                    <button type="button" class="setup-progress-icon" aria-label="Toggle provisioning terminal">&#9881;</button>
                    <span class="setup-progress-title">provisioning...</span>
                    <button class="peek-terminal-btn" title="Watch terminal during setup">&#9654; terminal</button>
                </div>
                <div class="setup-progress-steps"></div>
                <div class="setup-progress-hint">Setup in progress &mdash; <a class="peek-terminal-link" href="#">watch terminal</a> to observe</div>
            </div>
            <div class="pane-output"></div>
        `;

        const outputEl = pane.querySelector('.pane-output');
        // The pane has two .pane-shell-btn elements: the resync (⟳) escape
        // hatch and the legacy Reconnect button. Disambiguate by class.
        const resyncBtn = pane.querySelector('.pane-resync-btn');
        const shellBtn = pane.querySelector('.pane-shell-btn:not(.pane-resync-btn)');

        if (resyncBtn) {
            resyncBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                this.resyncPane(agent.id);
            });
        }

        // VM control buttons
        const restartBtn = pane.querySelector('.pane-vm-restart');
        const stopBtn = pane.querySelector('.pane-vm-stop');
        const killBtn = pane.querySelector('.pane-vm-kill');

        restartBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'restart'));
        stopBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'stop'));
        killBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'force-off'));

        // Gear icon, "terminal" button, or hint link -> toggle PTY peek during provisioning
        pane.addEventListener('click', (e) => {
            const target = e.target.closest('.setup-progress-icon, .peek-terminal-btn, .peek-terminal-link');
            if (!target) return;
            e.preventDefault();
            const entry = this.panes.get(agent.id);
            if (!entry) return;
            entry.peekMode = !entry.peekMode;
            this._applyPeekMode(agent.id, entry);
        });

        // Loadout badge -> detail modal
        const loadoutBadge = pane.querySelector('.pane-loadout-badge');
        if (loadoutBadge) {
            loadoutBadge.addEventListener('click', () => this.showAgentDetail(agent.id));
        }

        // Agent name -> detail modal
        const agentName = pane.querySelector('.pane-agent-name');
        if (agentName) {
            agentName.addEventListener('click', () => this.showAgentDetail(agent.id));
            agentName.style.cursor = 'pointer';
        }

        // Initialize xterm.js terminal — stdin enabled for PTY.
        // scrollback: 0 because tmux manages its own scrollback buffer.
        // This also eliminates the xterm scrollbar, giving FitAddon an
        // accurate column calculation (no scrollbar width to estimate).
        const term = new Terminal({
            cursorBlink: true,
            cursorStyle: 'block',
            disableStdin: false,
            convertEol: false,
            scrollback: 0,
            fontSize: 13,
            fontFamily: "'SF Mono', 'Fira Code', 'Consolas', monospace",
            theme: {
                background: '#0d0d1a',
                foreground: '#00ff88',
                cursor: '#00ff88',
                black: '#0d0d1a',
                red: '#ff4444',
                green: '#00ff88',
                yellow: '#ffaa00',
                blue: '#00d9ff',
                magenta: '#7b2cbf',
                cyan: '#00d9ff',
                white: '#e8e8e8',
            },
        });

        // Fit addon — auto-resize terminal to container
        const fitAddon = new FitAddon.FitAddon();
        term.loadAddon(fitAddon);

        container.appendChild(pane);

        // Wrapper div so FitAddon measures the inset area, not the full container.
        // Without this, tmux status bar overflows because FitAddon calculates
        // more columns than visually fit inside the padded region.
        const xtermWrapper = document.createElement('div');
        xtermWrapper.className = 'xterm-wrapper';
        xtermWrapper.setAttribute('role', 'region');
        xtermWrapper.setAttribute('aria-label', `${agent.id} terminal; connection connecting; role controller; interactive`);
        xtermWrapper.dataset.agentLabel = agent.id;
        xtermWrapper.dataset.transport = 'legacy';
        xtermWrapper.dataset.connection = 'connecting';
        xtermWrapper.dataset.role = 'controller';
        xtermWrapper.dataset.readonly = 'false';
        outputEl.appendChild(xtermWrapper);
        term.open(xtermWrapper);
        const terminalInput = xtermWrapper.querySelector('.xterm-helper-textarea');
        if (terminalInput) {
            terminalInput.setAttribute('aria-label', `${agent.id} terminal input`);
            terminalInput.setAttribute('aria-readonly', 'false');
        }
        const terminalExit = document.createElement('button');
        terminalExit.type = 'button';
        terminalExit.className = 'terminal-focus-exit';
        terminalExit.textContent = 'Leave terminal focus';
        terminalExit.setAttribute('aria-label', `Leave ${agent.id} terminal focus`);
        terminalExit.addEventListener('click', () => (resyncBtn || shellBtn).focus());
        outputEl.appendChild(terminalExit);

        // Fit after DOM insertion, then discover existing sessions or start shell
        const self = this;
        requestAnimationFrame(() => {
            try { fitAddon.fit(); } catch (_) {}
            self.discoverAndAttach(agent.id);
        });

        // Re-fit on container resize. Skip when the container is hidden /
        // zero-sized — fit() would compute degenerate dims and term.onResize
        // (below) would forward a junk resize to the PTY/tmux. The PTY
        // resize itself is plumbed via term.onResize, not from here, so we
        // have a single source of truth.
        const resizeObserver = new ResizeObserver((entries) => {
            const box = entries[0]?.contentRect;
            if (!box || box.width < 50 || box.height < 20) return;
            try { fitAddon.fit(); } catch (_) {}
        });
        resizeObserver.observe(xtermWrapper);

        // Ensure control keys (Ctrl+C, etc.) go to the PTY, not the browser.
        // Exception: allow browser Ctrl+C (copy) when text is selected.
        term.attachCustomKeyEventHandler((ev) => {
            if (ev.type !== 'keydown') return true;
            if (ev.ctrlKey && ev.shiftKey && ev.key === 'Escape') {
                terminalExit.focus();
                return false;
            }
            if (ev.ctrlKey && ev.key === 'c' && term.hasSelection()) {
                return false; // let browser copy selection
            }
            if (ev.ctrlKey && ev.key === 'v') {
                return false; // let browser paste
            }
            return true; // send everything else to PTY
        });

        // Forward terminal keystrokes to shell stdin
        term.onData((data) => {
            // Filter out terminal response sequences (DA1, DA2, cursor position reports, etc.)
            // These are responses to queries that shouldn't be sent as PTY input
            // Match with or without escape prefix (may be stripped or chunked)
            if (data.match(/^\x1b\[\??[\d;]*[cRn]$/) ||      // ESC [ sequences
                data.match(/^[\d;]+[cRn]$/) ||               // Response without ESC prefix (chunked)
                data.match(/^\x1b\].*\x07$/) ||              // OSC sequences
                data.match(/^\x1bP.*\x1b\\$/) ||             // DCS sequences
                data.match(/^\x1b[\[\]PO]/) ||               // Any escape sequence start
                data.match(/^[0-9;]+c$/)) {                  // Bare DA response like "0;276;0c"
                // Silently drop terminal response sequences
                return;
            }

            const shellCmdId = this.shellCommandIds.get(agent.id);
            if (shellCmdId) {
                this.send({
                    type: 'send_input',
                    agent_id: agent.id,
                    command_id: shellCmdId,
                    data: data,
                });
            }
        });

        // When xterm itself resizes (fitAddon, ResizeObserver, or any path),
        // re-assert the new dimensions to the server so tmux stays in sync.
        // Validation happens inside _sendPtyResize.
        term.onResize(({ cols, rows }) => {
            const shellCmdId = this.shellCommandIds.get(agent.id);
            this._sendPtyResize(agent.id, shellCmdId, cols, rows);
        });

        // Shell button — rediscover sessions and reattach (or start fresh if none running)
        shellBtn.addEventListener('click', () => {
            term.clear();
            term.reset();
            this.discoverAndAttach(agent.id);
        });

        this.panes.set(agent.id, { pane, output: outputEl, term, fitAddon, resizeObserver, peekMode: false });
        console.log('Pane created and stored for:', agent.id, 'Total panes:', this.panes.size, 'Keys:', [...this.panes.keys()]);
        // Shell auto-started in RAF callback above after fit completes
    }

    _applyPeekMode(agentId, entry) {
        const overlay = entry.pane.querySelector('.pane-setup-progress');
        const outputEl = entry.pane.querySelector('.pane-output');
        const gearIcon = entry.pane.querySelector('.setup-progress-icon');
        if (!overlay) return;
        if (entry.peekMode) {
            overlay.classList.add('peek-mode');
            if (outputEl) outputEl.style.display = '';
            if (gearIcon) gearIcon.classList.add('active');
            // Refit terminal now that it's visible
            if (entry.fitAddon) setTimeout(() => { try { entry.fitAddon.fit(); } catch(_) {} }, 50);
        } else {
            overlay.classList.remove('peek-mode');
            if (outputEl) outputEl.style.display = 'none';
            if (gearIcon) gearIcon.classList.remove('active');
        }
    }

    updatePaneHeader(agent) {
        const entry = this.panes.get(agent.id);
        if (!entry) return;
        const dot = entry.pane.querySelector('.pane-status-dot');
        const statusClass = agent.status.toLowerCase().replace('agent_status_', '');
        dot.className = `pane-status-dot ${statusClass}`;

        // Setup progress overlay
        const overlay = entry.pane.querySelector('.pane-setup-progress');
        const outputEl = entry.pane.querySelector('.pane-output');
        const shellBtn = entry.pane.querySelector('.pane-shell-btn');
        if (!overlay) return;

        if (statusClass === 'provisioning') {
            overlay.style.display = '';
            if (shellBtn) shellBtn.disabled = true;
            // Respect peek mode — terminal visibility controlled by _applyPeekMode
            if (!entry.peekMode && outputEl) outputEl.style.display = 'none';

            if (agent.setup_progress_json) {
                try {
                    const prog = JSON.parse(agent.setup_progress_json);
                    const steps = prog.steps || {};
                    const stepsHtml = Object.entries(steps).map(([name, state]) => {
                        const icon = state === 'done' ? '\u2713' :
                                     state === 'installing' ? '\u25CB' :
                                     state === 'failed' ? '\u2717' : '\u00B7';
                        const cls = state === 'done' ? 'done' :
                                    state === 'installing' ? 'active' :
                                    state === 'failed' ? 'failed' : 'pending';
                        return `<div class="setup-step ${cls}"><span class="setup-step-icon">${icon}</span> ${this.esc(name)}</div>`;
                    }).join('');
                    overlay.querySelector('.setup-progress-steps').innerHTML = stepsHtml;
                    overlay.querySelector('.setup-progress-title').textContent =
                        `provisioning: ${prog.current_step || '...'}`;
                } catch (_) {
                    overlay.querySelector('.setup-progress-title').textContent =
                        agent.setup_status || 'provisioning...';
                }
            } else if (agent.setup_status) {
                overlay.querySelector('.setup-progress-title').textContent = agent.setup_status;
            }
        } else {
            // Setup complete — clear peek mode and show terminal normally
            entry.peekMode = false;
            overlay.style.display = 'none';
            overlay.classList.remove('peek-mode');
            if (outputEl) outputEl.style.display = '';
            if (shellBtn) shellBtn.disabled = false;
        }
    }

    removePane(agentId) {
        // Close sessions blade if showing this agent
        if (this.selectedVmForSessions === agentId) {
            this.closeSessionsBlade();
        }
        this.vmSessions.delete(agentId);

        const entry = this.panes.get(agentId);
        if (entry) {
            if (entry.resizeObserver) entry.resizeObserver.disconnect();
            // #247: tear down any active v2 PTY client so the WS gets
            // closed cleanly with a `pty.leave_session` verb.
            if (entry.ptyV2Client && typeof entry.ptyV2Client.leave === 'function') {
                try { entry.ptyV2Client.leave(); } catch (_) {}
                entry.ptyV2Client = null;
            }
            if (entry.term) entry.term.dispose();
            entry.pane.remove();
            this.panes.delete(agentId);
        }
    }

    appendToPane(agentId, stream, data, timestamp) {
        let entry = this.panes.get(agentId);
        if (!entry) {
            // Agent not yet known — create a stub pane
            this.createPane({ id: agentId, status: 'ready', hostname: '' });
            entry = this.panes.get(agentId);
        }

        if (!entry.term) return;

        // For PTY shell output, write raw (PTY handles its own newlines/escapes)
        const shellCmdId = this.shellCommandIds.get(agentId);
        if (shellCmdId && stream === 'stdout') {
            entry.term.write(data);
            return;
        }

        // Non-PTY output: apply color prefix based on stream type
        let prefix = '';
        if (stream === 'stderr') {
            prefix = '\x1b[31m'; // red
        } else if (stream === 'log') {
            prefix = '\x1b[90m'; // dim gray
        }
        const reset = prefix ? '\x1b[0m' : '';

        const text = prefix + data + reset;
        entry.term.write(text);
    }

    // =========================================================================
    // VM Control
    // =========================================================================

    // Defer + de-duplicate a pty_resize. Two safeguards on top of the size
    // floor: (1) coalesce a burst of resize events into the last steady-state
    // value via setTimeout debounce, (2) require the measurement to settle
    // across two animation frames before sending — catches the case where
    // fit() ran mid-layout and produced a transient small value that would
    // shrink tmux. See #180.
    _sendPtyResize(agentId, commandId, cols, rows) {
        if (!commandId) return;
        const c = Number(cols);
        const r = Number(rows);
        // Floor of 60x10: smaller is almost certainly a layout glitch, not a
        // real terminal. xterm's default Terminal() is 80x24, so anything
        // below that range came from a degenerate measurement.
        if (!Number.isFinite(c) || !Number.isFinite(r) || c < 60 || r < 10) {
            // Bumped to console.log for #188 — drops were silently invisible
            // at debug level, making #180 recurrences impossible to diagnose
            // from a devtools recording.
            console.log(`[pty_resize] dropped reason=floor dims=${cols}x${rows} agent=${agentId} command=${commandId}`);
            return;
        }

        // Skip sending the same dimensions we just sent — eliminates spam
        // when fit() recomputes the same size repeatedly during a resize storm.
        const key = `${agentId}|${commandId}`;
        const last = this._lastSentResize?.get(key);
        if (last && last.cols === c && last.rows === r) return;

        // Debounce: collapse multiple rapid calls into one steady-state send.
        // Window-drag / sidebar-toggle triggers many ResizeObserver events in
        // quick succession; we want only the final settled measurement.
        if (!this._pendingResize) this._pendingResize = new Map();
        const prior = this._pendingResize.get(key);
        if (prior) clearTimeout(prior.timer);

        const pending = { cols: c, rows: r, timer: null };
        pending.timer = setTimeout(() => {
            // Two-frame stability check: re-read dims via fit at send time
            // and only send if the last debounced value still matches the
            // current measured value. Catches the "fit() returned a transient
            // small value while layout was settling" case.
            requestAnimationFrame(() => {
                requestAnimationFrame(() => {
                    const entry = this.panes.get(agentId);
                    const nowC = entry?.term?.cols;
                    const nowR = entry?.term?.rows;
                    if (Number.isFinite(nowC) && Number.isFinite(nowR)
                        && (nowC !== pending.cols || nowR !== pending.rows)) {
                        // Dims drifted between the original event and the
                        // settled frame — drop, the next term.onResize will
                        // bring us in.
                        console.log(`[pty_resize] dropped reason=drift ${pending.cols}x${pending.rows} → ${nowC}x${nowR} agent=${agentId} command=${commandId}`);
                        return;
                    }
                    if (!this._lastSentResize) this._lastSentResize = new Map();
                    this._lastSentResize.set(key, { cols: pending.cols, rows: pending.rows });
                    console.log(`[pty_resize] accepted dims=${pending.cols}x${pending.rows} agent=${agentId} command=${commandId}`);
                    this.send({
                        type: 'pty_resize',
                        agent_id: agentId,
                        command_id: commandId,
                        cols: pending.cols,
                        rows: pending.rows,
                    });
                });
            });
        }, 150);
        this._pendingResize.set(key, pending);
    }

    // Manual escape hatch for renderer/PTY drift (#180). Resets xterm
    // state, fits + sends a fresh resize, and re-discovers / re-attaches
    // to the underlying tmux session. Operator-triggered fallback when
    // the automatic protections aren't enough (multi-window tmux, deep
    // reconnect chains, etc.).
    resyncPane(agentId) {
        const entry = this.panes.get(agentId);
        if (!entry || !entry.term) return;
        try { entry.term.reset(); } catch (_) {}
        try { entry.fitAddon?.fit(); } catch (_) {}
        // Drop the stored seq so the next attach asks the server for a
        // fresh keyframe instead of a delta against a stale baseline.
        const sessionId = this.shellCommandIds.get(agentId);
        if (sessionId) this.lastSeqPerSession?.delete(sessionId);
        // Re-discover sessions (matches the "Reconnect" button flow).
        this.discoverAndAttach(agentId);
        this.showToast(`Resyncing ${agentId} terminal…`, 'info');
    }

    handleVmControl(agentId, action) {
        const instance = [...this.instances.values()].find((candidate) =>
            candidate.id === agentId || candidate.name === agentId);
        if (!instance) {
            this.showToast(
                `Canonical instance state is unavailable for ${agentId}; lifecycle controls are disabled until inventory reconciles.`,
                'error',
            );
            return;
        }

        if (action === 'force-off') {
            this.showToast(
                `${instance.name} does not advertise a distinct force-off operation. Use the reviewed destroy action from Instances if permanent removal is intended.`,
                'error',
            );
            return;
        }
        if (action === 'deploy') {
            this.showToast(
                `${instance.name} does not advertise a standalone deploy-agent operation. Use reprovision when that capability is available.`,
                'error',
            );
            return;
        }
        if (action === 'delete') {
            this.showConfirmDialog({
                title: `Destroy ${instance.name}?`,
                message: 'This permanently removes the runtime instance. Terminal evidence remains in the operation record.',
                confirmText: 'Destroy instance',
                confirmClass: 'danger',
                onConfirm: () => this.requestInstanceAction(instance, 'destroy'),
            });
            return;
        }
        this.requestInstanceAction(instance, action);
    }

    // =========================================================================
    // VM List Sidebar
    // =========================================================================

    setupVmSidebar() {
        // Legacy - now handled by setupBladeNav
        this.setupCreateVmModal();
    }

    setupCreateVmModal() {
        const modal = document.getElementById('create-vm-modal');
        if (!modal) return;

        const overlay = modal.querySelector('.modal-overlay');
        const closeBtn = modal.querySelector('.modal-close');
        const cancelBtn = modal.querySelector('.cancel-btn');
        const form = document.getElementById('create-vm-form');

        const closeModal = () => {
            modal.classList.add('hidden');
            form.reset();
        };

        overlay.addEventListener('click', closeModal);
        closeBtn.addEventListener('click', closeModal);
        cancelBtn.addEventListener('click', closeModal);

        // Loadout selection change — update detail panel and resource defaults
        const loadoutSelect = document.getElementById('vm-loadout');
        if (loadoutSelect) {
            loadoutSelect.addEventListener('change', () => {
                this.onLoadoutSelected();
            });
        }

        // Mode toggle (Preset / Custom)
        modal.querySelectorAll('.loadout-mode-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                modal.querySelectorAll('.loadout-mode-tab').forEach(t => t.classList.remove('active'));
                tab.classList.add('active');
                const mode = tab.dataset.mode;
                document.getElementById('loadout-preset-panel').classList.toggle('hidden', mode !== 'preset');
                document.getElementById('loadout-custom-panel').classList.toggle('hidden', mode !== 'custom');
                if (mode === 'custom') this.renderComposeBuilder();
            });
        });

        form.addEventListener('submit', async (e) => {
            e.preventDefault();
            await this.handleCreateInstance();
        });

        // Runtime selector — show/hide runtime-specific fields (#178).
        const runtimeSelect = document.getElementById('instance-runtime');
        if (runtimeSelect) {
            runtimeSelect.addEventListener('change', () => this._applyRuntimeVisibility());
        }

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !modal.classList.contains('hidden')) {
                closeModal();
            }
        });
    }

    onLoadoutSelected() {
        const select = document.getElementById('vm-loadout');
        const detail = document.getElementById('loadout-detail');
        const hint = document.getElementById('resource-hint');
        if (!select) return;

        const loadout = this.loadouts.find(l => l.path === select.value);
        if (!loadout) {
            if (detail) detail.innerHTML = '';
            if (hint) hint.classList.add('hidden');
            return;
        }

        // Auto-populate resource fields from loadout defaults
        if (loadout.resources) {
            this.applyLoadoutResources(loadout.resources);
            if (hint) hint.classList.remove('hidden');
        }

        // Render detail panel
        if (detail) detail.innerHTML = this.renderLoadoutDetail(loadout);
    }

    applyLoadoutResources(res) {
        if (res.cpus) {
            const sel = document.getElementById('vm-vcpus');
            if (sel) sel.value = String(res.cpus);
        }
        if (res.memory) {
            const mb = this.parseMemoryToMb(res.memory);
            if (mb) {
                const sel = document.getElementById('vm-memory');
                if (sel) sel.value = String(mb);
            }
        }
        if (res.disk) {
            const gb = parseInt(res.disk);
            if (gb) {
                const sel = document.getElementById('vm-disk');
                if (sel) {
                    // Pick closest option
                    const opts = [...sel.options].map(o => parseInt(o.value));
                    const closest = opts.reduce((a, b) => Math.abs(b - gb) < Math.abs(a - gb) ? b : a);
                    sel.value = String(closest);
                }
            }
        }
    }

    parseMemoryToMb(mem) {
        const m = mem.match(/^(\d+)\s*(G|M)/i);
        if (!m) return null;
        const val = parseInt(m[1]);
        return m[2].toUpperCase() === 'G' ? val * 1024 : val;
    }

    renderLoadoutDetail(loadout) {
        const tags = [];

        // Network mode tag
        if (loadout.network_mode) {
            const cls = loadout.network_mode === 'isolated' ? 'tag-warn' : '';
            tags.push(`<span class="loadout-tag ${cls}">${this.esc(loadout.network_mode)} network</span>`);
        }

        // AI tools
        for (const tool of (loadout.ai_tools || [])) {
            tags.push(`<span class="loadout-tag tag-ai">${this.esc(String(tool).replace(/_/g, ' '))}</span>`);
        }

        // Frameworks
        for (const fw of (loadout.frameworks || [])) {
            tags.push(`<span class="loadout-tag tag-fw">${this.esc(fw.name)}</span>`);
        }

        const desc = loadout.description ? `<div class="loadout-desc">${this.esc(loadout.description)}</div>` : '';
        const tagHtml = tags.length ? `<div class="loadout-tags">${tags.join('')}</div>` : '';

        return `${desc}${tagHtml}`;
    }

    async fetchLoadouts() {
        try {
            const resp = (await ApiClient.request('/api/v1/loadouts')).response;
            if (!resp.ok) {
                console.log('Loadouts API not available:', resp.status);
                return;
            }
            const data = await resp.json();
            if (data.loadouts) {
                this.loadouts = data.loadouts;
                this.loadoutsLoaded = true;
                this.populateLoadoutSelector();
            }
        } catch (e) {
            console.error('Failed to fetch loadouts:', e);
        }
    }

    async fetchLoadoutRegistry() {
        try {
            const resp = (await ApiClient.request('/api/v1/loadout/registry')).response;
            if (!resp.ok) return;
            this.loadoutRegistry = await resp.json();
            // Populate init select from registry
            const initSelect = document.getElementById('vm-init');
            if (initSelect && this.loadoutRegistry.init_scripts?.length) {
                initSelect.innerHTML = '';
                for (const s of this.loadoutRegistry.init_scripts) {
                    const opt = document.createElement('option');
                    opt.value = s.name;
                    opt.textContent = s.label;
                    if (s.default) opt.selected = true;
                    initSelect.appendChild(opt);
                }
            }
        } catch (e) {
            console.error('Failed to fetch loadout registry:', e);
        }
    }

    renderComposeBuilder() {
        const registry = this.loadoutRegistry;
        if (!registry) return;

        const fwGrid = document.getElementById('vm-frameworks');
        const pvGrid = document.getElementById('vm-providers');
        if (!fwGrid || !pvGrid) return;

        // Only render chips once
        if (fwGrid.dataset.rendered) {
            this.updateComposeSummary();
            return;
        }

        fwGrid.innerHTML = '';
        for (const fw of (registry.frameworks || [])) {
            const chip = document.createElement('button');
            chip.type = 'button';
            chip.className = 'compose-chip' + (fw.reserved ? ' chip-reserved' : '');
            chip.dataset.value = fw.name;
            chip.title = fw.description || '';
            chip.textContent = fw.label;
            chip.addEventListener('click', () => {
                chip.classList.toggle('selected');
                this.updateComposeSummary();
            });
            fwGrid.appendChild(chip);
        }
        fwGrid.dataset.rendered = '1';

        pvGrid.innerHTML = '';
        for (const pv of (registry.providers || [])) {
            const chip = document.createElement('button');
            chip.type = 'button';
            chip.className = 'compose-chip';
            chip.dataset.value = pv.name;
            chip.title = pv.label;
            chip.textContent = pv.label;
            chip.addEventListener('click', () => {
                chip.classList.toggle('selected');
                this.updateComposeSummary();
            });
            pvGrid.appendChild(chip);
        }
        pvGrid.dataset.rendered = '1';

        this.updateComposeSummary();
    }

    updateComposeSummary() {
        const summary = document.getElementById('compose-summary');
        if (!summary) return;
        const frameworks = this.getSelectedChips('vm-frameworks');
        const providers = this.getSelectedChips('vm-providers');
        const init = document.getElementById('vm-init')?.value || 'ubuntu';
        if (!frameworks.length && !providers.length) {
            summary.classList.add('hidden');
            return;
        }
        summary.classList.remove('hidden');
        summary.innerHTML =
            `<span class="compose-label">init:</span> <code>${this.esc(init)}</code> &nbsp; ` +
            `<span class="compose-label">frameworks:</span> ${frameworks.map(f => `<code>${this.esc(f)}</code>`).join(' ') || '<em>none</em>'} &nbsp; ` +
            `<span class="compose-label">providers:</span> ${providers.map(p => `<code>${this.esc(p)}</code>`).join(' ') || '<em>none</em>'}`;
    }

    getSelectedChips(gridId) {
        const grid = document.getElementById(gridId);
        if (!grid) return [];
        return Array.from(grid.querySelectorAll('.compose-chip.selected')).map(c => c.dataset.value);
    }

    populateLoadoutSelector() {
        const select = document.getElementById('vm-loadout');
        if (!select) return;

        select.innerHTML = '';

        // Group by category
        const categories = {};
        for (const l of this.loadouts) {
            const cat = l.category || 'other';
            if (!categories[cat]) categories[cat] = [];
            categories[cat].push(l);
        }

        const catNames = {
            'per-provider': 'Single Provider',
            'collaboration': 'Multi-Provider',
            'task-focused': 'Task-Focused',
            'backward-compat': 'Baseline',
            'other': 'Other'
        };

        const catOrder = ['per-provider', 'collaboration', 'task-focused', 'backward-compat', 'other'];
        for (const cat of catOrder) {
            const items = categories[cat];
            if (!items || !items.length) continue;

            const group = document.createElement('optgroup');
            group.label = catNames[cat] || cat;

            for (const l of items) {
                const opt = document.createElement('option');
                opt.value = l.path;
                opt.textContent = l.name;
                group.appendChild(opt);
            }
            select.appendChild(group);
        }

        // Default to claude-only
        const claudeOnly = this.loadouts.find(l => l.name === 'claude-only');
        if (claudeOnly) {
            select.value = claudeOnly.path;
        }

        this.onLoadoutSelected();
    }

    showCreateVmModal() {
        const modal = document.getElementById('create-vm-modal');
        if (modal) {
            modal.classList.remove('hidden');
            if (!this.loadoutsLoaded) this.fetchLoadouts();
            else this.onLoadoutSelected();
            // Lazy-load container images on first open.
            if (!this._containerImagesLoaded) this.fetchContainerImages();
            this.fetchRuntimeAvailability();
            this.fetchBootstrapReadiness();
            this.fetchStartupProfiles().catch((error) => console.debug('Startup profile discovery unavailable:', error));
            this._applyRuntimeVisibility();
            document.getElementById('vm-name').focus();
        }
    }

    // Show / hide form sections based on the runtime dropdown (#178).
    _applyRuntimeVisibility() {
        const runtime = document.getElementById('instance-runtime')?.value || 'vm';
        document.querySelectorAll('.runtime-only').forEach(el => {
            const target = el.dataset.runtime;
            el.hidden = target !== runtime;
        });
        const submit = document.getElementById('create-instance-submit');
        if (submit) {
            submit.textContent = runtime === 'container'
                ? 'Create container'
                : runtime === 'host'
                    ? 'Create host instance'
                    : 'Create VM';
        }
    }

    async fetchRuntimeAvailability() {
        const note = document.getElementById('runtime-availability');
        try {
            const data = await this.managementJson('/api/v2/admin/runtime/providers', {
                owner: 'runtime-provider-readiness',
            });
            const runtimes = Array.isArray(data.runtimes) ? data.runtimes : [];
            this.defaultVmProvider = data.default_vm_provider || null;
            this.runtimeAvailability.clear();
            for (const runtime of runtimes) {
                if (!runtime || typeof runtime.id !== 'string') continue;
                this.runtimeAvailability.set(runtime.id, runtime);
            }

            const select = document.getElementById('instance-runtime');
            const optionMap = { qemu: 'vm', docker: 'container', host: 'host' };
            for (const [runtimeId, optionValue] of Object.entries(optionMap)) {
                const option = select?.querySelector(`option[value="${optionValue}"]`);
                const descriptor = this.runtimeAvailability.get(runtimeId);
                if (!option || !descriptor) continue;
                option.disabled = !descriptor.available;
                option.title = descriptor.available
                    ? `${descriptor.isolation_tier || 'unknown isolation'}; ${descriptor.architecture || 'unknown architecture'}`
                    : (descriptor.unavailable_reason || descriptor.unavailable_code || 'Unavailable');
            }

            if (select?.selectedOptions[0]?.disabled) {
                const fallback = Array.from(select.options).find(option => !option.disabled);
                if (fallback) select.value = fallback.value;
                this._applyRuntimeVisibility();
            }
            if (note) {
                note.textContent = runtimes.length
                    ? runtimes.map(runtime => {
                        const status = runtime.available ? 'available' : 'unavailable';
                        const isolation = runtime.isolation_tier ? `, ${runtime.isolation_tier}` : '';
                        const reason = !runtime.available
                            ? ` — ${runtime.unavailable_reason || runtime.unavailable_code || 'provider did not report remediation'}`
                            : '';
                        return `${runtime.id}: ${status}${isolation}${reason}`;
                    }).join(' · ')
                    : 'Runtime-kind discovery is not available from this server.';
            }
        } catch (error) {
            // Older servers do not return the additive `runtimes` field. Keep
            // existing choices usable and degrade to a neutral status note.
            if (note) note.textContent = 'Runtime availability is not reported by this server.';
            console.debug('Runtime availability discovery unavailable:', error);
        }
    }

    async fetchBootstrapReadiness() {
        const note = document.getElementById('bootstrap-readiness');
        try {
            const data = await this.managementJson('/api/v2/admin/bootstrap/readiness', {
                owner: 'bootstrap-readiness',
            });
            this.bootstrapReadiness = data;
            const managementEndpoint = data.management_websocket?.endpoint;
            if (typeof managementEndpoint === 'string' && managementEndpoint.trim()) {
                this.advertisedManagementWsUrl = managementEndpoint;
            }
            const ca = data.ca_provider || {};
            const bootstrap = data.bootstrap || {};
            const reason = bootstrap.reason || ca.reason;
            if (note) {
                note.textContent = `Secure bootstrap: ${data.status || 'unknown'}; CA ${ca.status || 'unknown'}; enrollment ${bootstrap.status || 'unknown'}${reason ? ` — ${reason}` : ''}`;
                note.dataset.state = data.status || 'unknown';
            }
        } catch (error) {
            this.bootstrapReadiness = null;
            if (note) {
                note.textContent = `Secure bootstrap readiness unavailable — ${error.message}`;
                note.dataset.state = 'unavailable';
            }
        }
    }

    async fetchContainerImages() {
        try {
            const resp = (await ApiClient.request('/api/v1/container-images')).response;
            const select = document.getElementById('container-image');
            if (!select) return;
            if (!resp.ok) {
                // Endpoint not present — fall back to free-text only.
                this._enableContainerImageCustomFallback();
                this._containerImagesLoaded = true;
                return;
            }
            const data = await resp.json();
            const images = data.images || [];
            select.innerHTML = '';
            for (const img of images) {
                const opt = document.createElement('option');
                opt.value = img.ref;
                opt.textContent = `${img.label} — ${img.description}`;
                if (img.default) opt.selected = true;
                select.appendChild(opt);
            }
            const customOpt = document.createElement('option');
            customOpt.value = '__custom__';
            customOpt.textContent = 'Custom image…';
            select.appendChild(customOpt);
            select.addEventListener('change', () => {
                const custom = document.getElementById('container-image-custom-group');
                if (custom) custom.hidden = select.value !== '__custom__';
            });
            this._containerImagesLoaded = true;
        } catch (e) {
            console.warn('container-images fetch failed; falling back to custom input', e);
            this._enableContainerImageCustomFallback();
            this._containerImagesLoaded = true;
        }
    }

    _enableContainerImageCustomFallback() {
        const select = document.getElementById('container-image');
        const custom = document.getElementById('container-image-custom-group');
        if (select) { select.innerHTML = ''; select.hidden = true; }
        if (custom) custom.hidden = false;
    }

    async handleCreateInstance() {
        const runtime = document.getElementById('instance-runtime')?.value || 'vm';
        try {
            window.ManagementUI.requireAvailableRuntime(this.runtimeAvailability, runtime);
        } catch (error) {
            this.showToast(error.message, 'error');
            return;
        }
        if (runtime === 'container') return this.handleCreateContainer();
        if (runtime === 'host') return this.handleCreateHost();
        return this.handleCreateVm();
    }

    async handleCreateHost() {
        const nameInput = document.getElementById('vm-name');
        if (!nameInput.value.trim()) {
            this.showToast('Please enter a host instance name', 'error');
            return;
        }
        if (!/^[a-z0-9-]+$/.test(nameInput.value)) {
            this.showToast('Name can only contain lowercase letters, numbers, and hyphens', 'error');
            return;
        }
        if (!document.getElementById('host-access-ack')?.checked) {
            this.showToast('Acknowledge the full-host-access warning before continuing', 'error');
            return;
        }

        const name = `agent-${nameInput.value.trim()}`;
        const workingDir = (document.getElementById('host-working-dir')?.value || '').trim();
        const body = {
            name,
            runtime: 'host',
            agentshare: false,
            start: document.getElementById('host-autostart')?.checked !== false,
        };
        const startupProfileId = document.getElementById('instance-startup-profile')?.value;
        if (startupProfileId) body.startup_profile_id = startupProfileId;
        if (workingDir) body.working_dir = workingDir;

        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();
        this._applyRuntimeVisibility();
        await this.createCanonicalInstance(body);
    }

    async handleCreateContainer() {
        const nameInput = document.getElementById('vm-name');
        if (!nameInput.value.trim()) {
            this.showToast('Please enter a container name', 'error');
            return;
        }
        if (!/^[a-z0-9-]+$/.test(nameInput.value)) {
            this.showToast('Name can only contain lowercase letters, numbers, and hyphens', 'error');
            return;
        }
        const name = `agent-${nameInput.value.trim()}`;

        const select = document.getElementById('container-image');
        let image = select && !select.hidden ? select.value : '';
        if (image === '__custom__' || !image) {
            image = (document.getElementById('container-image-custom')?.value || '').trim();
        }
        if (!image) {
            this.showToast('Please choose an image', 'error');
            return;
        }

        const startupProfileId = document.getElementById('instance-startup-profile')?.value;
        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();
        this._applyRuntimeVisibility();

        const body = {
            name,
            runtime: 'docker',
            image,
            start: true,
        };
        if (startupProfileId) body.startup_profile_id = startupProfileId;
        await this.createCanonicalInstance(body);
    }

    async handleCreateVm() {
        const nameInput = document.getElementById('vm-name');
        const name = `agent-${nameInput.value.trim()}`;
        const vcpus = parseInt(document.getElementById('vm-vcpus').value);
        const memory_mb = parseInt(document.getElementById('vm-memory').value);
        const disk_gb = parseInt(document.getElementById('vm-disk').value);
        const agentshare = document.getElementById('vm-agentshare').checked;
        const start = document.getElementById('vm-autostart').checked;

        // Validate name
        if (!nameInput.value.trim()) {
            this.showToast('Please enter a VM name', 'error');
            return;
        }
        if (!/^[a-z0-9-]+$/.test(nameInput.value)) {
            this.showToast('Name can only contain lowercase letters, numbers, and hyphens', 'error');
            return;
        }

        // Determine mode (preset vs custom)
        const activeTab = document.querySelector('.loadout-mode-tab.active');
        const mode = activeTab?.dataset.mode || 'preset';

        let body;
        if (mode === 'custom') {
            const frameworks = this.getSelectedChips('vm-frameworks');
            const providers = this.getSelectedChips('vm-providers');
            this.showToast(
                `Save custom composition (${frameworks.length} frameworks, ${providers.length} providers) as a schema-valid loadout before provisioning.`,
                'error',
            );
            return;
        } else {
            const loadout = document.getElementById('vm-loadout').value;
            if (!loadout) {
                this.showToast('Please select a loadout', 'error');
                return;
            }
            body = {
                name,
                runtime: 'qemu',
                provider: this.defaultVmProvider || undefined,
                loadout,
                vcpus,
                memory_mb,
                disk_gb,
                agentshare,
                start,
            };
            const startupProfileId = document.getElementById('instance-startup-profile')?.value;
            if (startupProfileId) body.startup_profile_id = startupProfileId;
        }

        // Close modal
        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();

        await this.createCanonicalInstance(body);
    }

    async createCanonicalInstance(body) {
        const name = body.name;
        const intentKey = ApiClient.newIntentId();
        const intentId = `create:${name}`;
        const baselineInstance = this.instances.get(name);
        const reconciliationBefore = {
            checked_at: this.instanceInventoryObservedAt,
            authoritative: Boolean(this.instanceInventoryObservedAt),
            instance_present: Boolean(baselineInstance),
            instance_id: baselineInstance?.id || null,
            observed_state: baselineInstance?.observed_state || baselineInstance?.state || null,
        };
        if (this.instanceMutationIntents.has(intentId)
            || this.hasBlockingOperation(name, 'instance.provision')) {
            this.showToast(`Provisioning is already in flight for ${name}`, 'info');
            return;
        }
        this.instanceMutationIntents.set(intentId, intentKey);
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey,
            target: name,
            kind: 'instance.provision',
            reconciliationBefore,
            retryRequest: {
                path: '/api/v2/admin/instances',
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            },
        });
        this.showToast(`Creating ${body.runtime} instance ${name}…`, 'info');
        try {
            const outcome = await this.managementRequest('/api/v2/admin/instances', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
                idempotencyKey: intentKey,
                owner: intentId,
            });
            if (outcome.kind === 'accepted') {
                this.trackCanonicalOperation({
                    ...(outcome.body || {}),
                    trace_id: outcome.traceId,
                    request_id: outcome.requestId,
                    operation_id: outcome.operationId,
                    idempotency_replayed: outcome.idempotencyReplayed,
                }, {
                    target: name,
                    kind: 'instance.provision',
                    intentKey,
                    reconciliationBefore,
                    retryRequest: {
                        path: '/api/v2/admin/instances',
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify(body),
                    },
                });
                this.clearPendingMutationIntent(pendingIntentId);
                this.showToast(`${name} provisioning accepted`, 'success');
            } else {
                await this.fetchInstances();
                this.clearPendingMutationIntent(pendingIntentId);
            }
        } catch (error) {
            if (error instanceof UnknownMutationOutcomeError) {
                this.promotePendingMutationIntent(pendingIntentId, error);
                await this.fetchInstances();
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
                this.showToast(`Failed to create ${name}: ${error.message}`, 'error');
            }
        } finally {
            this.instanceMutationIntents.delete(intentId);
        }
    }

    async fetchInstances() {
        const list = document.getElementById('vm-list');
        try {
            const data = await this.managementJson('/api/v2/admin/instances', { owner: 'instance-inventory' });
            const items = window.ManagementUI.parseInstanceCollection(data);
            this.instances.clear();
            for (const instance of items) {
                if (!instance || typeof instance.name !== 'string') continue;
                this.instances.set(instance.name, instance);
            }
            this.instanceInventoryObservedAt = new Date().toISOString();
            this.instanceInventoryError = null;
            this.degradedProviders = Array.isArray(data.degraded_providers) ? data.degraded_providers : [];
            this.reconcileUnknownOperationsFromInventory();
            this.restoreDeepLinkSelection();
            this.renderVmList();
            this.renderOperations();
        } catch (error) {
            if (error.code === 'stale_response' || error.code === 'request_aborted') return;
            this.instanceInventoryError = error;
            if (list) {
                list.replaceChildren();
                const message = document.createElement('div');
                message.className = 'vm-degraded-banner';
                message.textContent = `Instance inventory unavailable: ${error.message}`;
                list.append(message);
            }
            console.error('Failed to fetch canonical instances:', error);
        }
    }

    // Compatibility alias for older call-sites while the view migrates from
    // VM-shaped naming to the canonical runtime-neutral inventory.
    async fetchVms() {
        return this.fetchInstances();
    }

    restoreDeepLinkSelection() {
        const params = new URLSearchParams(window.location.search);
        const instanceName = params.get('instance');
        const operationId = params.get('operation');
        if (instanceName && this.instances.has(instanceName)) {
            this.selectedAgent = instanceName;
            this.resourceState.select('instance', instanceName);
        }
        if (operationId && this.operations.has(operationId)) {
            this.selectedOperation = operationId;
            this.resourceState.select('operation', operationId);
        }
    }

    updateManagementDeepLink({ instance, operation } = {}) {
        const url = new URL(window.location.href);
        if (instance === null) url.searchParams.delete('instance');
        else if (instance) url.searchParams.set('instance', instance);
        if (operation === null) url.searchParams.delete('operation');
        else if (operation) url.searchParams.set('operation', operation);
        if (instance !== undefined) this.resourceState.select('instance', instance);
        if (operation !== undefined) this.resourceState.select('operation', operation);
        history.replaceState(null, '', url);
    }

    async requestInstanceAction(instance, action) {
        const capability = `instance.${action}`;
        const capabilities = new Set(instance.capabilities || []);
        if (!capabilities.has(capability)) {
            this.showToast(`${instance.name} does not advertise ${capability}`, 'error');
            return;
        }
        const intentKey = ApiClient.newIntentId();
        const intentId = `${instance.id}:${action}`;
        const reconciliationBefore = {
            checked_at: this.instanceInventoryObservedAt,
            authoritative: Boolean(this.instanceInventoryObservedAt),
            instance_present: true,
            instance_id: instance.id,
            observed_state: instance.observed_state || instance.state || null,
        };
        if (this.instanceMutationIntents.has(intentId)
            || this.hasBlockingOperation(instance.id, capability)) {
            this.showToast(`${action} is already in flight for ${instance.name}`, 'info');
            return;
        }
        this.instanceMutationIntents.set(intentId, intentKey);
        const retryRequest = {
            path: `/api/v2/admin/instances/${encodeURIComponent(instance.id)}/${action}`,
            method: 'POST',
        };
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey,
            target: instance.name,
            targetId: instance.id,
            kind: capability,
            reconciliationBefore,
            retryRequest,
        });
        try {
            const outcome = await this.managementRequest(
                `/api/v2/admin/instances/${encodeURIComponent(instance.id)}/${action}`,
                { method: 'POST', idempotencyKey: intentKey, owner: intentId },
            );
            if (outcome.kind === 'accepted') {
                this.trackCanonicalOperation({
                    ...(outcome.body || {}),
                    trace_id: outcome.traceId,
                    request_id: outcome.requestId,
                    operation_id: outcome.operationId,
                    idempotency_replayed: outcome.idempotencyReplayed,
                }, {
                    target: instance.name,
                    targetId: instance.id,
                    kind: capability,
                    intentKey,
                    reconciliationBefore,
                    retryRequest,
                });
                this.clearPendingMutationIntent(pendingIntentId);
            }
            await this.fetchInstances();
            if (outcome.kind !== 'accepted') this.clearPendingMutationIntent(pendingIntentId);
        } catch (error) {
            if (error instanceof UnknownMutationOutcomeError) {
                this.promotePendingMutationIntent(pendingIntentId, error);
                await this.fetchInstances();
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
                this.showToast(`${action} failed for ${instance.name}: ${error.message}`, 'error');
            }
        } finally {
            this.instanceMutationIntents.delete(intentId);
            this.renderVmList();
        }
    }

    trackCanonicalOperation(operation, metadata = {}) {
        const id = operation.id || operation.operation_id || operation.operation?.id;
        if (!id) throw new TypeError('accepted mutation response is missing an operation id');
        const value = {
            ...(operation.operation || operation),
            id,
            target: operation.target || metadata.target || null,
            target_id: operation.target_id || metadata.targetId || null,
            kind: operation.kind || metadata.kind || 'unknown',
            intent_key: metadata.intentKey || operation.intent_key || null,
            trace_id: operation.trace_id || operation.error?.trace_id || null,
            request_id: operation.request_id || operation.error?.request_id || null,
            idempotency_replayed: operation.idempotency_replayed === true,
            retry_request: metadata.retryRequest || operation.retry_request || null,
            reconciliation_before: metadata.reconciliationBefore || operation.reconciliation_before || null,
            pollable: metadata.pollable ?? operation.pollable ?? true,
        };
        this.operations.set(id, value);
        this.persistTrackedOperations();
        this.renderOperations();
        const instanceTarget = String(value.kind || '').startsWith('instance.')
            ? value.target
            : null;
        this.updateManagementDeepLink({ instance: instanceTarget, operation: id });
        if (value.pollable !== false && value.state !== 'unknown' && !this.isTerminalOperation(value)) {
            this.pollCanonicalOperation(id);
        }
        return value;
    }

    trackPendingMutationIntent({
        intentKey, target, targetId = null, kind, reconciliationBefore = null,
        retryRequest, pollable = true,
    }) {
        const id = `intent:${intentKey}`;
        this.operations.set(id, {
            id,
            target,
            target_id: targetId,
            kind,
            intent_key: intentKey,
            state: 'dispatching',
            created_at: new Date().toISOString(),
            reconciliation_before: reconciliationBefore,
            retry_request: retryRequest,
            pollable,
            progress: { percent: 0, phase: 'request dispatch in progress' },
        });
        this.persistTrackedOperations();
        this.renderOperations();
        return id;
    }

    clearPendingMutationIntent(id) {
        if (!id || !this.operations.delete(id)) return;
        this.persistTrackedOperations();
        this.renderOperations();
    }

    promotePendingMutationIntent(id, error) {
        const pending = this.operations.get(id);
        if (!pending) return null;
        this.operations.delete(id);
        return this.trackCanonicalOperation({
            ...pending,
            id: `unknown:${pending.intent_key}`,
            state: 'unknown',
            progress: null,
            error: { detail: error.message, code: error.code },
        });
    }

    persistTrackedOperations() {
        try {
            const records = [...this.operations.values()].map((operation) => ({
                id: operation.id,
                target: operation.target,
                target_id: operation.target_id,
                kind: operation.kind,
                intent_key: operation.intent_key,
                state: operation.state,
                created_at: operation.created_at,
                completed_at: operation.completed_at,
                progress: operation.progress,
                error: operation.error,
                result: operation.result,
                trace_id: operation.trace_id,
                request_id: operation.request_id,
                idempotency_replayed: operation.idempotency_replayed,
                reconciliation_error: operation.reconciliation_error,
                reconciliation_evidence: operation.reconciliation_evidence,
                reconciliation_before: operation.reconciliation_before,
                retry_request: operation.retry_request,
                pollable: operation.pollable,
            }));
            localStorage.setItem('management-ui-operations-v1', JSON.stringify(records.slice(-100)));
        } catch (_) {}
    }

    restoreTrackedOperations() {
        let records = [];
        try { records = JSON.parse(localStorage.getItem('management-ui-operations-v1') || '[]'); } catch (_) {}
        let convertedInterruptedIntent = false;
        for (const record of Array.isArray(records) ? records : []) {
            if (!record?.id) continue;
            const interrupted = String(record.id).startsWith('intent:');
            convertedInterruptedIntent ||= interrupted;
            const unknown = interrupted || String(record.id).startsWith('unknown:');
            const id = interrupted ? `unknown:${record.intent_key || String(record.id).slice(7)}` : record.id;
            this.operations.set(id, {
                ...record,
                id,
                state: unknown ? 'unknown' : (record.state || 'reconciling'),
                progress: interrupted ? null : record.progress,
                error: interrupted ? {
                    code: 'mutation_outcome_unknown',
                    detail: 'The dashboard reloaded while this request was in flight; reconcile or replay the exact stored intent key.',
                } : record.error,
            });
            if (!unknown && record.pollable !== false && !this.isTerminalOperation(record)) {
                this.pollCanonicalOperation(record.id);
            }
        }
        if (convertedInterruptedIntent) this.persistTrackedOperations();
        this.renderOperations();
    }

    async pollCanonicalOperation(operationId) {
        try {
            const body = await this.managementJson(
                `/api/v2/admin/operations/${encodeURIComponent(operationId)}`,
                { owner: `operation:${operationId}` },
            );
            const previous = this.operations.get(operationId) || {};
            const operation = { ...previous, ...body, id: operationId };
            this.operations.set(operationId, operation);
            this.persistTrackedOperations();
            this.renderOperations();
            if (!this.isTerminalOperation(operation)) {
                setTimeout(() => this.pollCanonicalOperation(operationId), 2000);
            } else {
                await this.fetchInstances();
            }
        } catch (error) {
            const previous = this.operations.get(operationId);
            if (previous) {
                this.operations.set(operationId, { ...previous, state: 'unknown', reconciliation_error: error.message });
                this.persistTrackedOperations();
                this.renderOperations();
            }
        }
    }

    async reconcileInstancesAndOperations() {
        await this.fetchInstances();
        await Promise.all([...this.operations.entries()]
            .filter(([id, operation]) => operation.pollable !== false
                && !this.isTerminalOperation(operation)
                && !String(id).startsWith('unknown:') && !String(id).startsWith('intent:'))
            .map(([id]) => this.pollCanonicalOperation(id)));
    }

    isTerminalOperation(operation) {
        return window.ManagementUI.isTerminalOperation(operation);
    }

    hasBlockingOperation(target, kind) {
        return [...this.operations.values()].some((operation) =>
            !this.isTerminalOperation(operation)
            && operation.kind === kind
            && (operation.target_id === target || operation.target === target));
    }

    reconcileUnknownOperationsFromInventory() {
        const now = new Date().toISOString();
        let changed = false;
        for (const [id, operation] of this.operations) {
            if (!String(id).startsWith('unknown:') || operation.state !== 'unknown') continue;
            if (!String(operation.kind || '').startsWith('instance.')) continue;
            const resolution = window.ManagementUI.reconcileUnknownInstanceOperation(
                operation, this.instances, now,
            );
            this.operations.set(id, {
                ...operation,
                state: resolution.reconciled ? 'succeeded' : 'unknown',
                completed_at: resolution.reconciled ? now : operation.completed_at,
                progress: resolution.reconciled
                    ? { percent: 100, phase: 'authoritative inventory reconciled' }
                    : operation.progress,
                result: resolution.result || operation.result,
                reconciliation_evidence: resolution.evidence,
            });
            changed = true;
        }
        if (changed) {
            this.persistTrackedOperations();
            this.renderOperations();
        }
    }

    async retryUnknownOperation(operation) {
        if (!String(operation.id).startsWith('unknown:')
            || operation.state !== 'unknown' || !operation.intent_key || !operation.retry_request) return;
        const request = operation.retry_request;
        try {
            const outcome = await this.managementRequest(request.path, {
                method: request.method,
                headers: request.headers,
                body: request.body,
                idempotencyKey: operation.intent_key,
                owner: `retry:${operation.id}`,
            });
            if (outcome.kind === 'accepted') {
                const recoveredOperation = this.trackCanonicalOperation({
                    ...(outcome.body || {}),
                    operation_id: outcome.operationId,
                    trace_id: outcome.traceId,
                    request_id: outcome.requestId,
                    idempotency_replayed: outcome.idempotencyReplayed,
                }, {
                    target: operation.target,
                    targetId: operation.target_id,
                    kind: operation.kind,
                    intentKey: operation.intent_key,
                    retryRequest: request,
                });
                this.operations.delete(operation.id);
                this.selectedOperation = recoveredOperation.id;
                this.persistTrackedOperations();
                this.renderOperations();
            } else {
                if (operation.kind === 'celld.command') {
                    this.reconcileCelldCommandOperations(outcome.body);
                    this.recordCelldResult('command replay reconciliation', {
                        operation_id: operation.intent_key,
                        cell: outcome.body,
                    }, 'celld-cell-result');
                    return;
                }
                this.operations.set(operation.id, {
                    ...operation,
                    state: 'succeeded',
                    completed_at: new Date().toISOString(),
                    progress: { percent: 100, phase: 'recovered by exact idempotent replay' },
                    result: {
                        recovered_from: 'exact idempotent replay',
                        response: outcome.body ?? null,
                    },
                });
                this.persistTrackedOperations();
                this.renderOperations();
                await this.reconcileRecoveredMutation(operation);
            }
        } catch (error) {
            this.showToast(`Intent replay did not reconcile: ${error.message}`, 'error');
            await this.reconcileRecoveredMutation(operation);
        }
    }

    async reconcileRecoveredMutation(operation) {
        const kind = String(operation?.kind || '');
        if (kind.startsWith('config.startup.')) return this.fetchStartupProfiles();
        if (kind.startsWith('config.loadout.')) return this.fetchConfigLoadouts();
        if (kind.startsWith('config.storage.')) {
            try {
                if (this.storageUrl() === operation.target) return this.readStoragePath();
            } catch (_) {}
            return;
        }
        if (kind === 'fleet.reconcile') return this.fetchFleetInventory();
        if (kind.startsWith('acceleration.')) return this.fetchInstances();
        if (kind.startsWith('instance.')) return this.fetchInstances();
    }

    renderOperations() {
        const list = document.getElementById('operation-list');
        if (!list) return;
        window.ManagementUI.renderOperationList(list, this.operations.values(), {
            selectedOperation: this.selectedOperation,
            onEvidence: (operation) => this.openActivityEvidence({
                instanceId: operation.target_id,
                agentId: operation.target_id,
                traceId: operation.trace_id,
            }),
            onReconcile: (operation) => {
                const kind = String(operation.kind || '');
                if (kind.startsWith('celld.')) return this.reconcileCelldOperation(operation);
                if (kind.startsWith('config.') || kind === 'fleet.reconcile'
                    || kind.startsWith('acceleration.')) {
                    return this.reconcileRecoveredMutation(operation);
                }
                return this.reconcileInstancesAndOperations();
            },
            onRetry: (operation) => this.retryUnknownOperation(operation),
            onSelect: (operation) => {
                this.selectedOperation = operation.id;
                this.updateManagementDeepLink({
                    instance: String(operation.kind || '').startsWith('instance.') ? operation.target : null,
                    operation: operation.id,
                });
            },
        });
    }

    renderVmList() {
        const list = document.getElementById('vm-list');
        if (!list) return;

        if (this.instanceInventoryError) {
            list.replaceChildren();
            const banner = document.createElement('div');
            banner.className = 'vm-degraded-banner';
            banner.textContent = `Canonical instance inventory unavailable: ${this.instanceInventoryError.message}. Lifecycle controls are disabled until authoritative state is restored.`;
            list.append(banner);
            const count = document.getElementById('vm-count');
            if (count) count.textContent = 'instances unavailable';
            return;
        }

        this.renderCanonicalInstanceList(list);
    }

    renderCanonicalInstanceList(list) {
        const instances = [...this.instances.values()].sort((a, b) => a.name.localeCompare(b.name));
        list.replaceChildren();
        for (const degraded of this.degradedProviders || []) {
            const banner = document.createElement('div');
            banner.className = 'vm-degraded-banner';
            banner.textContent = `${degraded.runtime || 'provider'} degraded: ${degraded.detail || degraded.code}`;
            list.append(banner);
        }
        if (!instances.length) {
            const empty = document.createElement('div');
            empty.className = 'vm-placeholder';
            empty.textContent = 'No instances found';
            list.append(empty);
            this.updateVmCount();
            return;
        }
        for (const instance of instances) {
            const wrapper = document.createElement('div');
            wrapper.innerHTML = this.renderCanonicalInstanceEntry(instance);
            const item = wrapper.firstElementChild;
            item.addEventListener('click', (event) => {
                if (event.target.closest('.vm-controls')) return;
                this.selectedAgent = instance.name;
                this.updateManagementDeepLink({ instance: instance.name });
                if (this.panes.has(instance.name)) this.openSessionsBlade(instance.name);
                else this.showToast(`${instance.name} has no authorized connected session`, 'info');
            });
            item.querySelectorAll('[data-instance-action]').forEach((button) => {
                button.addEventListener('click', (event) => {
                    event.stopPropagation();
                    const action = button.dataset.instanceAction;
                    const destructive = ['destroy', 'reprovision'].includes(action);
                    if (!destructive) return this.requestInstanceAction(instance, action);
                    this.showConfirmDialog({
                        title: `${action === 'destroy' ? 'Destroy' : 'Reprovision'} ${instance.name}?`,
                        message: action === 'destroy'
                            ? 'This permanently removes the runtime instance. Terminal evidence remains in the operation record.'
                            : 'This rebuilds runtime state and may interrupt active work. Persistent scoped storage is preserved by contract.',
                        confirmText: action === 'destroy' ? 'Destroy instance' : 'Reprovision',
                        confirmClass: 'danger',
                        onConfirm: () => this.requestInstanceAction(instance, action),
                    });
                });
            });
            item.querySelector('.instance-evidence')?.addEventListener('click', (event) => {
                event.stopPropagation();
                this.openActivityEvidence({ instanceId: instance.id, agentId: instance.id });
            });
            list.append(item);
        }
        this.updateVmCount();
    }

    renderCanonicalInstanceEntry(instance) {
        const state = String(instance.state || 'unknown');
        const stateClass = state.toLowerCase().replace(/[^a-z0-9_-]+/g, '-');
        const capabilities = new Set(instance.capabilities || []);
        const inFlight = [...this.instanceMutationIntents.keys()].some((key) => key.startsWith(`${instance.id}:`))
            || [...this.operations.values()].some((operation) =>
                !this.isTerminalOperation(operation)
                && (operation.target_id === instance.id || operation.target === instance.name));
        const action = (name, label, title) => capabilities.has(`instance.${name}`)
            ? `<button class="vm-ctrl-btn instance-action" data-instance-action="${name}" title="${this.esc(title)}" aria-label="${this.esc(title)} ${this.esc(instance.name)}" ${inFlight ? 'disabled' : ''}>${label}</button>`
            : '';
        const provider = instance.provider || instance.runtime || 'unknown';
        const availability = instance.agent_ready ? 'ready'
            : (instance.agent_registered ? 'registered' : (instance.operation_status || 'not ready'));
        const transport = instance.transport_posture || instance.transport || 'unknown transport';
        const identity = instance.security_posture?.label || instance.security_posture?.posture || 'identity posture unknown';
        const desired = instance.desired_state || 'not reported';
        const observed = instance.observed_state || state;
        const constraints = (instance.capability_constraints || []).map((value) => value.reason).filter(Boolean);
        const degraded = constraints.length ? `<span class="runtime-badge runtime-degraded" title="${this.esc(constraints.join(' · '))}">limited</span>` : '';
        const controls = [
            `<button class="vm-ctrl-btn instance-evidence" title="Open correlated activity evidence" aria-label="Open correlated activity evidence for ${this.esc(instance.name)}">◎</button>`,
            state === 'stopped' ? action('start', '▶', 'Start instance') : '',
            state === 'running' ? action('stop', '■', 'Stop instance gracefully') : '',
            state === 'running' ? action('restart', '↻', 'Restart instance') : '',
            action('reprovision', '⟲', 'Reprovision instance'),
            action('destroy', '✕', 'Destroy instance permanently'),
        ].join('');
        return `
            <div class="blade-item ${this.esc(stateClass)} ${instance.name === this.selectedAgent ? 'selected' : ''}"
                 data-vm-name="${escAttr(instance.name)}" data-instance-id="${escAttr(instance.id)}"
                 data-runtime="${escAttr(instance.runtime)}">
                <span class="blade-item-icon">${state === 'running' ? '●' : '○'}</span>
                <div class="blade-item-info">
                    <span class="blade-item-name">${this.esc(instance.name)}
                        <span class="runtime-badge" title="${this.esc(provider)}">${this.esc(instance.runtime || '?')}</span>${degraded}
                    </span>
                    <span class="instance-posture">desired ${this.esc(desired)} · observed ${this.esc(observed)}</span>
                    <span class="instance-posture">${this.esc(availability)} · ${this.esc(transport)} · ${this.esc(identity)}</span>
                    <span class="instance-posture">${capabilities.size} capabilities${instance.image_ref ? ` · ${this.esc(instance.image_ref)}` : ''}</span>
                </div>
                <div class="vm-controls">${controls}</div>
            </div>`;
    }

    focusAgentPane(agentId) {
        console.log('focusAgentPane called with:', agentId);
        const entry = this.panes.get(agentId);
        if (!entry || !entry.pane) {
            console.log('Entry not found or no pane:', entry);
            return;
        }

        // Update selected agent
        this.selectedAgent = agentId;
        console.log('Switching panes, total panes:', this.panes.size);

        // Hide all panes, show selected
        this.panes.forEach((e, id) => {
            if (e.pane) {
                const display = id === agentId ? 'flex' : 'none';
                console.log(`  Pane ${id}: display=${display}`);
                e.pane.style.display = display;
            }
        });

        // Update VM list selection highlight
        document.querySelectorAll('#vm-list .blade-item').forEach(el => {
            el.classList.toggle('selected', el.dataset.vmName === agentId);
        });

        // Focus terminal and re-fit
        if (entry.term) {
            entry.term.focus();
            // Re-fit after display change
            requestAnimationFrame(() => {
                try { entry.fitAddon.fit(); } catch (_) {}
            });
        }

        // Open sessions blade for this agent
        this.openSessionsBlade(agentId);
    }

    // =========================================================================
    // =========================================================================
    // Detail Inspector Modal
    // =========================================================================

    showDetailModal(title, bodyHtml) {
        const modal = document.getElementById('detail-modal');
        if (!modal) return;
        modal.querySelector('#detail-modal-title').textContent = title;
        modal.querySelector('#detail-modal-body').innerHTML = bodyHtml;
        modal.classList.remove('hidden');

        const close = () => {
            // Close any live chat SSE stream opened by the sessions panel
            // (#628) so it doesn't leak when the modal is dismissed.
            if (typeof closeActiveSessionsStream === 'function') closeActiveSessionsStream();
            modal.classList.add('hidden');
        };
        modal.querySelector('.modal-overlay').onclick = close;
        modal.querySelector('.modal-close').onclick = close;
        const onKey = (e) => {
            if (e.key === 'Escape') { close(); document.removeEventListener('keydown', onKey); }
        };
        document.addEventListener('keydown', onKey);
    }

    showAgentDetail(agentId) {
        const agent = this.agents.get(agentId);
        if (!agent) return;

        // Parse setup progress for step details
        let stepsHtml = '';
        if (agent.setup_progress_json) {
            try {
                const prog = JSON.parse(agent.setup_progress_json);
                const steps = prog.steps || {};
                stepsHtml = Object.entries(steps).map(([name, state]) => {
                    const icon = state === 'done' ? '\u2713' : state === 'failed' ? '\u2717' : '\u25CB';
                    const cls = state === 'done' ? 'done' : state === 'failed' ? 'failed' : 'active';
                    return `<span class="detail-step ${cls}">${icon} ${this.esc(name)}</span>`;
                }).join('');
            } catch (_) {}
        }

        // Build detail sections
        const sections = [];

        // === #245 AgentCard panel ===
        // Placeholder; populated asynchronously by renderAgentCardPanel().
        sections.push(`
            <section class="agentcard-panel" id="agentcard-panel-${this.esc(agent.id)}">
                <h3>A2A Identity</h3>
                <div class="agentcard-loading">Loading AgentCard…</div>
            </section>
        `);
        // === end #245 ===

        // === #628/#629 Sessions & structured-output panel ===
        // Placeholder; populated asynchronously by renderSessionsPanel().
        sections.push(`
            <section class="sessions-panel detail-section" id="sessions-panel-${this.esc(agent.id)}">
                <div class="sessions-loading">Loading sessions…</div>
            </section>
        `);
        // === end #628/#629 ===

        // Identity
        sections.push(`
            <div class="detail-section">
                <div class="detail-section-title">Identity</div>
                <div class="detail-grid">
                    <div class="detail-label">Agent ID</div><div class="detail-value">${this.esc(agent.id)}</div>
                    <div class="detail-label">Hostname</div><div class="detail-value">${this.esc(agent.hostname)}</div>
                    <div class="detail-label">IP Address</div><div class="detail-value">${this.esc(agent.ip_address)}</div>
                    <div class="detail-label">Status</div><div class="detail-value"><span class="detail-status-badge ${cssToken(agent.status)}">${this.esc(agent.status)}</span></div>
                </div>
            </div>
        `);

        // Loadout
        if (agent.loadout) {
            sections.push(`
                <div class="detail-section">
                    <div class="detail-section-title">Loadout</div>
                    <div class="detail-grid">
                        <div class="detail-label">Profile</div><div class="detail-value">${this.esc(agent.loadout)}</div>
                        <div class="detail-label">Setup Status</div><div class="detail-value">${this.esc(agent.setup_status || 'unknown')}</div>
                        ${stepsHtml ? `<div class="detail-label">Steps</div><div class="detail-value detail-steps-list">${stepsHtml}</div>` : ''}
                    </div>
                </div>
            `);
        }

        // System
        if (agent.system_info) {
            const si = agent.system_info;
            sections.push(`
                <div class="detail-section">
                    <div class="detail-section-title">System</div>
                    <div class="detail-grid">
                        <div class="detail-label">OS</div><div class="detail-value">${this.esc(si.os)}</div>
                        <div class="detail-label">Kernel</div><div class="detail-value">${this.esc(si.kernel)}</div>
                        <div class="detail-label">CPU Cores</div><div class="detail-value">${si.cpu_cores}</div>
                        <div class="detail-label">Memory</div><div class="detail-value">${this.formatBytes(si.memory_bytes)}</div>
                        <div class="detail-label">Disk</div><div class="detail-value">${this.formatBytes(si.disk_bytes)}</div>
                    </div>
                </div>
            `);
        }

        // Metrics
        if (agent.metrics) {
            const m = agent.metrics;
            sections.push(`
                <div class="detail-section">
                    <div class="detail-section-title">Metrics</div>
                    <div class="detail-grid">
                        <div class="detail-label">CPU</div><div class="detail-value">${m.cpu_percent.toFixed(1)}%</div>
                        <div class="detail-label">Memory</div><div class="detail-value">${this.formatBytes(m.memory_used_bytes)} / ${this.formatBytes(m.memory_total_bytes)}</div>
                        <div class="detail-label">Disk</div><div class="detail-value">${this.formatBytes(m.disk_used_bytes)} / ${this.formatBytes(m.disk_total_bytes)}</div>
                        <div class="detail-label">Load Avg</div><div class="detail-value">${(m.load_avg || []).map(v => v.toFixed(2)).join(', ')}</div>
                        <div class="detail-label">Uptime</div><div class="detail-value">${this.formatUptime(m.uptime_seconds)}</div>
                    </div>
                </div>
            `);
        }

        // Timestamps
        sections.push(`
            <div class="detail-section">
                <div class="detail-section-title">Connection</div>
                <div class="detail-grid">
                    <div class="detail-label">Connected</div><div class="detail-value">${new Date(agent.connected_at).toLocaleString()}</div>
                    <div class="detail-label">Last Heartbeat</div><div class="detail-value">${new Date(agent.last_heartbeat).toLocaleString()}</div>
                </div>
            </div>
        `);

        this.showDetailModal(`Agent: ${agent.id}`, sections.join(''));

        // === #245 AgentCard panel ===
        // Fire-and-forget; renderAgentCardPanel handles all errors internally.
        const panelEl = document.getElementById(`agentcard-panel-${agent.id}`);
        if (panelEl) {
            renderAgentCardPanel(agent.id, panelEl).catch(e => {
                console.error('renderAgentCardPanel failed', e);
            });
        }
        // === end #245 ===

        // === #628/#629 Sessions & structured-output panel ===
        const sessPanelEl = document.getElementById(`sessions-panel-${agent.id}`);
        if (sessPanelEl) {
            renderSessionsPanel(agent.id, sessPanelEl).catch(e => {
                console.error('renderSessionsPanel failed', e);
            });
        }
        // === end #628/#629 ===
    }

    formatBytes(bytes) {
        if (!bytes || bytes === 0) return '0 B';
        const units = ['B', 'KB', 'MB', 'GB', 'TB'];
        const i = Math.floor(Math.log(bytes) / Math.log(1024));
        return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
    }

    formatUptime(seconds) {
        if (!seconds) return '--';
        const d = Math.floor(seconds / 86400);
        const h = Math.floor((seconds % 86400) / 3600);
        const m = Math.floor((seconds % 3600) / 60);
        if (d > 0) return `${d}d ${h}h ${m}m`;
        if (h > 0) return `${h}h ${m}m`;
        return `${m}m`;
    }

    // Confirmation Dialog
    // =========================================================================

    showConfirmDialog({ title, message, confirmText, confirmClass, onConfirm }) {
        const modal = document.getElementById('confirm-modal');
        if (!modal) {
            console.error('Confirm modal not found');
            return;
        }

        modal.querySelector('.confirm-title').textContent = title;
        modal.querySelector('.confirm-message').textContent = message;

        const confirmBtn = modal.querySelector('.confirm-btn');
        confirmBtn.textContent = confirmText;
        confirmBtn.className = `confirm-btn ${confirmClass}`;

        // Set up event handlers
        const handleConfirm = () => {
            onConfirm();
            this.hideConfirmDialog();
        };

        const handleCancel = () => {
            this.hideConfirmDialog();
        };

        // Remove old listeners
        const newConfirmBtn = confirmBtn.cloneNode(true);
        confirmBtn.parentNode.replaceChild(newConfirmBtn, confirmBtn);

        const cancelBtn = modal.querySelector('.cancel-btn');
        const newCancelBtn = cancelBtn.cloneNode(true);
        cancelBtn.parentNode.replaceChild(newCancelBtn, cancelBtn);

        // Attach new listeners
        newConfirmBtn.addEventListener('click', handleConfirm);
        newCancelBtn.addEventListener('click', handleCancel);

        // Show modal
        modal.classList.remove('hidden');
    }

    hideConfirmDialog() {
        const modal = document.getElementById('confirm-modal');
        if (modal) {
            modal.classList.add('hidden');
        }
    }

    // =========================================================================
    // UI helpers
    // =========================================================================

    updateConnectionStatus(state) {
        const el = document.getElementById('connection-status');
        const text = el.querySelector('.status-text');
        const normalized = state === true ? 'ready' : (state === false ? 'disconnected' : state);
        if (normalized === 'ready') {
            el.className = 'status-connected';
            text.textContent = 'Connected';
        } else if (normalized === 'connecting' || normalized === 'negotiating') {
            el.className = 'status-connecting';
            text.textContent = normalized === 'negotiating' ? 'Negotiating' : 'Connecting';
        } else if (normalized === 'reconnecting') {
            el.className = 'status-connecting';
            text.textContent = 'Reconnecting';
        } else if (normalized === 'degraded') {
            el.className = 'status-degraded';
            text.textContent = 'Paused';
        } else if (normalized === 'terminal') {
            el.className = 'status-terminal';
            text.textContent = 'Action required';
        } else {
            el.className = 'status-disconnected';
            text.textContent = 'Disconnected';
        }
        const retry = document.getElementById('connection-retry');
        if (retry) retry.hidden = normalized !== 'terminal';
    }

    updateAgentCount() {
        document.getElementById('agent-count').textContent =
            `${this.agents.size} agent${this.agents.size !== 1 ? 's' : ''}`;
    }

    updateVmCount() {
        const vmCountEl = document.getElementById('vm-count');
        if (vmCountEl) {
            const source = this.instances;
            const total = source.size;
            const running = Array.from(source.values()).filter(instance =>
                String(instance.state).toLowerCase() === 'running'
            ).length;
            vmCountEl.textContent = running === total
                ? `${total} instance${total !== 1 ? 's' : ''}`
                : `${running}/${total} instances`;
        }
    }

    updateEmptyState() {
        const empty = document.getElementById('no-agents');
        if (empty) {
            empty.style.display = this.panes.size === 0 ? 'flex' : 'none';
        }
    }

    async pollAiwgStatus() {
        try {
            const resp = (await ApiClient.request('/api/v1/aiwg/status')).response;
            if (!resp.ok) return;
            const data = await resp.json();
            const el = document.getElementById('aiwg-status');
            if (!el) return;

            if (!data.configured) {
                el.classList.add('hidden');
                return;
            }

            el.classList.remove('hidden');
            const connected = data.connected;
            el.className = `aiwg-status ${connected ? 'aiwg-connected' : 'aiwg-disconnected'}`;

            const label = el.querySelector('.aiwg-status-text');
            if (label) {
                const id = data.sandbox_id ? data.sandbox_id.replace('sandbox-', '') : '';
                label.textContent = connected ? `AIWG ${id}` : 'AIWG offline';
                const title = [data.endpoint || ''];
                const crashLoop = data.mission_crash_loop;
                if (crashLoop) {
                    title.push(`Mission quarantine: ${crashLoop.quarantined_count || 0}`);
                    const quarantined = Array.isArray(crashLoop.missions)
                        ? crashLoop.missions.filter((m) => m && m.state === 'quarantined')
                        : [];
                    for (const mission of quarantined.slice(0, 3)) {
                        const loop = mission.crash_loop || {};
                        const reason = loop.last_failure_reason || 'no reason recorded';
                        title.push(`${mission.mission_id}: ${loop.consecutive_failures || 0} failures - ${reason}`);
                    }
                }
                label.title = title.filter(Boolean).join('\n');
            }
        } catch (_) {}
    }

    async triggerAiwgReconnect() {
        const btn = document.getElementById('aiwg-reconnect-btn');
        if (btn) { btn.style.opacity = '0.3'; btn.disabled = true; }
        try {
            await ApiClient.request('/api/v1/aiwg/reconnect', { method: 'POST' });
            this.showToast('AIWG reconnect triggered', 'info');
        } catch (_) {
            this.showToast('Failed to trigger reconnect', 'error');
        } finally {
            setTimeout(() => {
                if (btn) { btn.style.opacity = ''; btn.disabled = false; }
            }, 2000);
        }
    }

    async fetchAgents() {
        try {
            const resp = (await ApiClient.request('/api/v1/agents')).response;
            const data = await resp.json();
            if (data.agents) {
                this.handleAgentList({ agents: data.agents });
            }
        } catch (e) {
            console.error('Failed to fetch agents:', e);
        }
    }

    // =========================================================================
    // Blade Navigation (VMs → Sessions)
    // =========================================================================

    setupBladeNav() {
        // Back button on sessions blade
        const backBtn = document.querySelector('#sessions-blade .blade-back');
        if (backBtn) {
            backBtn.addEventListener('click', () => this.closeSessionsBlade());
        }

        // Reconcile button
        const reconcileBtn = document.getElementById('reconcile-btn');
        if (reconcileBtn) {
            reconcileBtn.addEventListener('click', () => {
                if (this.selectedVmForSessions) {
                    this.triggerReconciliation(this.selectedVmForSessions);
                }
            });
        }

        // Create VM button
        const createBtn = document.getElementById('create-vm-btn');
        if (createBtn) {
            createBtn.addEventListener('click', () => this.showCreateVmModal());
        }

        // Create Session button
        const createSessionBtn = document.getElementById('create-session-btn');
        if (createSessionBtn) {
            createSessionBtn.addEventListener('click', () => this.showCreateSessionModal());
        }

        this.setupCreateVmModal();
        this.setupCreateSessionModal();
    }

    openSessionsBlade(vmName) {
        this.selectedVmForSessions = vmName;

        const blade = document.getElementById('sessions-blade');
        const title = document.getElementById('sessions-blade-title');

        if (blade) {
            blade.classList.remove('hidden');
            blade.classList.remove('closing');
        }
        if (title) {
            title.textContent = vmName;
        }

        // Show loading
        const list = document.getElementById('sessions-list');
        if (list) {
            list.innerHTML = '<div class="blade-loading">Loading...</div>';
        }

        this.fetchSessionsForBlade(vmName);
    }

    closeSessionsBlade() {
        const blade = document.getElementById('sessions-blade');
        if (blade) {
            blade.classList.add('closing');
            setTimeout(() => {
                blade.classList.add('hidden');
                blade.classList.remove('closing');
            }, 150);
        }
        this.selectedVmForSessions = null;
    }

    fetchSessionsForBlade(vmName) {
        this.send({
            type: 'list_sessions',
            agent_id: vmName
        });
    }

    handleSessionsList(msg) {
        const vmName = msg.agent_id;
        if (!vmName) return;

        const sessions = msg.sessions || [];
        this.vmSessions.set(vmName, sessions);

        // Update blade if showing this VM
        if (this.selectedVmForSessions === vmName) {
            this.renderSessionsBlade(sessions);
        }

        // Update VM list badge
        this.updateVmSessionBadge(vmName, sessions.length);

        // Startup attach: triggered by discoverAndAttach on connect/refresh
        if (this.pendingStartupAttach.has(vmName)) {
            this.pendingStartupAttach.delete(vmName);
            const entry = this.panes.get(vmName);
            if (!entry || !entry.term) return;

            const interactive = sessions.find(s => s.session_type === 'interactive');
            if (interactive) {
                // Existing session found — attach via formal protocol (server replays ring buffer)
                this.attachExistingSession(vmName, interactive);
            } else {
                // No interactive session running — start a fresh one
                this.startShell(vmName);
            }
        }
    }

    // ── Persistent last-seen seq (#144) ─────────────────────────────
    //
    // Persist to localStorage so a reconnect (hard refresh, WS drop, tab
    // restore) can request incremental replay instead of replaying the
    // entire ring. Server-side keyframe injection (#145) ensures the
    // server clamps replay to a safe starting point even when our
    // stored seq is well past the last keyframe.

    setLastSeq(sessionId, seq) {
        this.lastSeqPerSession.set(sessionId, seq);
        try {
            localStorage.setItem(`sandbox_seq_${sessionId}`, String(seq));
        } catch (_) { /* private mode / quota — no-op */ }
    }

    getLastSeq(sessionId) {
        if (this.lastSeqPerSession.has(sessionId)) {
            return this.lastSeqPerSession.get(sessionId);
        }
        try {
            const v = localStorage.getItem(`sandbox_seq_${sessionId}`);
            if (v !== null) {
                const n = parseInt(v, 10);
                if (Number.isFinite(n)) {
                    this.lastSeqPerSession.set(sessionId, n);
                    return n;
                }
            }
        } catch (_) { /* no-op */ }
        return null;
    }

    forgetLastSeq(sessionId) {
        this.lastSeqPerSession.delete(sessionId);
        try {
            localStorage.removeItem(`sandbox_seq_${sessionId}`);
        } catch (_) { /* no-op */ }
    }

    // Join an existing session using the formal protocol: server replays
    // from the last-seen seq onward (#144 + #145). On a fresh tab with no
    // stored seq we ask the server to default to its most recent keyframe
    // (`replay_from=null`); the server emits a Keyframe payload containing
    // a full repaint, then any frames after it.
    attachExistingSession(agentId, session) {
        const entry = this.panes.get(agentId);
        if (!entry) return;
        this.sessionIdToAgentId.set(session.session_id, agentId);
        // Route keyboard input to this session's PTY (term.onData reads
        // shellCommandIds — without this, a client that joined an existing
        // session has nowhere to send keystrokes and the terminal looks dead).
        //
        // NOTE: when the v2 pty-ws.v1 path activates below, we skip this
        // mapping. The v2 client has its own onData listener that sends
        // `pty.session_input`; populating shellCommandIds would cause the
        // v1 term.onData handler to ALSO forward each keystroke as
        // `send_input` on the management bus, double-shipping input.
        const useLegacyTransport = _ptyV2PreferLegacy() ||
            typeof PtyWsV1Client === 'undefined' || !entry.term;
        if (session.command_id && useLegacyTransport) {
            this.shellCommandIds.set(agentId, session.command_id);
            this.activeCommandIds.set(agentId, session.command_id);
            // Mark this command_id as fed by the formal SessionFrame path so
            // handleOutput skips rendering its legacy duplicates.
            this.formallyJoinedCommandIds.add(session.command_id);
        }
        // === #247 PTY pty-ws.v1 attach path ===
        // When v2 is enabled (default), bypass the v1 join_session bus
        // and open a per-session WebSocket against /agents/{instance_id}
        // /sessions/{session_id}/attach. Falls back to v1 when the
        // toggle is on, when no instance_id can be derived, or when
        // PtyWsV1Client is unavailable.
        if (!_ptyV2PreferLegacy() && typeof PtyWsV1Client !== 'undefined' && entry.term) {
            const agent = this.agents.get(agentId);
            const md = (agent && agent.metadata) || {};
            const runtime = md.runtime && typeof md.runtime === 'object' ? md.runtime : md;
            const instanceId =
                (agent && (agent.instance_id || agent.instanceId)) ||
                runtime.instance_id ||
                md['runtime.instance_id'] ||
                agentId; // dev fallback: agent id == instance id
            const replayFrom = this.getLastSeq(session.session_id);
            console.log(`[attach v2] agent=${agentId} instance=${instanceId} session=${session.session_id} replay_from=${replayFrom}`);
            // Close any prior v2 client on this pane.
            if (entry.ptyV2Client && typeof entry.ptyV2Client.leave === 'function') {
                try { entry.ptyV2Client.leave(); } catch (_) {}
            }
            const client = openPtyV2Session({
                pane: entry.pane,
                agentId,
                instanceId,
                sessionId: session.session_id,
                terminal: entry.term,
                replayFromSeq: replayFrom,
                wsUrlOverride: _ptyV2MaterializeListedUrl(session.pty_ws_url),
            });
            entry.ptyV2Client = client;
            this.updateShellButton(agentId, true);
            return;
        }
        // === end #247 v2 path; fall through to legacy v1 ===
        const lastSeq = this.getLastSeq(session.session_id);
        // If we have a stored seq, request only the delta. The server's
        // ring-floor clamp + keyframe-emission logic handles the cases
        // where our stored seq is older than the ring or past the last
        // keyframe (it'll still send a fresh keyframe + delta).
        const replayFrom = lastSeq != null ? lastSeq + 1 : null;
        // #188 Section B — log every attach so a #180 recurrence leaves
        // a trace in devtools. Pairs with the server-side join_session log.
        console.log(`[attach] agent=${agentId} session=${session.session_id} replay_from=${replayFrom} command=${session.command_id}`);
        if (entry.term) {
            // ALWAYS reset xterm's state machine before joining/rejoining.
            // Without this, cursor position, alt-screen mode, scroll region,
            // and SGR attrs carry over from before the disconnect; tmux's
            // bytes assume a clean starting state, and the cumulative drift
            // produces stacked status bars + overlapping output (#180).
            // term.reset() also implies clear, so the previous behavior of
            // "preserve visible state" is intentionally dropped — the brief
            // flash is preferable to corrupted rendering.
            entry.term.reset();
            entry.term.write(`\x1b[2m[replaying session history…]\x1b[0m\r`);
        }
        const msg = {
            type: 'join_session',
            session_id: session.session_id,
            role: 'observer',
        };
        if (replayFrom !== null) {
            msg.replay_from = replayFrom;
        }
        this.send(msg);
    }

    handleSessionJoined(msg) {
        // msg: { type, session_id, role, current_seq }
        const agentId = this.sessionIdToAgentId.get(msg.session_id);
        if (!agentId) return;
        const entry = this.panes.get(agentId);
        if (!entry) return;
        this.updateShellButton(agentId, true);
    }

    handleSessionFrame(msg) {
        // msg: { type, session_id, seq, ts, kind, ... }
        const agentId = this.sessionIdToAgentId.get(msg.session_id);
        if (!agentId) return;
        const entry = this.panes.get(agentId);
        if (!entry || !entry.term) return;

        // Track sequence for incremental reconnect (#144). Persists to
        // localStorage so a hard refresh / tab restore can request only
        // the delta on next attach.
        if (msg.seq != null) {
            this.setLastSeq(msg.session_id, msg.seq);
        }

        switch (msg.kind) {
            case 'keyframe': {
                // Same wire shape as output — full-repaint snapshot
                // suitable as a safe replay starting point (#145). Write
                // it to the terminal exactly like output; the server
                // emits SGR/cursor sequences in `data` so the visible
                // state is reproduced even mid-session.
                const raw = atob(msg.data);
                const bytes = new Uint8Array(raw.length);
                for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
                entry.term.write(bytes);
                break;
            }
            case 'output': {
                // data is base64-encoded PTY bytes
                const raw = atob(msg.data);
                // Convert to Uint8Array for xterm
                const bytes = new Uint8Array(raw.length);
                for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
                entry.term.write(bytes);
                // Also buffer for session thumbnail
                if (msg.session_id) {
                    let buf = this.sessionBuffers.get(msg.session_id);
                    if (!buf) {
                        buf = { text: '', raw: '', dirty: true };
                        this.sessionBuffers.set(msg.session_id, buf);
                    }
                    buf.raw += raw;
                    if (buf.raw.length > 32768) buf.raw = buf.raw.slice(-32768);
                    buf.dirty = true;
                }
                break;
            }
            case 'closed':
                entry.term.writeln(`\r\n\x1b[2m[session closed]\x1b[0m`);
                this.updateShellButton(agentId, false);
                // Drop persisted seq for terminated session (#144).
                if (msg.session_id) this.forgetLastSeq(msg.session_id);
                {
                    const closedCmdId = this.shellCommandIds.get(agentId);
                    if (closedCmdId) this.formallyJoinedCommandIds.delete(closedCmdId);
                }
                break;
            case 'error':
                entry.term.writeln(`\r\n\x1b[31m[session error: ${msg.message}]\x1b[0m`);
                break;
        }
    }

    // Call list_sessions first; attach to existing interactive session or start fresh.
    discoverAndAttach(agentId) {
        this.pendingStartupAttach.add(agentId);
        this.send({ type: 'list_sessions', agent_id: agentId });
    }

    updatePaneSessionLabel(agentId, sessionName) {
        const entry = this.panes.get(agentId);
        if (!entry) return;
        const label = entry.pane.querySelector('.pane-session-label');
        if (!label) return;
        if (sessionName) {
            label.textContent = `· ${sessionName}`;
            label.style.display = '';
        } else {
            label.style.display = 'none';
        }
    }

    updateVmSessionBadge(vmName, count) {
        const badge = document.querySelector(`.blade-item[data-vm-name="${vmName}"] .blade-item-badge`);
        if (badge) {
            badge.textContent = count;
            badge.style.display = count > 0 ? '' : 'none';
        }
    }

    renderSessionsBlade(sessions) {
        const list = document.getElementById('sessions-list');
        if (!list) return;

        if (!sessions || sessions.length === 0) {
            list.innerHTML = '<div class="blade-placeholder">No active sessions</div>';
            return;
        }

        list.innerHTML = sessions.map(session => {
            const typeClass = cssToken(session.session_type || 'background');
            const name = session.session_name || session.command_id?.slice(0, 12) || 'session';
            const membership = session.membership || {};
            const liveness = session.liveness || {};
            const controllers = Array.isArray(membership.controllers) ? membership.controllers : [];
            const observers = Array.isArray(membership.observers) ? membership.observers : [];
            const leaseText = controllers.length > 0 ? `controller: ${controllers[0].slice(0, 8)}` : 'controller available';
            const replaySeq = typeof liveness.replay_newest_seq === 'number'
                ? `seq ${liveness.replay_newest_seq}`
                : 'no replay yet';

            // Pre-populate thumbnail from existing buffer if available
            const buf = this.sessionBuffers.get(session.command_id);
            const thumbText = buf ? this.esc(buf.text.split('\n').slice(-6).join('\n')) : '';

            return `
                <div class="session-card" data-session-id="${escAttr(session.command_id)}">
                    <div class="session-thumb" data-command-id="${escAttr(session.command_id)}">
                        <pre class="thumb-term">${thumbText}</pre>
                    </div>
                    <div class="session-card-info">
                        <span class="session-card-name">${this.esc(name)}</span>
                        <span class="session-card-type ${typeClass}">${this.esc(typeClass.slice(0, 3))}</span>
                        <span class="session-card-meta">${this.esc(leaseText)}</span>
                        <span class="session-card-meta">${this.esc(String(observers.length))} observers · ${this.esc(replaySeq)}</span>
                        <button class="session-card-activity" title="Open correlated activity evidence" aria-label="Open activity evidence for ${this.esc(name)}">◎</button>
                        <button class="session-card-kill" title="Kill" aria-label="Kill ${this.esc(name)} session">✕</button>
                    </div>
                </div>
            `;
        }).join('');

        // Attach handlers
        const vmName = this.selectedVmForSessions;
        list.querySelectorAll('.session-card').forEach(card => {
            const sessionId = card.dataset.sessionId;

            card.addEventListener('click', (e) => {
                if (e.target.closest('.session-card-kill, .session-card-activity')) return;
                this.connectToSession(vmName, sessionId);
            });

            card.querySelector('.session-card-activity')?.addEventListener('click', (e) => {
                e.stopPropagation();
                const instance = this.instances.get(vmName);
                this.openActivityEvidence({
                    instanceId: instance?.id || vmName,
                    agentId: this.agents.get(vmName)?.id || instance?.id || vmName,
                    sessionId,
                });
            });

            card.querySelector('.session-card-kill')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.killSession(vmName, sessionId);
            });
        });

        // Auto-select session:
        // - If only one session, connect to it automatically
        // - If multiple sessions, try last selected, otherwise first
        if (sessions.length === 1) {
            this.connectToSession(vmName, sessions[0].command_id);
        } else if (sessions.length > 1) {
            const lastId = this.lastSelectedSession.get(vmName);
            const validLast = lastId && sessions.find(s => s.command_id === lastId);
            if (validLast) {
                this.connectToSession(vmName, lastId);
            } else {
                // Default to first session
                this.connectToSession(vmName, sessions[0].command_id);
            }
        }
    }

    connectToSession(vmName, sessionId) {
        // Find session name from our cached sessions
        const sessions = this.vmSessions.get(vmName) || [];
        const session = sessions.find(s => s.command_id === sessionId);
        const sessionName = session?.name || session?.session_name || sessionId.slice(0, 12);

        // Remember this as the last selected session for this VM
        this.lastSelectedSession.set(vmName, sessionId);

        // Make sure we have a pane for this VM and focus it
        let entry = this.panes.get(vmName);
        if (!entry) {
            // Need to focus/select this agent first
            this.focusAgentPane(vmName);
            entry = this.panes.get(vmName);
        }
        if (!entry) {
            this.showToast(`No terminal pane for ${vmName}`, 'error');
            return;
        }

        // Ensure this pane is visible/focused
        if (this.selectedAgent !== vmName) {
            this.focusAgentPane(vmName);
        }

        // Mark as active
        document.querySelectorAll('.session-card').forEach(c => c.classList.remove('active'));
        const card = document.querySelector(`.session-card[data-session-id="${sessionId}"]`);
        if (card) card.classList.add('active');

        // Get terminal size
        let cols = 80, rows = 24;
        if (entry?.term) {
            cols = entry.term.cols;
            rows = entry.term.rows;
        }

        // Send attach message and track locally for client-side output routing
        this.send({
            type: 'attach_session',
            agent_id: vmName,
            session_name: sessionName,
            cols,
            rows
        });

        // Track attached session and route input to it
        if (entry) {
            entry.attachedSession = sessionId;
            entry.attachedSessionName = sessionName;
            // Update shell command ID so keyboard input routes to this session
            this.shellCommandIds.set(vmName, sessionId);
            // Clear terminal and replay buffered output for this session
            if (entry.term) {
                entry.term.clear();
                // Replay raw buffered output from this session
                const buf = this.sessionBuffers.get(sessionId);
                if (buf && buf.raw) {
                    entry.term.write(buf.raw);
                }
                entry.term.focus();
            }
        }

        this.showToast(`Attached to ${sessionName}`, 'success');
    }

    detachSession(vmName) {
        const entry = this.panes.get(vmName);
        if (!entry?.attachedSessionName) return;

        const sessionName = entry.attachedSessionName;
        this.send({
            type: 'detach_session',
            agent_id: vmName,
            session_name: sessionName
        });

        entry.attachedSession = null;
        entry.attachedSessionName = null;

        // Clear active state from cards
        document.querySelectorAll('.session-card').forEach(c => c.classList.remove('active'));

        this.showToast('Detached from session', 'info');
    }

    killSession(vmName, commandId) {
        // Look up session_name from cached sessions (server expects session_name)
        const sessions = this.vmSessions.get(vmName) || [];
        const session = sessions.find(s => s.command_id === commandId);
        const sessionName = session?.session_name || commandId;

        console.log('killSession:', { vmName, commandId, sessions, session, sessionName });

        this.send({
            type: 'kill_session',
            agent_id: vmName,
            session_name: sessionName,
        });
        this.showToast(`Killing session "${sessionName}"...`, 'info');
    }

    triggerReconciliation(vmName) {
        this.send({
            type: 'trigger_reconciliation',
            agent_id: vmName
        });
        this.showToast(`Reconciling ${vmName}...`, 'info');
    }

    // =========================================================================
    // Create Session
    // =========================================================================

    setupCreateSessionModal() {
        const modal = document.getElementById('create-session-modal');
        if (!modal) return;

        const overlay = modal.querySelector('.modal-overlay');
        const closeBtn = modal.querySelector('.modal-close');
        const cancelBtn = modal.querySelector('.cancel-btn');
        const form = document.getElementById('create-session-form');
        const typeSelect = document.getElementById('session-type');

        const closeModal = () => {
            modal.classList.add('hidden');
            form.reset();
            this.updateSessionCommandVisibility();
        };

        overlay.addEventListener('click', closeModal);
        closeBtn.addEventListener('click', closeModal);
        cancelBtn.addEventListener('click', closeModal);

        // Show/hide command field based on session type
        typeSelect.addEventListener('change', () => this.updateSessionCommandVisibility());

        form.addEventListener('submit', (e) => {
            e.preventDefault();
            this.handleCreateSession();
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !modal.classList.contains('hidden')) {
                closeModal();
            }
        });
    }

    updateSessionCommandVisibility() {
        const typeSelect = document.getElementById('session-type');
        const cmdGroup = document.getElementById('session-command-group');
        const cmdInput = document.getElementById('session-command');
        if (!typeSelect || !cmdGroup) return;

        const isInteractive = typeSelect.value === 'interactive';
        cmdGroup.style.display = isInteractive ? 'none' : '';
        cmdInput.required = !isInteractive;
    }

    showCreateSessionModal() {
        const vmName = this.selectedVmForSessions;
        if (!vmName) {
            this.showToast('Select a VM first', 'error');
            return;
        }

        const modal = document.getElementById('create-session-modal');
        const vmLabel = document.getElementById('session-modal-vm');
        if (vmLabel) vmLabel.textContent = vmName;

        // Reset and show
        document.getElementById('create-session-form').reset();
        this.updateSessionCommandVisibility();
        modal.classList.remove('hidden');
        document.getElementById('session-name').focus();
    }

    handleCreateSession() {
        const vmName = this.selectedVmForSessions;
        if (!vmName) return;

        const nameInput = document.getElementById('session-name');
        const name = nameInput.value.trim();
        const sessionType = document.getElementById('session-type').value;
        const workingDir = document.getElementById('session-working-dir')?.value.trim() || null;
        const commandRaw = document.getElementById('session-command').value.trim();

        if (!name) {
            this.showToast('Session name is required', 'error');
            return;
        }

        // For non-interactive types, command is required
        if (sessionType !== 'interactive' && !commandRaw) {
            this.showToast('Command is required for this session type', 'error');
            return;
        }

        // Parse command string into command + args
        let command = '';
        let args = [];
        if (commandRaw) {
            const parts = commandRaw.match(/(?:[^\s"]+|"[^"]*")+/g) || [];
            command = (parts[0] || '').replace(/^"|"$/g, '');
            args = parts.slice(1).map(a => a.replace(/^"|"$/g, ''));
        }

        // Get terminal size from main pane
        const entry = this.panes.get(vmName);
        const cols = entry?.term?.cols || 80;
        const rows = entry?.term?.rows || 24;

        this.send({
            type: 'create_session',
            agent_id: vmName,
            session_name: name,
            session_type: sessionType,
            command,
            args,
            working_dir: workingDir,
            cols,
            rows,
        });

        // Close modal
        document.getElementById('create-session-modal').classList.add('hidden');
        document.getElementById('create-session-form').reset();
        this.showToast(`Creating session "${name}"...`, 'info');
    }

    handleSessionCreated(msg) {
        this.showToast(`Session "${msg.session_name}" created`, 'success');

        // Refresh sessions blade if showing this agent
        if (msg.agent_id && this.selectedVmForSessions === msg.agent_id) {
            this.fetchSessionsForBlade(msg.agent_id);
        }

        // Auto-attach to interactive sessions
        if (msg.session_type === 'interactive' && msg.command_id) {
            const entry = this.panes.get(msg.agent_id);
            if (entry) {
                // Track as shell command so output routes to main terminal
                this.shellCommandIds.set(msg.agent_id, msg.command_id);
                entry.attachedSession = msg.command_id;
                entry.attachedSessionName = msg.session_name;

                // Clear and focus terminal
                if (entry.term) {
                    entry.term.clear();
                    entry.term.focus();
                }
            }
        }
    }

    formatDuration(seconds) {
        if (seconds < 60) return `${seconds}s`;
        if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
        return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
    }

    // =========================================================================
    // OAuth
    // =========================================================================

    detectOAuth(agentId, commandId, text) {
        for (const pattern of OAUTH_PATTERNS) {
            pattern.lastIndex = 0;
            const match = pattern.exec(text);
            if (match) {
                const url = (match[1] || match[0]).replace(/[.,;:!?'")\]}>]+$/, '');
                this.showOAuthModal({ agentId, commandId, url, message: text.trim() });
                return;
            }
        }
        for (const pattern of DEVICE_CODE_PATTERNS) {
            pattern.lastIndex = 0;
            const match = pattern.exec(text);
            if (match) {
                this.showToast(`Device code: ${match[1]}`, 'info');
            }
        }
    }

    showOAuthModal(prompt) {
        this.currentOAuthPrompt = prompt;
        document.getElementById('oauth-message').textContent =
            prompt.message.length > 200 ? prompt.message.substring(0, 200) + '...' : prompt.message;
        document.getElementById('oauth-link').href = prompt.url;
        document.getElementById('oauth-input').value = '';
        document.getElementById('oauth-modal').classList.remove('hidden');
        this.showToast(`Authorization required for ${prompt.agentId}`, 'info');
    }

    hideOAuthModal() {
        document.getElementById('oauth-modal').classList.add('hidden');
        this.currentOAuthPrompt = null;
    }

    submitOAuthInput() {
        const value = document.getElementById('oauth-input').value.trim();
        if (!value || !this.currentOAuthPrompt) return;
        const { agentId, commandId } = this.currentOAuthPrompt;
        this.send({
            type: 'send_input',
            agent_id: agentId,
            command_id: commandId || this.activeCommandIds.get(agentId),
            data: value + '\n',
        });
        this.hideOAuthModal();
    }

    // =========================================================================
    // Fleet and Celld workspaces (#808)
    // =========================================================================

    setupManagementWorkspaces() {
        document.querySelectorAll('[data-workspace]').forEach((button) => {
            button.addEventListener('click', () => this.switchManagementWorkspace(button.dataset.workspace));
        });
        document.getElementById('fleet-refresh')?.addEventListener('click', () => this.fetchFleetInventory());
        document.getElementById('fleet-preview-reconcile')?.addEventListener('click', () => this.previewFleetReconcile());
        document.getElementById('fleet-apply-reconcile')?.addEventListener('click', () => this.applyFleetReconcile());
        document.getElementById('celld-refresh')?.addEventListener('click', () => this.fetchCelldStatus());
        document.getElementById('celld-get-cell')?.addEventListener('click', () => this.fetchCelldCell());
        document.getElementById('celld-preview-reconcile')?.addEventListener('click', () => this.previewCelldReconcile());
        document.getElementById('celld-apply-reconcile')?.addEventListener('click', () => this.applyCelldReconcile());
        document.getElementById('celld-preview-command')?.addEventListener('click', () => this.previewCelldCommand());
        document.getElementById('celld-apply-command')?.addEventListener('click', () => this.applyCelldCommand());
        document.querySelectorAll('.celld-review-action').forEach((button) => {
            button.addEventListener('click', () => this.runCelldReview(button.dataset.celldReview));
        });
        document.getElementById('celld-plan-upgrade')?.addEventListener('click', () => this.planCelldUpgrade());
        document.getElementById('celld-cancel-plan')?.addEventListener('click', () => this.cancelCelldPlan());
        document.getElementById('config-refresh')?.addEventListener('click', () => this.fetchConfigurationWorkspace());
        document.getElementById('startup-profile-new')?.addEventListener('click', () => this.newStartupProfile());
        document.getElementById('startup-profile-review')?.addEventListener('click', () => this.reviewStartupProfile());
        document.getElementById('startup-profile-apply')?.addEventListener('click', () => this.applyStartupProfile());
        document.getElementById('startup-profile-delete')?.addEventListener('click', () => this.deleteStartupProfile());
        document.getElementById('config-loadout-review')?.addEventListener('click', () => this.reviewConfigLoadout());
        document.getElementById('config-loadout-apply')?.addEventListener('click', () => this.applyConfigLoadout());
        document.getElementById('storage-scope')?.addEventListener('change', () => this.updateStorageAuthority());
        document.getElementById('storage-path')?.addEventListener('input', () => this.renderStorageBreadcrumbs());
        document.getElementById('storage-read')?.addEventListener('click', () => this.readStoragePath());
        document.getElementById('storage-review-write')?.addEventListener('click', () => this.reviewStorageWrite());
        document.getElementById('storage-apply-write')?.addEventListener('click', () => this.applyStorageWrite());
        document.getElementById('storage-delete')?.addEventListener('click', () => this.deleteStorageObject());
        document.getElementById('acceleration-provider')?.addEventListener('change', () => this.renderAccelerationActions());
        document.getElementById('acceleration-review')?.addEventListener('click', () => this.reviewAcceleration());
        document.getElementById('acceleration-apply')?.addEventListener('click', () => this.applyAcceleration());
        document.getElementById('mcp-refresh')?.addEventListener('click', () => this.fetchMcpDiscovery());
        document.getElementById('access-refresh')?.addEventListener('click', () => this.fetchAccessWorkspace());
        document.getElementById('credential-lease-credential')?.addEventListener('change', () => this.updateCredentialLeaseUses());
        document.getElementById('credential-lease-review')?.addEventListener('click', () => this.reviewCredentialLease());
        document.getElementById('credential-lease-apply')?.addEventListener('click', () => this.applyCredentialLease());
        document.getElementById('ssh-lease-review')?.addEventListener('click', () => this.reviewSshLease());
        document.getElementById('ssh-lease-apply')?.addEventListener('click', () => this.applySshLease());
        document.getElementById('access-audit-refresh')?.addEventListener('click', () => this.fetchAccessAudit());
        for (const id of [
            'credential-lease-credential', 'credential-lease-agent', 'credential-lease-instance',
            'credential-lease-session', 'credential-lease-use', 'credential-lease-ttl',
        ]) document.getElementById(id)?.addEventListener('input', () => this.discardCredentialLeaseReview());
        for (const id of [
            'ssh-lease-instance', 'ssh-lease-principal', 'ssh-lease-mode',
            'ssh-lease-public-key', 'ssh-lease-ttl',
        ]) document.getElementById(id)?.addEventListener('input', () => this.discardSshLeaseReview(false));
        const requested = new URLSearchParams(window.location.search).get('workspace');
        if (['fleet', 'celld', 'config', 'access'].includes(requested)) this.switchManagementWorkspace(requested);
    }

    switchManagementWorkspace(workspace = 'console') {
        const selected = ['fleet', 'celld', 'config', 'access'].includes(workspace) ? workspace : 'console';
        document.body.classList.toggle('workspace-fleet', selected === 'fleet');
        document.body.classList.toggle('workspace-celld', selected === 'celld');
        document.body.classList.toggle('workspace-config', selected === 'config');
        document.body.classList.toggle('workspace-access', selected === 'access');
        document.getElementById('fleet-workspace')?.classList.toggle('hidden', selected !== 'fleet');
        document.getElementById('celld-workspace')?.classList.toggle('hidden', selected !== 'celld');
        document.getElementById('config-workspace')?.classList.toggle('hidden', selected !== 'config');
        document.getElementById('access-workspace')?.classList.toggle('hidden', selected !== 'access');
        document.querySelectorAll('[data-workspace]').forEach((button) => {
            button.classList.toggle('active', button.dataset.workspace === selected);
        });
        const url = new URL(window.location.href);
        if (selected === 'console') url.searchParams.delete('workspace');
        else url.searchParams.set('workspace', selected);
        history.replaceState(null, '', url);
        if (selected === 'fleet') this.fetchFleetInventory();
        if (selected === 'celld') this.fetchCelldStatus();
        if (selected === 'config') this.fetchConfigurationWorkspace();
        if (selected === 'access') this.fetchAccessWorkspace();
        else this.discardSshLeaseReview(true);
    }

    async managementRequest(path, options = {}) {
        const boundary = await window.ManagementUIReady;
        if (!this.managementTransport) this.managementTransport = new boundary.HttpTransport();
        try {
            return await this.managementTransport.request(path, ApiClient.withMutationIntent(options));
        } catch (error) {
            // Preserve the existing dashboard recovery contract while sharing
            // normalized transport outcomes with the modular domain clients.
            if (error instanceof boundary.UnknownMutationOutcomeError) {
                throw new UnknownMutationOutcomeError({
                    method: error.method, path: error.url,
                    idempotencyKey: error.idempotencyKey, cause: error,
                });
            }
            if (error instanceof boundary.HttpOutcomeError) {
                error.status = error.outcome.status;
                error.retryAfter = error.outcome.retryAfterMs == null ? null : String(error.outcome.retryAfterMs / 1000);
            }
            throw error;
        }
    }

    async managementJson(path, options = {}) {
        const outcome = await this.managementRequest(path, { ...options, expectJson: true });
        return outcome.body;
    }

    async trackedMutation(path, options, metadata) {
        const intentKey = metadata.intentKey || ApiClient.newIntentId();
        const retryRequest = {
            path,
            method: options.method || 'POST',
            headers: options.headers,
            body: options.body,
        };
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey,
            target: metadata.target,
            targetId: metadata.targetId,
            kind: metadata.kind,
            reconciliationBefore: metadata.reconciliationBefore,
            retryRequest,
        });
        try {
            const outcome = await this.managementRequest(path, {
                ...options,
                idempotencyKey: intentKey,
                owner: metadata.owner || `mutation:${metadata.kind}:${metadata.target}`,
            });
            let operation = null;
            if (outcome.kind === 'accepted') {
                operation = this.trackCanonicalOperation({
                    ...(outcome.body || {}),
                    operation_id: outcome.operationId,
                    trace_id: outcome.traceId,
                    request_id: outcome.requestId,
                    idempotency_replayed: outcome.idempotencyReplayed,
                }, {
                    ...metadata,
                    intentKey,
                    retryRequest,
                });
            }
            this.clearPendingMutationIntent(pendingIntentId);
            return { outcome, operation };
        } catch (error) {
            if (error instanceof UnknownMutationOutcomeError) {
                this.promotePendingMutationIntent(pendingIntentId, error);
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
            }
            throw error;
        }
    }

    async fetchConfigurationWorkspace() {
        this.setWorkspaceStatus('config-status', 'loading', 'Loading configuration capabilities…');
        const results = await Promise.allSettled([
            this.fetchStartupProfiles(), this.fetchConfigLoadouts(),
            this.fetchAccelerationProviders(), this.fetchMcpDiscovery(),
        ]);
        const failures = results.filter((result) => result.status === 'rejected');
        this.updateStorageAuthority();
        this.setWorkspaceStatus(
            'config-status', failures.length ? 'degraded' : 'ready',
            failures.length ? `${failures.length} configuration domains unavailable; available panels remain usable.` : 'Configuration domains loaded.',
        );
    }

    startupProfileRequest(profile) {
        const body = {
            description: profile.description ?? null,
            trigger: profile.trigger || 'on_instance_ready',
            target: profile.target || {},
            session: profile.session,
            credential_refs: profile.credential_refs || [],
            readiness_probes: profile.readiness_probes || [],
            observation: profile.observation || {},
            control: profile.control || {},
            restart: profile.restart || {},
        };
        if (!body.session?.command) throw new Error('session.command is required');
        return body;
    }

    async fetchStartupProfiles() {
        const data = await this.managementJson('/api/v2/startup-profiles/');
        this.startupProfiles.clear();
        for (const profile of (data.startup_profiles || [])) this.startupProfiles.set(profile.id, profile);
        const list = document.getElementById('startup-profile-list');
        list.replaceChildren();
        if (!this.startupProfiles.size) list.textContent = 'No startup profiles.';
        for (const profile of this.startupProfiles.values()) {
            const button = document.createElement('button');
            button.type = 'button';
            const bindings = Number(profile.active_instance_bindings || 0);
            button.textContent = `${profile.id} · ${profile.status?.state || 'unknown'} · ${profile.trigger} · ${bindings} active binding${bindings === 1 ? '' : 's'}`;
            button.addEventListener('click', () => this.selectStartupProfile(profile.id));
            list.append(button);
        }
        const selector = document.getElementById('instance-startup-profile');
        if (selector) {
            const selected = selector.value;
            selector.replaceChildren();
            const none = document.createElement('option'); none.value = ''; none.textContent = 'None'; selector.append(none);
            for (const profile of this.startupProfiles.values()) {
                const option = document.createElement('option'); option.value = profile.id;
                option.textContent = `${profile.id} · ${profile.status?.state || 'unknown'}`;
                selector.append(option);
            }
            if ([...selector.options].some((option) => option.value === selected)) selector.value = selected;
        }
    }

    selectStartupProfile(id) {
        const profile = this.startupProfiles.get(id);
        if (!profile) return;
        const activeBindings = Number(profile.active_instance_bindings || 0);
        const activeExecution = ['launching', 'running'].includes(profile.status?.state);
        this.selectedStartupProfileId = id;
        document.getElementById('startup-profile-document').value = JSON.stringify({ id, ...this.startupProfileRequest(profile) }, null, 2);
        document.getElementById('startup-profile-delete').disabled = activeBindings > 0 || activeExecution;
        this.reviewedStartupProfile = null;
        document.getElementById('startup-profile-apply').disabled = true;
        document.getElementById('startup-profile-preview').textContent = JSON.stringify({
            selected: id,
            active_instance_bindings: activeBindings,
            active_execution: activeExecution,
            safe_delete: activeBindings === 0 && !activeExecution,
            instruction: 'Review changes before applying.',
        }, null, 2);
    }

    newStartupProfile() {
        this.selectedStartupProfileId = null;
        this.reviewedStartupProfile = null;
        document.getElementById('startup-profile-delete').disabled = true;
        document.getElementById('startup-profile-apply').disabled = true;
        document.getElementById('startup-profile-document').value = JSON.stringify({
            id: '', description: '', trigger: 'on_instance_ready', target: {},
            session: { command: '', workdir: '/workspace', backend: 'tmux', class: 'managed', cols: 120, rows: 40 },
            credential_refs: [], readiness_probes: [],
            observation: { transcript_enabled: true, retention_class: 'standard', redaction_profile: 'default' },
            control: { default_role: 'observer', controller_allowed: false },
            restart: { mode: 'never', max_attempts: 0 },
        }, null, 2);
        document.getElementById('startup-profile-preview').textContent = 'New profile draft; review before applying.';
    }

    reviewStartupProfile() {
        try {
            const documentValue = JSON.parse(document.getElementById('startup-profile-document').value);
            const id = String(documentValue.id || '').trim();
            if (!id && !this.selectedStartupProfileId) throw new Error('id is required for a new profile');
            const body = this.startupProfileRequest(documentValue);
            if (!this.selectedStartupProfileId) body.id = id;
            this.reviewedStartupProfile = {
                id: this.selectedStartupProfileId || id,
                body,
                update: Boolean(this.selectedStartupProfileId),
                intentKey: ApiClient.newIntentId(),
            };
            document.getElementById('startup-profile-preview').textContent = JSON.stringify({
                id: this.reviewedStartupProfile.id,
                body: this.reviewedStartupProfile.body,
                update: this.reviewedStartupProfile.update,
            }, null, 2);
            document.getElementById('startup-profile-apply').disabled = false;
        } catch (error) {
            this.reviewedStartupProfile = null;
            document.getElementById('startup-profile-apply').disabled = true;
            this.showToast(`Profile review failed: ${error.message}`, 'error');
        }
    }

    async applyStartupProfile() {
        const reviewed = this.reviewedStartupProfile;
        if (!reviewed) return;
        const path = reviewed.update ? `/api/v2/startup-profiles/${encodeURIComponent(reviewed.id)}` : '/api/v2/startup-profiles/';
        document.getElementById('startup-profile-apply').disabled = true;
        await this.trackedMutation(path, {
            method: reviewed.update ? 'PUT' : 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(reviewed.body),
            expectJson: true,
        }, {
            target: reviewed.id,
            kind: reviewed.update ? 'config.startup.update' : 'config.startup.create',
            intentKey: reviewed.intentKey,
        });
        this.reviewedStartupProfile = null;
        await this.fetchStartupProfiles();
        this.selectStartupProfile(reviewed.id);
        this.showToast(`Startup profile ${reviewed.id} saved`, 'success');
    }

    async deleteStartupProfile() {
        const id = this.selectedStartupProfileId;
        const profile = this.startupProfiles.get(id);
        if (!id || !profile) return;
        if (['launching', 'running'].includes(profile.status?.state)) {
            this.showToast('A launching or running profile cannot be deleted.', 'error');
            return;
        }
        if (!confirm(`Delete startup profile ${id}? The server will refuse active references.`)) return;
        await this.trackedMutation(
            `/api/v2/startup-profiles/${encodeURIComponent(id)}`,
            { method: 'DELETE' },
            { target: id, kind: 'config.startup.delete' },
        );
        this.newStartupProfile();
        await this.fetchStartupProfiles();
    }

    async fetchConfigLoadouts() {
        const data = await this.managementJson('/api/v2/admin/loadouts');
        this.configLoadouts = data.items || [];
        const list = document.getElementById('config-loadout-list');
        list.replaceChildren();
        if (!this.configLoadouts.length) list.textContent = 'No loadouts available.';
        for (const loadout of this.configLoadouts) {
            const button = document.createElement('button');
            button.type = 'button';
            button.textContent = `${loadout.name} · ${loadout.runtime || 'runtime unspecified'}`;
            button.addEventListener('click', () => {
                document.getElementById('config-loadout-detail').textContent = JSON.stringify({
                    description: loadout.description, runtime: loadout.runtime,
                    runtime_options: loadout.runtime_options, compatibility: loadout.compatibility || [],
                }, null, 2);
            });
            list.append(button);
        }
    }

    reviewConfigLoadout() {
        const name = document.getElementById('config-loadout-name').value.trim();
        const manifest = document.getElementById('config-loadout-manifest').value;
        if (!/^[a-z0-9][a-z0-9_-]{0,79}$/.test(name) || !manifest.trim()) {
            this.showToast('Loadout name and YAML manifest are required.', 'error'); return;
        }
        this.reviewedConfigLoadout = { name, manifest, intentKey: ApiClient.newIntentId() };
        document.getElementById('config-loadout-preview').textContent = JSON.stringify({ name, manifest_bytes: new TextEncoder().encode(manifest).length, effect: 'create catalog entry' }, null, 2);
        document.getElementById('config-loadout-apply').disabled = false;
    }

    async applyConfigLoadout() {
        if (!this.reviewedConfigLoadout) return;
        const reviewed = this.reviewedConfigLoadout;
        document.getElementById('config-loadout-apply').disabled = true;
        await this.trackedMutation('/api/v2/admin/loadouts', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: reviewed.name, manifest: reviewed.manifest }),
            expectJson: true,
        }, {
            target: reviewed.name,
            kind: 'config.loadout.create',
            intentKey: reviewed.intentKey,
        });
        this.reviewedConfigLoadout = null;
        await this.fetchConfigLoadouts();
    }

    normalizedStoragePath() {
        const path = document.getElementById('storage-path').value.replace(/^\/+|\/+$/g, '');
        const segments = path.split('/').filter(Boolean);
        if (!segments.length || segments.some((segment) => segment === '.' || segment === '..')) throw new Error('A bounded path without . or .. segments is required');
        return segments;
    }

    storageUrl() {
        const scope = document.getElementById('storage-scope').value;
        return `/api/v2/admin/storage/${encodeURIComponent(scope)}/${this.normalizedStoragePath().map(encodeURIComponent).join('/')}`;
    }

    updateStorageAuthority() {
        const readOnly = document.getElementById('storage-scope').value === 'global';
        document.getElementById('storage-authority').textContent = readOnly ? 'Authority: read-only shared storage.' : 'Authority: scoped read/write storage; mutations require review or confirmation.';
        document.getElementById('storage-review-write').disabled = readOnly;
        document.getElementById('storage-delete').disabled = readOnly;
        document.getElementById('storage-apply-write').disabled = readOnly || !this.reviewedStorageWrite;
        this.renderStorageBreadcrumbs();
    }

    renderStorageBreadcrumbs() {
        const holder = document.getElementById('storage-breadcrumbs');
        holder.replaceChildren();
        let segments;
        try { segments = this.normalizedStoragePath(); } catch (_) { return; }
        segments.forEach((segment, index) => {
            const button = document.createElement('button');
            button.type = 'button'; button.className = 'btn'; button.textContent = segment;
            button.addEventListener('click', () => {
                document.getElementById('storage-path').value = segments.slice(0, index + 1).join('/');
                this.readStoragePath();
            });
            holder.append(button);
        });
    }

    async readStoragePath() {
        try {
            const data = await this.managementJson(this.storageUrl());
            this.storageObjectExists = data.kind !== 'directory';
            this.reviewedStorageWrite = null;
            document.getElementById('storage-apply-write').disabled = true;
            const list = document.getElementById('storage-list'); list.replaceChildren();
            if (data.kind === 'directory') {
                for (const item of (data.items || [])) {
                    const button = document.createElement('button');
                    button.type = 'button'; button.textContent = `${item.kind === 'directory' ? 'dir' : 'object'} · ${item.name}${item.size_bytes == null ? '' : ` · ${item.size_bytes} B`}`;
                    button.addEventListener('click', () => {
                        const base = document.getElementById('storage-path').value.replace(/\/+$/, '');
                        document.getElementById('storage-path').value = `${base}/${item.name}`;
                        this.readStoragePath();
                    });
                    list.append(button);
                }
                if (data.truncated) list.append(`Showing the first ${data.limit} entries.`);
                document.getElementById('storage-content').value = '';
            } else {
                list.textContent = `${data.media_type} · ${data.size_bytes} B · sha256 ${data.sha256}`;
                document.getElementById('storage-content').value = data.content ?? '[binary content is not editable in this panel]';
            }
            this.renderStorageBreadcrumbs();
        } catch (error) {
            this.storageObjectExists = false;
            this.showToast(`Storage read failed: ${error.message}`, 'error');
        }
    }

    reviewStorageWrite() {
        try {
            const url = this.storageUrl();
            const content = document.getElementById('storage-content').value;
            const size = new TextEncoder().encode(content).length;
            if (size > 1024 * 1024) throw new Error('content exceeds the 1 MiB management limit');
            this.reviewedStorageWrite = {
                url,
                body: { media_type: 'text/plain; charset=utf-8', content },
                replaces_existing: this.storageObjectExists,
                size_bytes: size,
                intentKey: ApiClient.newIntentId(),
            };
            document.getElementById('storage-preview').textContent = JSON.stringify({ path: url, size_bytes: size, replaces_existing: this.storageObjectExists }, null, 2);
            document.getElementById('storage-apply-write').disabled = false;
        } catch (error) { this.showToast(`Storage review failed: ${error.message}`, 'error'); }
    }

    async applyStorageWrite() {
        const reviewed = this.reviewedStorageWrite;
        if (!reviewed) return;
        if (reviewed.replaces_existing && !confirm('Replace the existing storage object with the reviewed content?')) return;
        document.getElementById('storage-apply-write').disabled = true;
        await this.trackedMutation(reviewed.url, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(reviewed.body),
            expectJson: true,
        }, {
            target: reviewed.url,
            kind: 'config.storage.write',
            intentKey: reviewed.intentKey,
        });
        this.reviewedStorageWrite = null;
        await this.readStoragePath();
    }

    async deleteStorageObject() {
        let url;
        try { url = this.storageUrl(); } catch (error) { this.showToast(error.message, 'error'); return; }
        if (!this.storageObjectExists) { this.showToast('Browse and select an object before deleting.', 'error'); return; }
        if (!confirm(`Delete ${url}? This action cannot be undone.`)) return;
        await this.trackedMutation(url, { method: 'DELETE' }, {
            target: url,
            kind: 'config.storage.delete',
        });
        this.storageObjectExists = false;
        document.getElementById('storage-content').value = '';
    }

    async fetchAccelerationProviders() {
        const data = await this.managementJson('/api/v2/admin/runtime/providers');
        this.accelerationProviders.clear();
        for (const provider of (data.providers || [])) this.accelerationProviders.set(provider.id, provider);
        const select = document.getElementById('acceleration-provider'); select.replaceChildren();
        for (const provider of this.accelerationProviders.values()) {
            if (!provider.available) continue;
            const option = document.createElement('option'); option.value = provider.id; option.textContent = provider.id; select.append(option);
        }
        this.renderAccelerationActions();
    }

    accelerationActions(provider) {
        const caps = new Set(provider?.capabilities || []);
        const actions = [];
        if (caps.has('instance.checkpoint') || caps.has('instance.snapshot')) actions.push(['capture', provider.id === 'libvirt' ? 'Capture checkpoint' : 'Capture snapshot']);
        if (caps.has('instance.restore')) actions.push(['restore', 'Restore']);
        if (caps.has('instance.fork')) actions.push(['fork', 'Fork']);
        if (caps.has('warm_pool.manage')) actions.push(['warm-init', 'Initialize warm pool'], ['warm-handoff', 'Handoff warm slot']);
        return actions;
    }

    renderAccelerationActions() {
        const provider = this.accelerationProviders.get(document.getElementById('acceleration-provider').value);
        const holder = document.getElementById('acceleration-capabilities'); holder.replaceChildren();
        for (const candidate of this.accelerationProviders.values()) {
            if (candidate.available) continue;
            const tag = document.createElement('span');
            tag.textContent = `${candidate.id} unavailable · ${candidate.unavailable_reason || candidate.unavailable_code || 'provider readiness was not established'}`;
            holder.append(tag);
        }
        for (const cap of (provider?.capabilities || [])) { const tag = document.createElement('span'); tag.textContent = cap; holder.append(tag); }
        for (const reason of (provider?.constraints || [])) { const tag = document.createElement('span'); tag.textContent = typeof reason === 'string' ? reason : JSON.stringify(reason); holder.append(tag); }
        const select = document.getElementById('acceleration-action'); select.replaceChildren();
        const actions = this.accelerationActions(provider);
        for (const [value, label] of actions) { const option = document.createElement('option'); option.value = value; option.textContent = label; select.append(option); }
        this.reviewedAcceleration = null;
        document.getElementById('acceleration-review').disabled = actions.length === 0;
        document.getElementById('acceleration-apply').disabled = true;
    }

    reviewAcceleration() {
        const provider = document.getElementById('acceleration-provider').value;
        const action = document.getElementById('acceleration-action').value;
        const asset = document.getElementById('acceleration-asset').value.trim();
        const target = document.getElementById('acceleration-target').value.trim();
        const capacity = Number(document.getElementById('acceleration-capacity').value);
        if (!this.accelerationActions(this.accelerationProviders.get(provider)).some(([name]) => name === action)) return;
        if (!asset || !target || !Number.isInteger(capacity) || capacity < 1 || capacity > 64) { this.showToast('Identifier, target, and bounded capacity are required.', 'error'); return; }
        let path; let body;
        if (provider === 'cloud-hypervisor') {
            if (action === 'capture') [path, body] = ['/api/v2/admin/cloud-hypervisor/snapshots', { vm: target, snapshot_id: asset, pre_enrollment: true, secret_bearing: false }];
            if (action === 'restore') [path, body] = [`/api/v2/admin/cloud-hypervisor/snapshots/${encodeURIComponent(asset)}/restore`, { name: target }];
            if (action === 'fork') [path, body] = [`/api/v2/admin/cloud-hypervisor/snapshots/${encodeURIComponent(asset)}/fork`, { prefix: target, count: capacity }];
            if (action === 'warm-init') [path, body] = ['/api/v2/admin/cloud-hypervisor/warm-pools', { snapshot_id: asset, size: capacity, prefix: target }];
            if (action === 'warm-handoff') [path, body] = [`/api/v2/admin/cloud-hypervisor/warm-pools/${encodeURIComponent(asset)}/handoff`, { name: target }];
        } else if (provider === 'libvirt') {
            if (action === 'capture') [path, body] = ['/api/v2/admin/libvirt/checkpoints', { vm: target, checkpoint_id: asset, pre_enrollment: true }];
            if (action === 'restore') [path, body] = [`/api/v2/admin/libvirt/checkpoints/${encodeURIComponent(asset)}/restore`, { name: target }];
            if (action === 'warm-init') [path, body] = ['/api/v2/admin/libvirt/warm-pools', { checkpoint_ids: asset.split(',').map((id) => id.trim()).filter(Boolean), pool: target }];
            if (action === 'warm-handoff') [path, body] = [`/api/v2/admin/libvirt/warm-pools/${encodeURIComponent(asset)}/handoff`, {}];
        }
        if (!path) return;
        const descriptor = this.accelerationProviders.get(provider);
        const destructiveImplications = {
            capture: 'Quiesces the selected source while creating a reusable state artifact.',
            restore: 'Creates or replaces runtime state from the selected artifact; review the target identity.',
            fork: 'Creates multiple runtime allocations and consumes the reviewed capacity.',
            'warm-init': 'Allocates the reviewed warm-pool capacity and underlying runtime resources.',
            'warm-handoff': 'Consumes a prepared slot and transfers it to the reviewed target.',
        };
        this.reviewedAcceleration = {
            provider, action, path, body, capacity, intentKey: ApiClient.newIntentId(),
            provenance: 'runtime provider capability discovery',
            compatibility: {
                available: descriptor?.available === true,
                capabilities: descriptor?.capabilities || [],
                constraints: descriptor?.constraints || [],
            },
            destructive_implications: destructiveImplications[action],
        };
        const { intentKey: _intentKey, ...reviewedForDisplay } = this.reviewedAcceleration;
        document.getElementById('acceleration-preview').textContent = JSON.stringify(reviewedForDisplay, null, 2);
        document.getElementById('acceleration-apply').disabled = false;
    }

    async applyAcceleration() {
        const reviewed = this.reviewedAcceleration;
        if (!reviewed || !confirm(`Apply reviewed ${reviewed.action} operation on ${reviewed.provider}?`)) return;
        document.getElementById('acceleration-apply').disabled = true;
        await this.trackedMutation(reviewed.path, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(reviewed.body),
            expectJson: true,
        }, {
            kind: `acceleration.${reviewed.action}`,
            target: reviewed.body.name || reviewed.body.vm || reviewed.body.pool || reviewed.body.prefix,
            intentKey: reviewed.intentKey,
        });
        this.reviewedAcceleration = null;
    }

    async fetchMcpDiscovery() {
        const holder = document.getElementById('mcp-discovery');
        try {
            const data = await this.managementJson('/api/v2/admin/mcp/discovery');
            holder.textContent = JSON.stringify({
                availability: { enabled: data.enabled, status: data.status, reason_code: data.reason_code },
                endpoint: data.endpoint, protocol: data.protocol, auth_posture: data.auth,
                capabilities: data.capabilities, tools: data.tools || [], resources: data.resources || [],
                resource_templates: data.resource_templates || [], errors: data.errors || [], notes: data.notes || [],
            }, null, 2);
        } catch (error) { holder.textContent = `MCP discovery unavailable: ${error.message}`; }
    }

    hasAccessPermission(permission) {
        return Boolean(this.accessAuthority?.permissions?.includes(permission));
    }

    accessErrorMessage(error) {
        if (error.status === 401) return 'Authentication is required.';
        if (error.status === 403) return 'The authenticated operator lacks authority.';
        if (error.status === 409) return 'State changed; refresh before trying again.';
        if (error.status === 429) return `Rate limited${error.retryAfter ? `; retry after ${error.retryAfter} seconds` : ''}.`;
        if (!error.status) return 'Outcome unknown after a transport failure; inventory will be reconciled without replaying the mutation.';
        return `Access request failed (HTTP ${error.status}${error.code ? `, ${error.code}` : ''}).`;
    }

    async fetchAccessAuthority() {
        await window.ManagementUIReady;
        const data = await this.managementJson('/api/v2/credentials/authority');
        if (data.schema_version !== 'management.access-authority/v1' || !Array.isArray(data.permissions)) {
            throw new Error('unsupported access authority contract');
        }
        this.accessAuthority = {
            schema_version: data.schema_version,
            mode: String(data.mode || 'unresolved'),
            actor: data.actor == null ? null : String(data.actor),
            role: data.role == null ? null : String(data.role),
            permissions: data.permissions.map(String),
        };
        this.setWorkspaceStatus(
            'access-authority',
            this.accessAuthority.role ? 'ready' : 'degraded',
            `Authority: ${this.accessAuthority.mode} · actor ${this.accessAuthority.actor || 'unresolved'} · role ${this.accessAuthority.role || 'none'}`,
        );
        this.updateAccessMutationControls();
    }

    updateAccessMutationControls() {
        const credentialReview = document.getElementById('credential-lease-review');
        const sshReview = document.getElementById('ssh-lease-review');
        if (credentialReview) credentialReview.disabled = !this.hasAccessPermission('credential_lease.issue') || !this.accessCredentials.size;
        if (sshReview) sshReview.disabled = !this.hasAccessPermission('ssh_lease.issue');
        const credentialApply = document.getElementById('credential-lease-apply');
        const sshApply = document.getElementById('ssh-lease-apply');
        if (credentialApply) credentialApply.disabled = !this.reviewedCredentialLease || !this.hasAccessPermission('credential_lease.issue');
        if (sshApply) sshApply.disabled = !this.reviewedSshLease || !this.hasAccessPermission('ssh_lease.issue');
    }

    discardCredentialLeaseReview() {
        this.reviewedCredentialLease = null;
        const preview = document.getElementById('credential-lease-preview');
        if (preview) preview.textContent = 'No lease issuance reviewed.';
        this.updateAccessMutationControls();
    }

    discardSshLeaseReview(clearInput = false) {
        if (this.reviewedSshLease?.body) this.reviewedSshLease.body.public_key = '';
        this.reviewedSshLease = null;
        const input = document.getElementById('ssh-lease-public-key');
        if (clearInput && input) input.value = '';
        const preview = document.getElementById('ssh-lease-preview');
        if (preview) preview.textContent = 'No SSH lease issuance reviewed.';
        this.updateAccessMutationControls();
    }

    async fetchAccessWorkspace() {
        this.discardSshLeaseReview(true);
        this.discardCredentialLeaseReview();
        this.accessAuthority = null;
        this.accessCredentials.clear();
        this.accessCredentialLeases.clear();
        this.accessSshLeases.clear();
        this.accessAuditEvents.clear();
        document.getElementById('credential-list').textContent = 'Credential inventory unavailable until authority resolves.';
        document.getElementById('credential-lease-list').textContent = 'Credential lease inventory unavailable until authority resolves.';
        document.getElementById('ssh-lease-list').textContent = 'SSH lease inventory unavailable until authority resolves.';
        document.getElementById('access-audit-list').textContent = 'Audit evidence unavailable until authority resolves.';
        document.getElementById('credential-detail').textContent = 'Select a credential definition.';
        document.getElementById('credential-lease-detail').textContent = 'Select a credential lease for scope and lifecycle metadata.';
        document.getElementById('ssh-lease-detail').textContent = 'Select an SSH lease for actor, fingerprint, and lifecycle metadata.';
        document.getElementById('access-audit-detail').textContent = 'Select an audit event.';
        this.updateAccessMutationControls();
        this.setWorkspaceStatus('access-authority', 'loading', 'Loading server authority evidence…');
        try {
            await this.fetchAccessAuthority();
        } catch (error) {
            this.setWorkspaceStatus('access-authority', 'degraded', `Access workspace unavailable: ${this.accessErrorMessage(error)}`);
            return;
        }
        const tasks = [this.fetchAccessCredentials(), this.fetchCredentialLeases(), this.fetchAccessAudit()];
        if (this.hasAccessPermission('ssh_lease.read')) tasks.push(this.fetchSshLeases());
        else {
            this.accessSshLeases.clear();
            document.getElementById('ssh-lease-list').textContent = 'SSH lease inventory requires authenticated operator identity.';
        }
        const results = await Promise.allSettled(tasks);
        const failures = results.filter((result) => result.status === 'rejected');
        if (failures.length) {
            this.setWorkspaceStatus('access-authority', 'degraded', `${this.accessAuthority.role} authority resolved; ${failures.length} metadata sources are unavailable.`);
        }
        this.updateAccessMutationControls();
    }

    credentialMetadata(value) {
        return window.ManagementUI.projectCredentialMetadata(value);
    }

    workspaceListFocusKey(list) {
        return list?.contains(document.activeElement) ? document.activeElement.dataset.focusKey || null : null;
    }

    restoreWorkspaceListFocus(list, key, fallbackId = 'access-refresh') {
        if (!key) return;
        const target = [...list.querySelectorAll('[data-focus-key]')]
            .find((element) => element.dataset.focusKey === key);
        (target || document.getElementById(fallbackId))?.focus();
    }

    credentialLeaseMetadata(value) {
        return window.ManagementUI.projectCredentialLeaseMetadata(value);
    }

    sshLeaseMetadata(value) {
        return window.ManagementUI.projectSshLeaseMetadata(value);
    }

    async fetchAccessCredentials() {
        const data = await this.managementJson('/api/v2/credentials/');
        this.accessCredentials.clear();
        for (const raw of (data.credentials || [])) {
            const credential = this.credentialMetadata(raw);
            this.accessCredentials.set(credential.id, credential);
        }
        const list = document.getElementById('credential-list');
        const focusKey = this.workspaceListFocusKey(list);
        list.replaceChildren();
        if (!this.accessCredentials.size) list.textContent = 'No credential definitions.';
        for (const credential of this.accessCredentials.values()) {
            const button = document.createElement('button'); button.type = 'button';
            button.dataset.focusKey = `credential:${credential.id}`;
            button.textContent = `${credential.id} · ${credential.provider}/${credential.type} · ${credential.configured ? 'configured' : 'not configured'}`;
            button.addEventListener('click', () => {
                document.getElementById('credential-detail').textContent = JSON.stringify(credential, null, 2);
            });
            list.append(button);
        }
        this.restoreWorkspaceListFocus(list, focusKey);
        const selector = document.getElementById('credential-lease-credential'); selector.replaceChildren();
        for (const credential of this.accessCredentials.values()) {
            const option = document.createElement('option'); option.value = credential.id;
            option.textContent = `${credential.id} · ${credential.provider}`; selector.append(option);
        }
        this.updateCredentialLeaseUses();
        this.updateAccessMutationControls();
    }

    updateCredentialLeaseUses() {
        const credential = this.accessCredentials.get(document.getElementById('credential-lease-credential').value);
        const input = document.getElementById('credential-lease-use');
        const options = document.getElementById('credential-lease-use-options');
        options.replaceChildren();
        for (const use of (credential?.allowed_uses || [])) {
            const option = document.createElement('option'); option.value = use; options.append(option);
        }
        if (credential?.allowed_uses?.length && !credential.allowed_uses.includes(input.value)) {
            input.value = credential.allowed_uses[0];
        } else if (!credential?.allowed_uses?.length) {
            input.value = '';
        }
    }

    async fetchCredentialLeases() {
        const data = await this.managementJson('/api/v2/credentials/leases');
        this.accessCredentialLeases.clear();
        for (const raw of (data.leases || [])) {
            const lease = this.credentialLeaseMetadata(raw);
            this.accessCredentialLeases.set(lease.id, lease);
        }
        const list = document.getElementById('credential-lease-list');
        const focusKey = this.workspaceListFocusKey(list);
        list.replaceChildren();
        if (!this.accessCredentialLeases.size) list.textContent = 'No credential leases.';
        for (const lease of this.accessCredentialLeases.values()) {
            const row = document.createElement('div'); row.className = 'workspace-list-item';
            const detail = document.createElement('button'); detail.type = 'button';
            detail.dataset.focusKey = `credential-lease:${lease.id}`;
            detail.textContent = `${lease.id} · ${lease.credential_id} · ${lease.state} · expires ${lease.expires_at}`;
            detail.addEventListener('click', () => {
                document.getElementById('credential-lease-detail').textContent = JSON.stringify(lease, null, 2);
                this.showAccessAuditForResource(lease.id);
            });
            const revoke = document.createElement('button'); revoke.type = 'button'; revoke.textContent = 'Revoke';
            revoke.dataset.focusKey = `credential-lease-revoke:${lease.id}`;
            revoke.disabled = lease.state !== 'active' || !this.hasAccessPermission('credential_lease.revoke');
            revoke.addEventListener('click', () => this.revokeCredentialLease(lease));
            row.append(detail, revoke); list.append(row);
        }
        this.restoreWorkspaceListFocus(list, focusKey);
    }

    reviewCredentialLease() {
        const credential = this.accessCredentials.get(document.getElementById('credential-lease-credential').value);
        const ttl = Number(document.getElementById('credential-lease-ttl').value);
        const body = {
            agent_id: document.getElementById('credential-lease-agent').value.trim(),
            instance_id: document.getElementById('credential-lease-instance').value.trim(),
            session_id: document.getElementById('credential-lease-session').value.trim(),
            provider: credential?.provider,
            allowed_use: document.getElementById('credential-lease-use').value,
            ttl_seconds: ttl,
        };
        if (!credential || Object.values(body).some((value) => value == null || value === '')
            || (credential.allowed_uses.length > 0 && !credential.allowed_uses.includes(body.allowed_use))
            || !Number.isInteger(ttl) || ttl < 1 || ttl > 3600) {
            this.showToast('Credential, complete scope, allowed use, and TTL from 1 to 3600 seconds are required.', 'error'); return;
        }
        this.reviewedCredentialLease = {
            credential_id: credential.id,
            target: `${body.instance_id}/${body.session_id}`,
            impact: `grants ${body.allowed_use} access through ${body.provider} for ${ttl} seconds`,
            body,
        };
        document.getElementById('credential-lease-preview').textContent = JSON.stringify(this.reviewedCredentialLease, null, 2);
        this.updateAccessMutationControls();
    }

    async applyCredentialLease() {
        const reviewed = this.reviewedCredentialLease;
        if (!reviewed) return;
        try {
            await this.managementJson(`/api/v2/credentials/${window.ManagementUI.encodedAccessPathSegment(reviewed.credential_id)}/leases`, {
                method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(reviewed.body),
            });
            this.showToast(`Credential lease issued for ${reviewed.body.instance_id}`, 'success');
        } catch (error) {
            this.setWorkspaceStatus('access-authority', 'degraded', this.accessErrorMessage(error));
        } finally {
            this.reviewedCredentialLease = null; this.updateAccessMutationControls();
            await this.fetchCredentialLeases().catch(() => {});
            await this.fetchAccessAudit().catch(() => {});
        }
    }

    async revokeCredentialLease(lease) {
        if (!confirm(`Revoke credential lease ${lease.id} for ${lease.instance_id}/${lease.session_id}?`)) return;
        try {
            await this.managementJson(`/api/v2/credentials/leases/${window.ManagementUI.encodedAccessPathSegment(lease.id)}`, { method: 'DELETE' });
        } catch (error) {
            this.setWorkspaceStatus('access-authority', 'degraded', this.accessErrorMessage(error));
        } finally {
            await this.fetchCredentialLeases().catch(() => {});
            await this.fetchAccessAudit().catch(() => {});
        }
    }

    async fetchSshLeases() {
        const data = await this.managementJson('/api/v2/gateway/ssh/leases');
        this.accessSshLeases.clear();
        for (const raw of (data.leases || [])) {
            const lease = this.sshLeaseMetadata(raw);
            this.accessSshLeases.set(lease.id, lease);
        }
        const list = document.getElementById('ssh-lease-list');
        const focusKey = this.workspaceListFocusKey(list);
        list.replaceChildren();
        if (!this.accessSshLeases.size) list.textContent = 'No gateway SSH leases.';
        for (const lease of this.accessSshLeases.values()) {
            const row = document.createElement('div'); row.className = 'workspace-list-item';
            const detail = document.createElement('button'); detail.type = 'button';
            detail.dataset.focusKey = `ssh-lease:${lease.id}`;
            detail.textContent = `${lease.id} · ${lease.instance_id}/${lease.principal} · ${lease.state} · expires ${lease.expires_at}`;
            detail.addEventListener('click', () => {
                document.getElementById('ssh-lease-detail').textContent = JSON.stringify(lease, null, 2);
                this.showAccessAuditForResource(lease.id);
            });
            const revoke = document.createElement('button'); revoke.type = 'button'; revoke.textContent = 'Revoke';
            revoke.dataset.focusKey = `ssh-lease-revoke:${lease.id}`;
            revoke.disabled = lease.state !== 'active' || !this.hasAccessPermission('ssh_lease.revoke');
            revoke.addEventListener('click', () => this.revokeSshLease(lease));
            row.append(detail, revoke); list.append(row);
        }
        this.restoreWorkspaceListFocus(list, focusKey);
    }

    reviewSshLease() {
        const publicKey = document.getElementById('ssh-lease-public-key').value.trim();
        const ttl = Number(document.getElementById('ssh-lease-ttl').value);
        const body = {
            actor: this.accessAuthority?.actor || '',
            instance_id: document.getElementById('ssh-lease-instance').value.trim(),
            principal: document.getElementById('ssh-lease-principal').value.trim(),
            access_mode: document.getElementById('ssh-lease-mode').value,
            public_key: publicKey,
            ttl_seconds: ttl,
        };
        if (!body.instance_id || !/^[a-zA-Z0-9._-]{1,64}$/.test(body.principal)
            || !publicKey.startsWith('ssh-') || publicKey.length > 16384
            || !Number.isInteger(ttl) || ttl < 1 || ttl > 3600) {
            this.showToast('Instance, valid principal, bounded OpenSSH public key, and TTL from 1 to 3600 seconds are required.', 'error'); return;
        }
        this.reviewedSshLease = { body };
        document.getElementById('ssh-lease-preview').textContent = JSON.stringify({
            actor: body.actor, instance_id: body.instance_id, principal: body.principal,
            access_mode: body.access_mode, ttl_seconds: body.ttl_seconds,
            target: `${body.principal}@${body.instance_id}`,
            impact: `grants gateway SSH access for ${ttl} seconds; revocation cannot invalidate an already issued certificate before expiry`,
            public_key_posture: `write-only request field (${publicKey.length} characters)`,
        }, null, 2);
        this.updateAccessMutationControls();
    }

    async applySshLease() {
        const reviewed = this.reviewedSshLease;
        if (!reviewed) return;
        try {
            const raw = await this.managementJson('/api/v2/gateway/ssh/leases', {
                method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(reviewed.body),
            });
            const safe = this.sshLeaseMetadata(raw);
            this.showToast(`SSH lease ${safe.id} issued; certificate content is intentionally not displayed or retained.`, 'success');
        } catch (error) {
            this.setWorkspaceStatus('access-authority', 'degraded', this.accessErrorMessage(error));
        } finally {
            reviewed.body.public_key = '';
            document.getElementById('ssh-lease-public-key').value = '';
            this.reviewedSshLease = null; this.updateAccessMutationControls();
            await this.fetchSshLeases().catch(() => {});
            await this.fetchAccessAudit().catch(() => {});
        }
    }

    async revokeSshLease(lease) {
        if (!confirm(`Revoke SSH lease ${lease.id} for ${lease.instance_id}/${lease.principal}? Existing certificates remain valid until expiry.`)) return;
        try {
            await this.managementJson(`/api/v2/gateway/ssh/leases/${window.ManagementUI.encodedAccessPathSegment(lease.id)}`, { method: 'DELETE' });
        } catch (error) {
            this.setWorkspaceStatus('access-authority', 'degraded', this.accessErrorMessage(error));
        } finally {
            await this.fetchSshLeases().catch(() => {});
            await this.fetchAccessAudit().catch(() => {});
        }
    }

    async fetchAccessAudit() {
        const dateInput = document.getElementById('access-audit-date');
        if (!dateInput.value) dateInput.value = new Date().toISOString().slice(0, 10);
        const data = await this.managementJson(`/api/v2/credentials/audit?date=${encodeURIComponent(dateInput.value)}`);
        const list = document.getElementById('access-audit-list');
        const focusKey = this.workspaceListFocusKey(list);
        list.replaceChildren();
        this.accessAuditEvents.clear();
        if (!data.available) {
            list.textContent = data.reason || 'Audit evidence unavailable.';
            this.restoreWorkspaceListFocus(list, focusKey, 'access-audit-refresh');
            return;
        }
        for (const raw of (data.events || [])) {
            const event = window.ManagementUI.projectAccessAuditMetadata(raw);
            this.accessAuditEvents.set(event.id, event);
            const button = document.createElement('button'); button.type = 'button';
            button.dataset.focusKey = `audit:${event.id}`;
            button.textContent = `${event.timestamp} · ${event.action} · ${event.outcome} · ${event.resource}`;
            button.addEventListener('click', () => { document.getElementById('access-audit-detail').textContent = JSON.stringify(event, null, 2); });
            list.append(button);
        }
        this.restoreWorkspaceListFocus(list, focusKey, 'access-audit-refresh');
        if (!this.accessAuditEvents.size) list.textContent = 'No access audit evidence for this date.';
    }

    showAccessAuditForResource(resource) {
        const matches = [...this.accessAuditEvents.values()].filter((event) => event.resource === resource || event.correlation_id === resource);
        document.getElementById('access-audit-detail').textContent = matches.length
            ? JSON.stringify(matches, null, 2)
            : `No loaded audit evidence for ${resource}; refresh the current date or inspect another date.`;
    }

    setWorkspaceStatus(id, state, message) {
        const status = document.getElementById(id);
        if (!status) return;
        window.ManagementUI.renderWorkspaceStatus(status, state, message);
    }

    async fetchFleetInventory() {
        this.setWorkspaceStatus('fleet-status', 'loading', 'Loading durable fleet inventory…');
        try {
            const response = (await ApiClient.request('/api/v2/fleet/workloads')).response;
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const inventory = await response.json();
            if (inventory.api_version !== 'agentic-orchestration/v1' || !Array.isArray(inventory.records)) {
                throw new Error('unsupported fleet inventory contract');
            }
            this.fleetWorkloads.clear();
            for (const record of inventory.records) {
                const id = record?.lineage?.child_id;
                if (id) this.fleetWorkloads.set(String(id), record);
            }
            this.fleetInventoryRevision = inventory.inventory_revision;
            this.reviewedFleetReconcile = null;
            const apply = document.getElementById('fleet-apply-reconcile');
            if (apply) apply.disabled = true;
            this.renderFleetInventory();
            this.setWorkspaceStatus(
                'fleet-status',
                'ready',
                `${this.fleetWorkloads.size} durable workloads · revision ${inventory.inventory_revision} · generated ${inventory.generated_at}`,
            );
        } catch (error) {
            this.setWorkspaceStatus('fleet-status', 'degraded', `Fleet inventory unavailable (${error.message}).`);
        }
    }

    renderFleetInventory() {
        const list = document.getElementById('fleet-list');
        if (!list) return;
        list.replaceChildren();
        if (!this.fleetWorkloads.size) {
            const empty = document.createElement('p');
            empty.textContent = 'No durable workloads are present.';
            list.append(empty);
            return;
        }
        for (const [id, record] of this.fleetWorkloads) {
            const row = document.createElement('label');
            row.className = 'workspace-list-item';
            const selected = document.createElement('input');
            selected.type = 'checkbox';
            selected.dataset.fleetChild = id;
            const summary = document.createElement('button');
            summary.type = 'button';
            summary.className = 'activity-link';
            summary.textContent = `${id} · ${record.kind || 'unknown'} · ${record.status?.observed_state || 'unknown'} · r${record.status?.revision ?? '?'}`;
            summary.addEventListener('click', () => this.renderFleetDetail(record));
            row.append(selected, summary);
            list.append(row);
        }
    }

    appendDefinition(list, label, value) {
        const term = document.createElement('dt');
        term.textContent = label;
        const detail = document.createElement('dd');
        detail.textContent = value == null || value === ''
            ? 'not reported'
            : (typeof value === 'string' ? value : JSON.stringify(value));
        list.append(term, detail);
    }

    renderFleetDetail(record) {
        const detail = document.getElementById('fleet-detail');
        if (!detail) return;
        const lineage = record.lineage || {};
        const status = record.status || {};
        const spec = record.spec || {};
        const values = document.createElement('dl');
        for (const [label, value] of [
            ['Child', lineage.child_id], ['Parent', lineage.parent_id], ['Mission', lineage.mission_id],
            ['Dispatch', lineage.dispatch_id], ['Idempotency key', lineage.idempotency_key],
            ['Idempotency outcome', 'durable admission recorded; replay outcome unavailable in inventory'],
            ['Target', lineage.target_id], ['Executor', lineage.executor_id], ['Runtime', lineage.runtime_id],
            ['Session', lineage.session_id], ['Task', lineage.task_id], ['Command', lineage.command_id],
            ['Desired state', spec.desired_state], ['Observed state', status.observed_state],
            ['Attested state', status.attested_state || 'not reported by the workload contract'],
            ['Revision', status.revision], ['Last observation', status.last_seen], ['Health', status.health],
            ['Backpressure', status.backpressure], ['Artifacts', status.artifacts],
            ['Exit classification', status.exit_classification], ['Error code', status.error_code],
        ]) this.appendDefinition(values, label, value);
        const evidence = document.createElement('button');
        evidence.type = 'button';
        evidence.className = 'btn';
        evidence.textContent = 'Open correlated activity evidence';
        evidence.addEventListener('click', () => this.openActivityEvidence({
            instanceId: lineage.target_id,
            agentId: lineage.executor_id,
            sessionId: lineage.session_id,
        }));
        detail.replaceChildren(values, evidence);
    }

    previewFleetReconcile() {
        const childIds = [...document.querySelectorAll('[data-fleet-child]:checked')]
            .map((input) => input.dataset.fleetChild);
        if (!childIds.length || this.fleetInventoryRevision == null) {
            this.showToast('Select at least one workload from a loaded inventory', 'error');
            return;
        }
        this.reviewedFleetReconcile = {
            before_revision: this.fleetInventoryRevision,
            child_ids: childIds,
        };
        document.getElementById('fleet-reconcile-preview').textContent = JSON.stringify(this.reviewedFleetReconcile, null, 2);
        document.getElementById('fleet-apply-reconcile').disabled = false;
    }

    async applyFleetReconcile() {
        const plan = this.reviewedFleetReconcile;
        if (!plan || plan.before_revision !== this.fleetInventoryRevision) {
            this.showToast('The fleet reconciliation plan is missing or stale; preview it again', 'error');
            return;
        }
        const apply = document.getElementById('fleet-apply-reconcile');
        apply.disabled = true;
        const intentKey = ApiClient.newIntentId();
        const retryRequest = {
            path: '/api/v2/fleet/reconcile',
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(plan),
        };
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey,
            target: 'fleet',
            kind: 'fleet.reconcile',
            retryRequest,
        });
        try {
            const outcome = await this.managementRequest('/api/v2/fleet/reconcile', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: retryRequest.body,
                idempotencyKey: intentKey,
                owner: 'fleet-reconcile',
            });
            if (outcome.kind === 'accepted') {
                const operation = this.trackCanonicalOperation({
                    ...(outcome.body || {}),
                    operation_id: outcome.operationId,
                    trace_id: outcome.traceId,
                    request_id: outcome.requestId,
                    idempotency_replayed: outcome.idempotencyReplayed,
                }, {
                    target: 'fleet', kind: 'fleet.reconcile', intentKey,
                    retryRequest,
                });
                this.clearPendingMutationIntent(pendingIntentId);
                this.renderJsonResult('fleet-reconcile-result', {
                    evidence_completeness: 'bounded to durable fleet inventory and operation result',
                    operation,
                });
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
                this.renderJsonResult('fleet-reconcile-result', {
                    evidence_completeness: 'bounded to durable fleet inventory',
                    operation_model: 'synchronous compatibility response',
                    ...(outcome.body || {}),
                });
            }
            await this.fetchFleetInventory();
        } catch (error) {
            const unknown = error instanceof UnknownMutationOutcomeError;
            if (unknown) {
                this.promotePendingMutationIntent(pendingIntentId, error);
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
            }
            const currentRevision = error.outcome?.problem?.current_revision;
            this.setWorkspaceStatus('fleet-status', 'degraded', unknown
                ? 'Reconciliation response was lost. Outcome is unknown; refreshing inventory for scoped reconciliation.'
                : error.status === 409
                    ? `Reconciliation revision conflict; current revision is ${currentRevision ?? 'unknown'}. Refresh and review again.`
                    : `Fleet reconciliation failed (${error.message}).`);
            await this.fetchFleetInventory();
        } finally {
            this.reviewedFleetReconcile = null;
        }
    }

    renderJsonResult(id, value) {
        const target = document.getElementById(id);
        if (target) target.textContent = JSON.stringify(value, null, 2);
    }

    async fetchCelldStatus() {
        try {
            const response = (await ApiClient.request('/api/v2/celld/status')).response;
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const body = await response.json();
            const status = body.status || {};
            if (body.schema_version !== 'management.celld-capabilities/v1' || !Array.isArray(body.capabilities)) {
                throw new Error('unsupported Celld capability discovery contract');
            }
            this.celldCapabilities = new Set(body.capabilities.map(String));
            this.reviewedCelldCommand = null;
            this.reviewedCelldReconcile = null;
            document.getElementById('celld-apply-command').disabled = true;
            document.getElementById('celld-apply-reconcile').disabled = true;
            this.renderCelldCapabilities(status);
            this.setWorkspaceStatus(
                'celld-status',
                status.enabled && status.configured ? 'ready' : 'degraded',
                `${status.enabled ? 'enabled' : 'disabled'} · ${status.configured ? 'configured' : 'not configured'} · protocol ${status.protocol_version || 'unknown'} · ${status.security_posture || 'security posture unknown'}${body.configuration_error ? ` · ${body.configuration_error}` : ''}`,
            );
        } catch (error) {
            this.celldCapabilities.clear();
            this.renderCelldCapabilities({});
            this.setWorkspaceStatus('celld-status', 'degraded', `Celld capability discovery unavailable (${error.message}).`);
        }
    }

    renderCelldCapabilities(status) {
        const list = document.getElementById('celld-capabilities');
        if (!list) return;
        list.replaceChildren();
        for (const capability of this.celldCapabilities) {
            const chip = document.createElement('span');
            chip.textContent = capability;
            list.append(chip);
        }
        if (!this.celldCapabilities.size) {
            const chip = document.createElement('span');
            chip.textContent = 'No capabilities discovered';
            list.append(chip);
        }
        for (const [id, capability] of [
            ['celld-get-cell', 'cell.read'],
            ['celld-preview-command', 'cell.command'],
            ['celld-preview-reconcile', 'cell.reconcile'],
        ]) {
            const button = document.getElementById(id);
            if (button) {
                button.disabled = !this.celldCapabilities.has(capability);
                button.title = this.celldCapabilities.has(capability)
                    ? ''
                    : `Server did not advertise ${capability}`;
            }
        }
    }

    celldIdentity() {
        return {
            instance: document.getElementById('celld-instance')?.value.trim() || '',
            generation: Number(document.getElementById('celld-generation')?.value),
        };
    }

    async fetchCelldCell(identity = this.celldIdentity()) {
        if (!identity.instance || !Number.isInteger(identity.generation) || identity.generation < 1) {
            this.showToast('A valid instance and generation are required', 'error');
            return;
        }
        try {
            const response = (await ApiClient.request(`/api/v2/celld/cells/${encodeURIComponent(identity.instance)}?generation=${identity.generation}`)).response;
            const body = await response.json().catch(() => ({}));
            if (!response.ok) throw new Error(body.error?.detail || `HTTP ${response.status}`);
            this.reconcileCelldCommandOperations(body);
            this.recordCelldResult('cell detail', {
                evidence_completeness: body.effects || body.history ? 'provider-reported effect/history evidence' : 'effect and recovery history not reported',
                ...body,
            }, 'celld-cell-result');
        } catch (error) {
            this.recordCelldResult('cell detail failed', { error: error.message }, 'celld-cell-result');
        }
    }

    celldEffectOperationState(status) {
        if (status === 'succeeded') return 'succeeded';
        if (['failed', 'rejected'].includes(status)) return 'failed';
        if (['pending', 'dispatched'].includes(status)) return 'running';
        return 'unknown';
    }

    reconcileCelldCommandOperations(cell) {
        if (!Array.isArray(cell?.effects)) return;
        let changed = false;
        for (const [id, operation] of this.operations) {
            if (operation.kind !== 'celld.command' || operation.target !== cell.instance_id) continue;
            const effect = cell.effects.find((candidate) =>
                candidate?.operation_id === operation.intent_key
                || candidate?.operation_id === String(id).replace(/^(?:unknown:|celld-command:)/, ''));
            if (!effect) continue;
            const state = this.celldEffectOperationState(effect.status);
            const terminal = ['succeeded', 'failed'].includes(state);
            this.operations.set(id, {
                ...operation,
                state,
                completed_at: terminal ? (effect.completed_at || cell.updated_at || new Date().toISOString()) : null,
                progress: terminal
                    ? { percent: 100, phase: `effect ledger ${effect.status}` }
                    : { percent: effect.status === 'dispatched' ? 50 : 10, phase: `effect ledger ${effect.status}` },
                result: terminal ? { effect, cell_updated_at: cell.updated_at } : operation.result,
                reconciliation_evidence: {
                    checked_at: new Date().toISOString(),
                    source: 'scoped Celld cell detail',
                    effect_status: effect.status,
                    generation: cell.generation,
                },
            });
            changed = true;
        }
        if (changed) {
            this.persistTrackedOperations();
            this.renderOperations();
        }
    }

    async reconcileCelldOperation(operation) {
        const instanceId = operation?.target;
        let generation = Number(operation?.reconciliation_before?.generation);
        if (!Number.isInteger(generation) && operation?.retry_request?.body) {
            try { generation = Number(JSON.parse(operation.retry_request.body).generation); } catch (_) {}
        }
        if (!instanceId || !Number.isInteger(generation) || generation < 1) {
            this.showToast('This Celld operation lacks a scoped generation for reconciliation.', 'error');
            return;
        }
        try {
            const outcome = await this.managementRequest(
                `/api/v2/celld/cells/${encodeURIComponent(instanceId)}?generation=${generation}`,
                { owner: `celld-operation:${operation.id}`, expectJson: true },
            );
            this.reconcileCelldCommandOperations(outcome.body);
            this.recordCelldResult('operation reconciliation', {
                operation_id: operation.id,
                evidence_completeness: outcome.body?.effects || outcome.body?.history
                    ? 'provider-reported effect/history evidence'
                    : 'effect and recovery history not reported',
                cell: outcome.body,
            }, 'celld-cell-result');
        } catch (error) {
            this.showToast(`Celld operation reconciliation failed: ${error.message}`, 'error');
        }
    }

    previewCelldReconcile() {
        const identity = this.celldIdentity();
        if (!identity.instance || !Number.isInteger(identity.generation) || identity.generation < 1) return;
        this.reviewedCelldReconcile = {
            management_generation: identity.generation,
            instance_id: identity.instance,
            intentKey: ApiClient.newIntentId(),
        };
        this.renderJsonResult('celld-cell-result', {
            review_required: true,
            action: 'reconcile',
            management_generation: identity.generation,
            instance_id: identity.instance,
        });
        document.getElementById('celld-apply-reconcile').disabled = false;
    }

    async applyCelldReconcile() {
        const plan = this.reviewedCelldReconcile;
        if (!plan || !this.celldCapabilities.has('cell.reconcile')) return;
        document.getElementById('celld-apply-reconcile').disabled = true;
        const path = `/api/v2/celld/cells/${encodeURIComponent(plan.instance_id)}/reconcile`;
        const body = JSON.stringify({ management_generation: plan.management_generation });
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey: plan.intentKey,
            target: plan.instance_id,
            kind: 'celld.reconcile',
            retryRequest: null,
            pollable: false,
            reconciliationBefore: { generation: plan.management_generation },
        });
        try {
            const outcome = await this.managementRequest(path, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body,
                expectJson: true,
                idempotencyKey: plan.intentKey,
                owner: `celld-reconcile:${plan.instance_id}`,
            });
            this.clearPendingMutationIntent(pendingIntentId);
            this.trackCanonicalOperation({
                id: `celld-reconcile:${plan.intentKey}`,
                kind: 'celld.reconcile',
                state: 'succeeded',
                target: plan.instance_id,
                created_at: new Date().toISOString(),
                completed_at: new Date().toISOString(),
                progress: { percent: 100, phase: 'provider reconciliation returned' },
                result: outcome.body,
                request_id: outcome.requestId,
                trace_id: outcome.traceId,
            }, {
                pollable: false,
                reconciliationBefore: { generation: plan.management_generation },
            });
            this.recordCelldResult('cell reconcile', outcome.body, 'celld-cell-result');
        } catch (error) {
            if (error instanceof UnknownMutationOutcomeError) {
                this.promotePendingMutationIntent(pendingIntentId, error);
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
            }
            this.recordCelldResult(
                error instanceof UnknownMutationOutcomeError ? 'cell reconcile outcome unknown' : 'cell reconcile failed',
                { error: error.message, retry_allowed: false },
                'celld-cell-result',
            );
            await this.fetchCelldCell({
                instance: plan.instance_id,
                generation: plan.management_generation,
            });
        } finally {
            this.reviewedCelldReconcile = null;
        }
    }

    parseReviewDocument(id) {
        const text = document.getElementById(id)?.value || '';
        const value = JSON.parse(text);
        if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('review document must be a JSON object');
        return value;
    }

    previewCelldCommand() {
        try {
            const command = this.parseReviewDocument('celld-command');
            const identity = this.celldIdentity();
            const valid = command.document_type === 'instance-cell-command'
                && command.schema_version === '1'
                && command.instance_id === identity.instance
                && command.generation === identity.generation
                && command.operation_id && command.request_hash && command.action;
            if (!valid) throw new Error('command identity/version fields do not bind the selected cell');
            this.reviewedCelldCommand = command;
            this.renderJsonResult('celld-cell-result', { review_required: true, command });
            document.getElementById('celld-apply-command').disabled = false;
        } catch (error) {
            this.reviewedCelldCommand = null;
            document.getElementById('celld-apply-command').disabled = true;
            this.renderJsonResult('celld-cell-result', { valid: false, error: error.message });
        }
    }

    async applyCelldCommand() {
        const command = this.reviewedCelldCommand;
        if (!command || !this.celldCapabilities.has('cell.command')) return;
        document.getElementById('celld-apply-command').disabled = true;
        const path = `/api/v2/celld/cells/${encodeURIComponent(command.instance_id)}/commands`;
        const requestBody = JSON.stringify(command);
        const pendingIntentId = this.trackPendingMutationIntent({
            intentKey: command.operation_id,
            target: command.instance_id,
            kind: 'celld.command',
            retryRequest: {
                path,
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: requestBody,
            },
            pollable: false,
            reconciliationBefore: { generation: command.generation },
        });
        try {
            const outcome = await this.managementRequest(path, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: requestBody,
                expectJson: true,
                idempotencyKey: command.operation_id,
                owner: `celld-command:${command.operation_id}`,
            });
            this.clearPendingMutationIntent(pendingIntentId);
            const effect = (outcome.body?.effects || []).find((candidate) =>
                candidate?.operation_id === command.operation_id);
            const operationState = this.celldEffectOperationState(effect?.status);
            const terminal = ['succeeded', 'failed'].includes(operationState);
            this.trackCanonicalOperation({
                id: `celld-command:${command.operation_id}`,
                kind: 'celld.command',
                state: operationState,
                target: command.instance_id,
                created_at: new Date().toISOString(),
                completed_at: terminal ? new Date().toISOString() : null,
                progress: terminal
                    ? { percent: 100, phase: `effect ledger ${effect.status}` }
                    : { percent: effect?.status === 'dispatched' ? 50 : 10, phase: `effect ledger ${effect?.status || 'not reported'}` },
                result: terminal ? outcome.body : null,
                request_id: outcome.requestId,
                trace_id: outcome.traceId,
            }, {
                intentKey: command.operation_id,
                pollable: false,
                reconciliationBefore: { generation: command.generation },
            });
            this.recordCelldResult('cell command', outcome.body, 'celld-cell-result');
        } catch (error) {
            if (error instanceof UnknownMutationOutcomeError) {
                this.promotePendingMutationIntent(pendingIntentId, error);
            } else {
                this.clearPendingMutationIntent(pendingIntentId);
            }
            this.recordCelldResult(
                error instanceof UnknownMutationOutcomeError ? 'cell command outcome unknown' : 'cell command failed',
                { error: error.message, retry_allowed: error instanceof UnknownMutationOutcomeError },
                'celld-cell-result',
            );
            await this.fetchCelldCell({
                instance: command.instance_id,
                generation: command.generation,
            });
        } finally {
            this.reviewedCelldCommand = null;
        }
    }

    async runCelldReview(kind) {
        const routes = {
            bundle: '/api/v2/celld/bundles/validate', fleet: '/api/v2/celld/fleets/validate',
            preflight: '/api/v2/celld/fleets/preflight', diagnose: '/api/v2/celld/fleets/diagnose',
        };
        const capabilities = {
            bundle: 'bundle.validate', fleet: 'fleet.validate',
            preflight: 'fleet.preflight', diagnose: 'fleet.diagnose',
        };
        if (!routes[kind] || !this.celldCapabilities.has(capabilities[kind])) return;
        try {
            const document = this.parseReviewDocument('celld-review-document');
            const response = (await ApiClient.request(routes[kind], {
                method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(document),
            })).response;
            const body = await response.json().catch(() => ({}));
            this.recordCelldResult(`${kind} review`, {
                mutating: false,
                accepted: response.ok,
                evidence_completeness: kind === 'diagnose' ? (body.live_qualification ? 'live' : 'fixture/local only') : 'contract validation only',
                ...body,
            }, 'celld-review-result');
        } catch (error) {
            this.recordCelldResult(`${kind} review failed`, { mutating: false, error: error.message }, 'celld-review-result');
        }
    }

    async planCelldUpgrade() {
        if (!this.celldCapabilities.has('fleet.plan-upgrade')) return;
        try {
            const manifest = this.parseReviewDocument('celld-review-document');
            const from = document.getElementById('celld-upgrade-from')?.value.trim();
            const to = document.getElementById('celld-upgrade-to')?.value.trim();
            const response = (await ApiClient.request('/api/v2/celld/fleets/plan-upgrade', {
                method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ manifest, from, to }),
            })).response;
            const body = await response.json().catch(() => ({}));
            this.recordCelldResult('upgrade plan review', { accepted: response.ok, ...body }, 'celld-review-result');
            document.getElementById('celld-cancel-plan').disabled = !response.ok;
        } catch (error) {
            this.recordCelldResult('upgrade plan rejected', { error: error.message }, 'celld-review-result');
        }
    }

    cancelCelldPlan() {
        document.getElementById('celld-cancel-plan').disabled = true;
        this.recordCelldResult('upgrade plan cancelled', { mutating: false, cancelled: true }, 'celld-review-result');
    }

    recordCelldResult(kind, evidence, target) {
        this.celldHistory.unshift({ kind, observed_at: new Date().toISOString(), evidence });
        this.celldHistory = this.celldHistory.slice(0, 25);
        this.renderJsonResult(target, { latest: this.celldHistory[0], recovery_history: this.celldHistory });
    }

    // =========================================================================
    // Log Sidebar
    // =========================================================================

    setupLogSidebar() {
        const sidebar = document.getElementById('log-sidebar');
        const toggle = sidebar.querySelector('.sidebar-toggle');
        const tabs = sidebar.querySelectorAll('.tab-btn');
        const filterSelect = document.getElementById('event-filter');
        const clearBtn = document.getElementById('clear-events');
        const reconcileEventsBtn = document.getElementById('reconcile-events');
        const autoScrollCheckbox = document.getElementById('auto-scroll');

        // Toggle sidebar
        toggle.addEventListener('click', () => {
            sidebar.classList.toggle('collapsed');
            toggle.setAttribute('aria-expanded', sidebar.classList.contains('collapsed') ? 'false' : 'true');
        });

        // Tab switching
        tabs.forEach(tab => {
            tab.addEventListener('click', () => {
                tabs.forEach(t => {
                    const active = t === tab;
                    t.classList.toggle('active', active);
                    t.setAttribute('aria-selected', active ? 'true' : 'false');
                    t.tabIndex = active ? 0 : -1;
                });
                tab.classList.add('active');

                const panels = sidebar.querySelectorAll('.log-panel');
                panels.forEach(p => p.classList.remove('active'));

                const targetPanel = document.getElementById(`log-${tab.dataset.tab}`);
                if (targetPanel) targetPanel.classList.add('active');
                if (tab.dataset.tab === 'activity') this.hydrateActivityScope();
            });
            tab.addEventListener('keydown', (event) => {
                const items = [...tabs];
                let index = items.indexOf(tab);
                if (event.key === 'ArrowRight') index = (index + 1) % items.length;
                else if (event.key === 'ArrowLeft') index = (index - 1 + items.length) % items.length;
                else if (event.key === 'Home') index = 0;
                else if (event.key === 'End') index = items.length - 1;
                else return;
                event.preventDefault();
                items[index].focus();
                items[index].click();
            });
        });

        // Event filters (type + level) — full rebuild only on filter change.
        filterSelect.addEventListener('change', (e) => {
            this.eventFilter = e.target.value;
            this.rebuildEventList();
        });
        const eventLevelSelect = document.getElementById('event-level-filter');
        eventLevelSelect?.addEventListener('change', (e) => {
            this.eventLevelFilter = e.target.value;
            this.rebuildEventList();
        });

        // System log filters (level + target).
        const systemLevelSelect = document.getElementById('system-level-filter');
        systemLevelSelect?.addEventListener('change', (e) => {
            this.systemLevelFilter = e.target.value;
            this.rebuildSystemLogsList();
        });
        const systemTargetSelect = document.getElementById('system-target-filter');
        systemTargetSelect?.addEventListener('change', (e) => {
            this.systemTargetFilter = e.target.value;
            this.rebuildSystemLogsList();
        });
        document.getElementById('reconcile-system-logs')?.addEventListener('click', () => this.reconcileSystemLogStream());

        // Clear events — wipe data, dedup set, and DOM.
        clearBtn.addEventListener('click', () => {
            this.logEvents = [];
            this._eventSeenKeys = new Set();
            this.rebuildEventList();
        });
        reconcileEventsBtn?.addEventListener('click', () => this.reconcileEventStream());

        // Auto-scroll toggle
        autoScrollCheckbox.addEventListener('change', (e) => {
            this.autoScroll = e.target.checked;
        });

        // Copy events to clipboard
        const copyBtn = document.getElementById('copy-events');
        copyBtn.addEventListener('click', () => this.copyEventsToClipboard());

        const activityForm = document.getElementById('activity-query-form');
        activityForm?.addEventListener('submit', (event) => {
            event.preventDefault();
            this.fetchActivityTimeline();
        });
        document.getElementById('activity-next')?.addEventListener('click', () => {
            if (this.activityNextCursor) this.fetchActivityTimeline({ cursor: this.activityNextCursor, append: true });
        });
        document.getElementById('activity-export')?.addEventListener('click', () => this.exportActivityTimeline());
        this.hydrateActivityScope();
    }

    hydrateActivityScope() {
        const setIfEmpty = (id, value) => {
            const input = document.getElementById(id);
            if (input && !input.value && value) input.value = String(value);
        };
        try {
            const saved = JSON.parse(sessionStorage.getItem('management-ui-activity-scope') || '{}');
            setIfEmpty('activity-tenant', saved.tenant);
            setIfEmpty('activity-host', saved.host);
            setIfEmpty('activity-instance', saved.instance);
            setIfEmpty('activity-agent', saved.agent);
        } catch (_) {
            // Invalid browser-local convenience state must not block querying.
        }
        const selected = this.instances.get(this.selectedAgent);
        const agent = this.agents.get(this.selectedAgent);
        setIfEmpty('activity-instance', selected?.id || this.selectedAgent);
        setIfEmpty('activity-agent', agent?.id || selected?.id);
        const params = new URLSearchParams(window.location.search);
        setIfEmpty('activity-session', params.get('activity_session'));
        const linkedFilters = {
            activity_event_name: 'activity-event-name', activity_since: 'activity-since',
            activity_until: 'activity-until', activity_plane: 'activity-plane',
            activity_trust: 'activity-trust', activity_outcome: 'activity-outcome',
            activity_mission: 'activity-mission', activity_task: 'activity-task',
            activity_tool: 'activity-tool', activity_command: 'activity-command',
            activity_process: 'activity-process', activity_trace: 'activity-trace',
        };
        for (const [name, id] of Object.entries(linkedFilters)) setIfEmpty(id, params.get(name));
    }

    openActivityEvidence({ instanceId, agentId, sessionId, traceId } = {}) {
        const set = (id, value) => {
            const input = document.getElementById(id);
            if (input && value) input.value = String(value);
        };
        set('activity-instance', instanceId);
        set('activity-agent', agentId);
        set('activity-session', sessionId);
        set('activity-trace', traceId);
        this.switchManagementWorkspace('console');
        document.querySelector('.tab-btn[data-tab="activity"]')?.click();
        const request = this.activityRequest();
        if (request && Object.values(request.scope).every(Boolean)) this.fetchActivityTimeline();
        else this.showToast('Activity scope derived where possible; complete tenant and host to query.', 'info');
    }

    activityRequest() {
        const field = (id) => document.getElementById(id)?.value.trim() || '';
        const scope = {
            tenant: field('activity-tenant'),
            host: field('activity-host'),
            instance: field('activity-instance'),
            agent: field('activity-agent'),
        };
        const query = {};
        const fields = {
            event_name: 'activity-event-name', session_id: 'activity-session',
            mission_id: 'activity-mission', task_id: 'activity-task', tool_call_id: 'activity-tool',
            command_id: 'activity-command', process_id: 'activity-process', trace_id: 'activity-trace',
            plane: 'activity-plane', trust: 'activity-trust', outcome: 'activity-outcome',
        };
        for (const [name, id] of Object.entries(fields)) {
            const value = field(id);
            if (value) query[name] = value;
        }
        for (const [name, id] of [['since', 'activity-since'], ['until', 'activity-until']]) {
            const value = field(id);
            if (value) query[name] = new Date(value).toISOString();
        }
        query.limit = 200;
        const headers = {
            'x-agentic-tenant-id': scope.tenant,
            'x-agentic-host-id': scope.host,
            'x-agentic-instance-id': scope.instance,
            'x-agentic-agent-id': scope.agent,
        };
        return { scope, query, headers };
    }

    async fetchActivityTimeline({ cursor = null, append = false } = {}) {
        const request = this.activityRequest();
        if (!request || Object.values(request.scope).some((value) => !value)) {
            this.showToast('Tenant, host, instance, and agent are required', 'error');
            return;
        }
        const { scope, query, headers } = request;
        if (cursor) query.cursor = cursor;
        this.activityScope = scope;
        this.activityQuery = { ...query, cursor: undefined };
        sessionStorage.setItem('management-ui-activity-scope', JSON.stringify(scope));
        const coverage = document.getElementById('activity-coverage');
        if (coverage) {
            coverage.className = 'activity-coverage unknown';
            coverage.textContent = 'Loading coverage before rendering the timeline…';
        }
        try {
            const queryString = new URLSearchParams(Object.entries(query).filter(([, value]) => value != null));
            const suffix = queryString.toString() ? `?${queryString}` : '';
            const response = (await ApiClient.request(`/api/v2/activity/timeline${suffix}`, { headers })).response;
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const data = await response.json();
            if (cursor && data.cursor_found === false) {
                await this.fetchActivityTimeline({ cursor: null, append: false });
                if (coverage) {
                    const marker = document.createElement('div');
                    marker.textContent = 'Requested Activity cursor is no longer retained. The scoped first page was reconciled; evidence before that page remains incomplete.';
                    coverage.className = 'activity-coverage incomplete';
                    coverage.prepend(marker);
                }
                return;
            }
            if (data.cursor_found === false) throw new Error('activity cursor is invalid for this filtered timeline');
            this.renderActivityCoverage(data.completeness || {}, data.coverage || []);
            this.renderActivityTimeline(data.events || [], { append });
            this.activityNextCursor = data.has_more ? data.next_cursor : null;
            const next = document.getElementById('activity-next');
            if (next) next.disabled = !this.activityNextCursor;
            const page = document.getElementById('activity-page-status');
            if (page) page.textContent = `${data.events?.length || 0} events on this page · ${data.has_more ? 'more available' : 'end of timeline'}`;
            this.focusActivityDeepLink();
        } catch (error) {
            if (coverage) {
                coverage.className = 'activity-coverage incomplete';
                coverage.textContent = `Timeline unavailable; completeness is unknown (${error.message}).`;
            }
            this.renderActivityTimeline([]);
        }
    }

    async exportActivityTimeline() {
        const request = this.activityRequest();
        if (!request || Object.values(request.scope).some((value) => !value)) {
            this.showToast('Tenant, host, instance, and agent are required for export', 'error');
            return;
        }
        try {
            const response = (await ApiClient.request('/api/v2/activity/export', {
                method: 'POST',
                headers: { ...request.headers, 'Content-Type': 'application/json' },
                body: JSON.stringify(request.query),
            })).response;
            if (!response.ok) throw new Error(`HTTP ${response.status}`);
            const evidence = await response.json();
            const blob = new Blob([JSON.stringify(evidence, null, 2)], { type: 'application/json' });
            const link = document.createElement('a');
            link.href = URL.createObjectURL(blob);
            link.download = `activity-evidence-${Date.now()}.json`;
            link.click();
            URL.revokeObjectURL(link.href);
            this.showToast('Signed activity evidence exported', 'success');
        } catch (error) {
            this.showToast(`Activity export unavailable: ${error.message}`, 'error');
        }
    }

    renderActivityCoverage(summary, collectors = []) {
        const coverage = document.getElementById('activity-coverage');
        if (!coverage) return;
        const complete = summary.complete === true;
        coverage.className = `activity-coverage ${complete ? 'complete' : 'incomplete'}`;
        const unsupported = Array.isArray(summary.unsupported_event_classes)
            ? summary.unsupported_event_classes.join(', ') || 'none'
            : 'unknown';
        const overview = document.createElement('div');
        overview.textContent = [
            `Coverage: ${complete ? 'complete' : 'incomplete or unknown'}`,
            `collectors=${summary.collector_count ?? 0}`,
            `gaps=${summary.sequence_gap_count ?? 0}`,
            `durable loss=${summary.durable_loss_count ?? 0}`,
            `dropped=${summary.dropped_event_count ?? 0}`,
            `restarts=${summary.restart_count ?? 0}`,
            `stale=${summary.stale_collector_count ?? 0}`,
            `clock uncertainty=${summary.maximum_clock_error_ms ?? 0}ms`,
            `unsupported=${unsupported}`,
        ].join(' · ');
        coverage.replaceChildren(overview);
        if (collectors.length) {
            const details = document.createElement('details');
            const heading = document.createElement('summary');
            heading.textContent = `Collector coverage (${collectors.length})`;
            details.append(heading);
            for (const collector of collectors) {
                const row = document.createElement('div');
                row.textContent = [
                    collector.collector_id || 'unknown collector',
                    `${collector.event_count ?? 0} events`,
                    `${collector.sequence_gaps?.length ?? 0} gaps`,
                    `${collector.durable_loss_records?.length ?? 0} durable losses`,
                    collector.stale ? 'stale' : 'current',
                    `clock ±${collector.maximum_clock_error_ms ?? 0}ms`,
                ].join(' · ');
                details.append(row);
            }
            coverage.append(details);
        }
    }

    renderActivityTimeline(events, { append = false } = {}) {
        const list = document.getElementById('activity-list');
        if (!list) return;
        const fragment = document.createDocumentFragment();
        for (const event of events) {
            const row = document.createElement('div');
            row.className = 'activity-entry';
            row.id = `activity-event-${String(event.event_id || '').replace(/[^a-zA-Z0-9_-]/g, '-')}`;

            const header = document.createElement('div');
            header.className = 'log-entry-header';
            const name = document.createElement('span');
            name.className = 'log-entry-type';
            name.textContent = String(event.event_name || 'unknown');
            const time = document.createElement('time');
            time.className = 'log-entry-time';
            time.textContent = this.formatEventTime(event.occurred_at);
            header.append(name, time);

            const source = document.createElement('div');
            source.className = 'activity-source';
            const trust = String(event.source?.trust || 'unknown');
            const trustClass = trust === 'self_reported' ? 'self-reported' : trust;
            const badge = document.createElement('span');
            badge.className = `trust-badge ${['observed', 'attested', 'self-reported', 'derived'].includes(trustClass) ? trustClass : 'unknown'}`;
            badge.textContent = trust === 'observed' ? 'independently observed' : trust.replace('_', '-');
            const sourceText = document.createElement('span');
            sourceText.textContent = `${event.source?.layer || 'unknown'} / ${event.source?.collector || 'unknown'}`;
            source.append(badge, sourceText);

            const correlation = document.createElement('div');
            correlation.className = 'log-entry-details';
            const ids = event.correlation || {};
            correlation.textContent = [
                ids.session_id && `session=${ids.session_id}`,
                ids.mission_id && `mission=${ids.mission_id}`,
                ids.task_id && `task=${ids.task_id}`,
                ids.tool_call_id && `tool=${ids.tool_call_id}`,
                ids.command_id && `command=${ids.command_id}`,
                ids.process_id && `process=${ids.process_id}`,
                event.outcome?.status && `outcome=${event.outcome.status}`,
                `sensitivity=${event.sensitivity || 'unknown'}`,
            ].filter(Boolean).join(' · ');
            const links = document.createElement('div');
            const permalink = document.createElement('button');
            permalink.type = 'button';
            permalink.className = 'activity-link';
            permalink.textContent = 'Permalink';
            permalink.addEventListener('click', () => this.updateActivityDeepLink(event));
            links.append(permalink);
            if (ids.session_id) {
                const sessionLink = document.createElement('button');
                sessionLink.type = 'button';
                sessionLink.className = 'activity-link';
                sessionLink.textContent = 'Filter session';
                sessionLink.addEventListener('click', () => {
                    document.getElementById('activity-session').value = ids.session_id;
                    this.fetchActivityTimeline();
                });
                links.append(sessionLink);
            }
            const instance = [...(this.instances || new Map()).values()].find((item) => item.id === ids.instance_id);
            if (instance) {
                const instanceLink = document.createElement('button');
                instanceLink.type = 'button';
                instanceLink.className = 'activity-link';
                instanceLink.textContent = `Open ${instance.name}`;
                instanceLink.addEventListener('click', () => {
                    this.selectedAgent = instance.name;
                    this.updateManagementDeepLink({ instance: instance.name });
                    this.renderVmList();
                });
                links.append(instanceLink);
            }
            row.append(header, source, correlation, links);
            fragment.appendChild(row);
        }
        if (append) list.appendChild(fragment);
        else list.replaceChildren(fragment);
        if (!append && events.length === 0) {
            const empty = document.createElement('div');
            empty.className = 'log-placeholder';
            empty.textContent = 'No activity events matched this authorized scope.';
            list.appendChild(empty);
        }
    }

    updateActivityDeepLink(event) {
        const url = new URL(window.location.href);
        url.searchParams.set('activity_event', event.event_id);
        if (event.correlation?.session_id) url.searchParams.set('activity_session', event.correlation.session_id);
        const filters = this.activityQuery || {};
        const names = {
            event_name: 'activity_event_name', since: 'activity_since', until: 'activity_until',
            plane: 'activity_plane', trust: 'activity_trust', outcome: 'activity_outcome',
            mission_id: 'activity_mission', task_id: 'activity_task', tool_call_id: 'activity_tool',
            command_id: 'activity_command', process_id: 'activity_process', trace_id: 'activity_trace',
        };
        for (const [filter, parameter] of Object.entries(names)) {
            if (filters[filter]) url.searchParams.set(parameter, filters[filter]);
            else url.searchParams.delete(parameter);
        }
        history.replaceState(null, '', url);
        this.focusActivityDeepLink();
    }

    focusActivityDeepLink() {
        const eventId = new URLSearchParams(window.location.search).get('activity_event');
        if (!eventId) return;
        const rowId = `activity-event-${eventId.replace(/[^a-zA-Z0-9_-]/g, '-')}`;
        const row = document.getElementById(rowId);
        row?.scrollIntoView({ block: 'nearest' });
        row?.classList.add('selected');
    }

    // Map a VmEvent.event_type to a UI severity level for filter/styling.
    eventLevelFor(eventType) {
        if (!eventType) return 'info';
        if (eventType.endsWith('.crashed') || eventType.endsWith('.failed')) return 'error';
        if (eventType.endsWith('.disconnected') || eventType.endsWith('.killed') || eventType.endsWith('.shutdown')) return 'warn';
        return 'info';
    }

    // Keep a `<select>`'s option list in sync with a Set of observed values.
    // Preserves the current selection and the leading "All" option.
    _syncFilterOptions(selectEl, knownSet, formatLabel = (v) => v) {
        if (!selectEl) return;
        const current = selectEl.value;
        const sorted = Array.from(knownSet).sort();
        // Detect if the option set changed; cheap signature avoids reflow churn.
        const sig = sorted.join('|');
        if (selectEl._optsSig === sig) return;
        selectEl._optsSig = sig;

        // Capture the first "All" option (always option 0); clear the rest.
        const firstOpt = selectEl.options[0];
        selectEl.innerHTML = '';
        selectEl.appendChild(firstOpt);
        for (const v of sorted) {
            const opt = document.createElement('option');
            opt.value = v;
            opt.textContent = formatLabel(v);
            selectEl.appendChild(opt);
        }
        // Restore selection if still valid; otherwise reset to "all".
        selectEl.value = sorted.includes(current) || current === 'all' ? current : 'all';
    }

    copyEventsToClipboard() {
        // Get filtered events (apply both type and level filters)
        let events = this.logEvents;
        if (this.eventFilter !== 'all') {
            events = events.filter(e => e.event_type === this.eventFilter);
        }
        if (this.eventLevelFilter !== 'all') {
            events = events.filter(e => this.eventLevelFor(e.event_type) === this.eventLevelFilter);
        }

        // Format events as text
        const lines = events.map(event => {
            const time = this.formatEventTime(event.timestamp);
            const type = event.event_type || 'unknown';
            const source = event.agent_id || event.vm_name || 'unknown';

            let details = [];
            if (event.details) {
                if (event.details.hostname) details.push(`host=${event.details.hostname}`);
                if (event.details.ip_address && event.details.ip_address !== 'pending') {
                    details.push(`ip=${event.details.ip_address}`);
                }
                if (event.details.session_id) details.push(`session=${event.details.session_id.slice(0, 8)}`);
                if (event.details.reason) details.push(`reason=${event.details.reason}`);
            }

            const detailStr = details.length > 0 ? ` [${details.join(', ')}]` : '';
            return `${time}  ${type.padEnd(20)}  ${source}${detailStr}`;
        });

        const text = lines.join('\n');
        navigator.clipboard.writeText(text)
            .then(() => this.showToast('Events copied to clipboard', 'success'))
            .catch(() => this.showToast('Failed to copy', 'error'));
    }

    setEventStreamStatus(state, message) {
        this.eventStreamState = state;
        const status = document.getElementById('event-stream-status');
        if (!status) return;
        status.className = `stream-status ${state}`;
        status.textContent = message;
    }

    normalizeEventV2(event) {
        const subject = String(event.subject || '');
        return {
            id: String(event.id || ''),
            event_type: String(event.kind || 'unknown'),
            timestamp: event.timestamp,
            vm_name: subject.startsWith('instance/') ? subject.slice('instance/'.length) : subject,
            details: event.data || {},
        };
    }

    async fetchEvents({ forceSnapshot = false, reconciliationReason = null } = {}) {
        try {
            const params = new URLSearchParams({ limit: String(this.maxLogEvents) });
            if (!forceSnapshot && this.lastEventId) params.set('cursor', String(this.lastEventId));
            const resp = (await ApiClient.request(`/api/v2/admin/events?${params}`)).response;
            if (!resp.ok) {
                const problem = await resp.json().catch(() => ({}));
                if (!forceSnapshot && problem.code === 'stream.invalid_cursor') {
                    this.lastEventId = 0;
                    this.setEventStreamStatus('incomplete', 'Event resume cursor was invalid. Reconciling the bounded snapshot; earlier evidence remains incomplete.');
                    return this.fetchEvents({
                        forceSnapshot: true,
                        reconciliationReason: 'Event resume cursor was invalid.',
                    });
                }
                throw new Error(problem.detail || `HTTP ${resp.status}`);
            }
            const data = await resp.json();
            if (data.gap && !forceSnapshot) {
                this.setEventStreamStatus(
                    'incomplete',
                    'Event cursor fell behind the retained window. Reconciling the bounded snapshot; older events remain unavailable.',
                );
                return this.fetchEvents({
                    forceSnapshot: true,
                    reconciliationReason: 'Event cursor fell behind the retained window.',
                });
            }
            const events = (data.items || [])
                .map((event) => this.normalizeEventV2(event))
                .sort((a, b) => Number(String(b.id).replace(/^ev_/, '')) - Number(String(a.id).replace(/^ev_/, '')));
            this.mergeEvents(events);
            if (data.cursor != null) this.lastEventId = String(data.cursor);
            if (forceSnapshot) {
                this.setEventStreamStatus(
                    'incomplete',
                    `${reconciliationReason || 'Bounded event snapshot requested.'} Bounded snapshot reconciled; earlier events cannot be proven complete.`,
                );
            }
        } catch (e) {
            console.error('Failed to fetch events:', e);
            this.setEventStreamStatus('degraded', `Event reconciliation unavailable (${e.message}).`);
        }
    }

    // Live event stream via SSE. Fetches an initial snapshot first, then opens
    // a follow stream so new events render immediately. Falls back to the 5s
    // polling timer if the stream drops.
    startEventStream() {
        if (this._eventSource) return;
        try {
            const params = new URLSearchParams({ follow: 'true', limit: String(this.maxLogEvents) });
            if (this.lastEventId) params.set('cursor', String(this.lastEventId));
            const es = new EventSource(`/api/v2/admin/events?${params}`);
            this._eventSource = es;

            es.onopen = () => {
                this.setEventStreamStatus('live', `Live event stream connected at cursor ${this.lastEventId || 'latest'}.`);
            };

            es.addEventListener('event', (msg) => {
                if (!msg.data) return;
                try {
                    const ev = this.normalizeEventV2(JSON.parse(msg.data));
                    if (msg.lastEventId) this.lastEventId = String(msg.lastEventId);
                    this.addEvent(ev);
                } catch (e) {
                    console.warn('Bad SSE event payload:', e);
                    this.setEventStreamStatus('degraded', 'Event stream returned an invalid payload; polling reconciliation remains active.');
                }
            });

            es.addEventListener('resync-required', (msg) => {
                console.warn('Event stream requires resync:', msg.data);
                this.setEventStreamStatus('incomplete', 'Event stream gap detected. Reconciling the bounded snapshot now.');
                this.stopEventStream();
                this.fetchEvents({ forceSnapshot: true }).then(() => this.startEventStream());
            });

            es.addEventListener('stream-closed', (msg) => {
                this.setEventStreamStatus('degraded', `Event stream closed by the server (${msg.data || 'no reason'}).`);
                this.stopEventStream();
            });

            es.onerror = () => {
                console.warn('Event stream disconnected; polling fallback continues');
                this.setEventStreamStatus('degraded', `Event stream disconnected at cursor ${this.lastEventId || 'unknown'}; polling reconciliation remains active.`);
            };
        } catch (e) {
            console.error('Failed to start event stream:', e);
        }
    }

    async reconcileEventStream() {
        this.stopEventStream();
        await this.fetchEvents({ forceSnapshot: true });
        this.startEventStream();
    }

    stopEventStream() {
        if (this._eventSource) {
            this._eventSource.close();
            this._eventSource = null;
        }
    }

    // Single-event entry from the SSE stream.
    addEvent(event) {
        this.mergeEvents([event]);
    }

    // Stable key per event for dedup across polling+SSE.
    _eventKey(e) {
        return e.id || `${e.timestamp}|${e.event_type}|${e.agent_id || e.vm_name || ''}`;
    }

    _eventPasses(e) {
        if (this.eventFilter !== 'all' && e.event_type !== this.eventFilter) return false;
        if (this.eventLevelFilter !== 'all' && this.eventLevelFor(e.event_type) !== this.eventLevelFilter) return false;
        return true;
    }

    // Incremental list update: only build/prepend rows for events we haven't
    // seen yet. Filter-passing rows go to the DOM; the rest stay in the data
    // store so a filter change can rebuild without refetching.
    mergeEvents(snapshot) {
        if (!this._eventSeenKeys) this._eventSeenKeys = new Set();

        // Snapshot is newest-first; collect new ones in the same order.
        const newOnes = [];
        for (const e of snapshot) {
            const k = this._eventKey(e);
            if (this._eventSeenKeys.has(k)) continue;
            this._eventSeenKeys.add(k);
            newOnes.push(e);
            if (e.event_type) this._knownEventTypes.add(e.event_type);
        }
        if (newOnes.length === 0) return;

        // Update data store, capped.
        this.logEvents = newOnes.concat(this.logEvents).slice(0, this.maxLogEvents);
        // Re-sync the seen-key set to what's still in the store.
        this._eventSeenKeys = new Set(this.logEvents.map(e => this._eventKey(e)));

        this._syncFilterOptions(document.getElementById('event-filter'), this._knownEventTypes);

        const list = document.getElementById('event-list');
        if (!list) return;

        // Build a fragment for visible new rows only.
        const fragment = document.createDocumentFragment();
        for (const e of newOnes) {
            if (!this._eventPasses(e)) continue;
            const tmp = document.createElement('div');
            tmp.innerHTML = this.renderEventEntry(e);
            const node = tmp.firstElementChild;
            if (node) fragment.appendChild(node);
        }

        const wasAtTop = list.scrollTop <= 4;
        if (fragment.childNodes.length > 0) {
            list.insertBefore(fragment, list.firstChild);
        }
        // Trim DOM tail so it can't grow past the data cap.
        while (list.children.length > this.maxLogEvents) {
            list.removeChild(list.lastElementChild);
        }
        if (this.autoScroll && wasAtTop) list.scrollTop = 0;

        this._updateEventCount();
    }

    // Full rebuild — only used when a filter changes.
    rebuildEventList() {
        const list = document.getElementById('event-list');
        if (!list) return;
        const visible = this.logEvents.filter(e => this._eventPasses(e));
        list.innerHTML = visible.map(e => this.renderEventEntry(e)).join('');
        this._updateEventCount();
    }

    _updateEventCount() {
        const countEl = document.getElementById('event-count');
        if (!countEl) return;
        const visible = this.logEvents.filter(e => this._eventPasses(e));
        countEl.textContent = `${visible.length} events`;
    }

    renderEventEntry(event) {
        const eventType = event.event_type || 'unknown';
        // Handle vm.*, agent.*, and session.* event types
        const shortType = eventType.replace(/^(vm\.|agent\.|session\.)/, '');
        const isAgent = eventType.startsWith('agent.');
        const isSession = eventType.startsWith('session.');
        const cssClass = `event-${cssToken(shortType)}`;

        const time = this.formatEventTime(event.timestamp);
        const source = event.agent_id || event.vm_name || 'unknown';

        let details = '';
        if (event.details) {
            const parts = [];
            if (event.details.hostname) parts.push(`host: ${event.details.hostname}`);
            if (event.details.ip_address && event.details.ip_address !== 'pending') {
                parts.push(`ip: ${event.details.ip_address}`);
            }
            if (event.details.session_id) parts.push(`session: ${event.details.session_id.slice(0, 8)}`);
            if (event.details.command) parts.push(`cmd: ${event.details.command}`);
            if (event.details.reason) parts.push(event.details.reason);
            if (event.details.uptime_seconds) parts.push(`uptime: ${event.details.uptime_seconds}s`);
            // Session reconciliation details
            if (event.details.session_count !== undefined) parts.push(`sessions: ${event.details.session_count}`);
            if (event.details.keep_count !== undefined) parts.push(`kept: ${event.details.keep_count}`);
            if (event.details.kill_count !== undefined) parts.push(`killed: ${event.details.kill_count}`);
            if (event.details.failed_count !== undefined && event.details.failed_count > 0) {
                parts.push(`failed: ${event.details.failed_count}`);
            }
            details = parts.join(' | ');
        }

        // Determine type label prefix
        let typeLabel;
        if (isSession) {
            typeLabel = `session.${shortType}`;
        } else if (isAgent) {
            typeLabel = shortType;
        } else {
            typeLabel = `vm.${shortType}`;
        }

        // Special icons for session events
        let icon = '';
        if (isSession) {
            switch (shortType) {
                case 'query_sent': icon = '&#128269; '; break;      // magnifying glass
                case 'report_received': icon = '&#128203; '; break; // clipboard
                case 'reconcile_started': icon = '&#9881; '; break; // gear
                case 'reconcile_complete': icon = '&#10004; '; break; // checkmark
                case 'killed': icon = '&#10060; '; break;           // X
                case 'preserved': icon = '&#128994; '; break;       // green circle
                case 'reconcile_failed': icon = '&#9888; '; break;  // warning
            }
        }

        return `
            <div class="log-entry ${cssClass}">
                <div class="log-entry-header">
                    <span class="log-entry-type">${icon}${this.esc(typeLabel)}</span>
                    <span class="log-entry-time">${time}</span>
                </div>
                <div class="log-entry-vm">${this.esc(source)}</div>
                ${details ? `<div class="log-entry-details">${this.esc(details)}</div>` : ''}
            </div>
        `;
    }

    formatEventTime(timestamp) {
        if (!timestamp) return '--:--:--';
        const date = new Date(timestamp);
        return date.toLocaleTimeString('en-US', { hour12: false });
    }

    // =========================================================================
    // System Logs
    // =========================================================================

    handleSystemLog(msg) {
        const log = {
            level: msg.level || 'info',
            message: msg.message,
            target: msg.target || '',
            timestamp: msg.timestamp || new Date().toISOString(),
        };
        this.addSystemLog(log);
    }

    addSystemLog(log) {
        this.mergeSystemLogs([log]);
    }

    _systemLogKey(l) {
        return l.id || `${l.timestamp}|${l.target}|${l.message}`;
    }

    _systemLogPasses(l) {
        if (this.systemLevelFilter !== 'all'
            && (l.level || 'INFO').toUpperCase() !== this.systemLevelFilter.toUpperCase()) return false;
        if (this.systemTargetFilter !== 'all' && l.target !== this.systemTargetFilter) return false;
        return true;
    }

    // Incremental list update: prepend only the rows for log entries we
    // haven't seen yet. Polling and (future) streaming both flow through here.
    mergeSystemLogs(snapshot) {
        if (!this._systemSeenKeys) this._systemSeenKeys = new Set();

        const newOnes = [];
        for (const log of snapshot) {
            const k = this._systemLogKey(log);
            if (this._systemSeenKeys.has(k)) continue;
            this._systemSeenKeys.add(k);
            newOnes.push(log);
            if (log.target) this._knownTargets.add(log.target);
        }
        if (newOnes.length === 0) return;

        this.systemLogs = newOnes.concat(this.systemLogs).slice(0, this.maxSystemLogs);
        this._systemSeenKeys = new Set(this.systemLogs.map(l => this._systemLogKey(l)));

        this._syncFilterOptions(
            document.getElementById('system-target-filter'),
            this._knownTargets,
            (v) => v.split('::').pop() || v,
        );

        const list = document.getElementById('system-list');
        if (!list) return;

        // Drop the placeholder once we have real content.
        if (list.querySelector('.log-placeholder')) list.innerHTML = '';

        const fragment = document.createDocumentFragment();
        for (const log of newOnes) {
            if (!this._systemLogPasses(log)) continue;
            const tmp = document.createElement('div');
            tmp.innerHTML = this.renderSystemLogEntry(log);
            const node = tmp.firstElementChild;
            if (node) fragment.appendChild(node);
        }

        const wasAtTop = list.scrollTop <= 4;
        if (fragment.childNodes.length > 0) {
            list.insertBefore(fragment, list.firstChild);
        }
        while (list.children.length > this.maxSystemLogs) {
            list.removeChild(list.lastElementChild);
        }
        if (this.autoScroll && wasAtTop) list.scrollTop = 0;
    }

    rebuildSystemLogsList() {
        const list = document.getElementById('system-list');
        if (!list) return;
        const visible = this.systemLogs.filter(l => this._systemLogPasses(l));
        if (visible.length === 0) {
            list.innerHTML = '<div class="log-placeholder">No system logs</div>';
            return;
        }
        list.innerHTML = visible.map(l => this.renderSystemLogEntry(l)).join('');
    }

    renderSystemLogEntry(log) {
        const time = this.formatEventTime(log.timestamp);
        const level = (log.level || 'INFO').toUpperCase();
        const levelClass = `log-level-${cssToken(level)}`;

        return `
            <div class="log-entry system-log ${levelClass}">
                <div class="log-entry-header">
                    <span class="log-entry-type">${this.esc(level)}</span>
                    <span class="log-entry-time">${time}</span>
                </div>
                ${log.target ? `<div class="log-entry-target">${this.esc(log.target)}</div>` : ''}
                <div class="log-entry-message">${this.esc(log.message)}</div>
            </div>
        `;
    }

    setSystemStreamStatus(state, message) {
        const status = document.getElementById('system-stream-status');
        if (!status) return;
        status.className = `stream-status ${state}`;
        status.textContent = message;
    }

    async fetchSystemLogs({ forceSnapshot = false, reconciliationReason = null } = {}) {
        try {
            const params = new URLSearchParams({ limit: String(this.maxSystemLogs) });
            if (!forceSnapshot && this.lastSystemLogId) params.set('cursor', String(this.lastSystemLogId));
            const resp = (await ApiClient.request(`/api/v2/admin/logs?${params}`)).response;
            if (!resp.ok) {
                const problem = await resp.json().catch(() => ({}));
                if (!forceSnapshot && problem.code === 'stream.invalid_cursor') {
                    this.lastSystemLogId = 0;
                    this.setSystemStreamStatus('incomplete', 'System log resume cursor was invalid. Reconciling the bounded snapshot; earlier evidence remains incomplete.');
                    return this.fetchSystemLogs({
                        forceSnapshot: true,
                        reconciliationReason: 'System log resume cursor was invalid.',
                    });
                }
                throw new Error(problem.detail || `HTTP ${resp.status}`);
            }
            const data = await resp.json();
            if (data.gap && !forceSnapshot) {
                this.setSystemStreamStatus(
                    'incomplete',
                    'System log cursor fell behind the retained window. Reconciling the bounded snapshot.',
                );
                return this.fetchSystemLogs({
                    forceSnapshot: true,
                    reconciliationReason: 'System log cursor fell behind the retained window.',
                });
            }
            const logs = (data.items || []).sort((a, b) => {
                const aId = Number(String(a.id || '').replace(/^log_/, ''));
                const bId = Number(String(b.id || '').replace(/^log_/, ''));
                return bId - aId;
            });
            this.mergeSystemLogs(logs);
            if (data.cursor != null) this.lastSystemLogId = String(data.cursor);
            if (forceSnapshot) {
                this.setSystemStreamStatus(
                    'incomplete',
                    `${reconciliationReason || 'Bounded system log snapshot requested.'} Bounded snapshot reconciled; older evicted entries remain unavailable.`,
                );
            }
        } catch (e) {
            console.error('Failed to fetch system logs:', e);
            this.setSystemStreamStatus('degraded', `System log reconciliation unavailable (${e.message}).`);
        }
    }

    startSystemLogStream() {
        if (this._systemLogEventSource) return;
        const params = new URLSearchParams({ follow: 'true', limit: String(this.maxSystemLogs) });
        if (this.lastSystemLogId) params.set('cursor', String(this.lastSystemLogId));
        try {
            const source = new EventSource(`/api/v2/admin/logs?${params}`);
            this._systemLogEventSource = source;
            source.onopen = () => {
                this.setSystemStreamStatus('live', `Live system log stream connected at cursor ${this.lastSystemLogId || 'latest'}.`);
            };
            source.addEventListener('log', (message) => {
                if (!message.data) return;
                try {
                    const log = JSON.parse(message.data);
                    if (message.lastEventId) this.lastSystemLogId = String(message.lastEventId);
                    this.addSystemLog(log);
                } catch (error) {
                    console.warn('Bad system log SSE payload:', error);
                    this.setSystemStreamStatus('degraded', 'System log stream returned an invalid payload.');
                }
            });
            source.addEventListener('resync-required', () => {
                this.setSystemStreamStatus('incomplete', 'System log stream gap detected. Reconciling the bounded snapshot.');
                this.stopSystemLogStream();
                this.fetchSystemLogs({ forceSnapshot: true }).then(() => this.startSystemLogStream());
            });
            source.onerror = () => {
                this.setSystemStreamStatus(
                    'degraded',
                    `System log stream disconnected at cursor ${this.lastSystemLogId || 'unknown'}; polling remains active.`,
                );
            };
        } catch (error) {
            this.setSystemStreamStatus('degraded', `System log stream unavailable (${error.message}).`);
        }
    }

    stopSystemLogStream() {
        this._systemLogEventSource?.close();
        this._systemLogEventSource = null;
    }

    async reconcileSystemLogStream() {
        this.stopSystemLogStream();
        await this.fetchSystemLogs({ forceSnapshot: true });
        this.startSystemLogStream();
    }

    // =========================================================================
    // Global event listeners
    // =========================================================================

    setupModalAccessibility() {
        const focusable = (modal) => [...modal.querySelectorAll(
            'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        )].filter((element) => !element.hidden && element.getClientRects().length > 0);
        let lastOutsideFocus = document.activeElement;

        document.addEventListener('focusin', (event) => {
            if (!event.target.closest?.('.modal:not(.hidden)')) lastOutsideFocus = event.target;
        });
        for (const modal of document.querySelectorAll('.modal')) {
            modal.setAttribute('aria-hidden', modal.classList.contains('hidden') ? 'true' : 'false');
            modal.addEventListener('keydown', (event) => {
                if (event.key === 'Escape') {
                    event.preventDefault();
                    const dismiss = modal.querySelector('.cancel-btn, .modal-close');
                    if (dismiss) dismiss.click();
                    else modal.classList.add('hidden');
                    return;
                }
                if (event.key !== 'Tab') return;
                const items = focusable(modal);
                if (!items.length) { event.preventDefault(); modal.focus(); return; }
                const first = items[0];
                const last = items[items.length - 1];
                if (event.shiftKey && document.activeElement === first) {
                    event.preventDefault(); last.focus();
                } else if (!event.shiftKey && document.activeElement === last) {
                    event.preventDefault(); first.focus();
                }
            });
            const observer = new MutationObserver(() => {
                const hidden = modal.classList.contains('hidden');
                modal.setAttribute('aria-hidden', hidden ? 'true' : 'false');
                if (!hidden) {
                    modal.__returnFocus = lastOutsideFocus;
                    queueMicrotask(() => {
                        if (!modal.contains(document.activeElement)) {
                            (focusable(modal)[0] || modal).focus();
                        }
                    });
                } else if (modal.__returnFocus?.isConnected) {
                    modal.__returnFocus.focus();
                    modal.__returnFocus = null;
                }
            });
            observer.observe(modal, { attributes: true, attributeFilter: ['class'] });
        }
    }

    setupGlobalListeners() {
        // OAuth modal
        document.querySelector('#oauth-modal .modal-close').addEventListener('click', () => this.hideOAuthModal());
        document.querySelector('#oauth-modal .modal-overlay').addEventListener('click', () => this.hideOAuthModal());
        document.getElementById('oauth-submit').addEventListener('click', () => this.submitOAuthInput());
        document.getElementById('oauth-input').addEventListener('keypress', (e) => {
            if (e.key === 'Enter') this.submitOAuthInput();
        });
        document.getElementById('copy-oauth-url').addEventListener('click', () => {
            navigator.clipboard.writeText(document.getElementById('oauth-link').href)
                .then(() => this.showToast('URL copied', 'success'));
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                this.hideOAuthModal();
                this.hideConfirmDialog();
            }
        });

        // Keepalive
        setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                this.send({ type: 'ping', timestamp: Date.now() });
            }
        }, 30000);

        // Periodic agent refresh
        setInterval(() => this.fetchAgents(), 10000);

        // Periodic event refresh (until WebSocket broadcast is implemented)
        setInterval(() => this.fetchEvents(), 5000);

        // Canonical inventory includes VM, Docker, and host runtimes.
        setInterval(() => this.fetchInstances(), 10000);

        // Periodic system log refresh
        setInterval(() => this.fetchSystemLogs(), 5000);
    }

    // =========================================================================
    // Utilities
    // =========================================================================

    stripAnsi(str) {
        // Remove ANSI escape sequences for clean thumbnail text
        return str.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '')
                  .replace(/\x1b\][^\x07]*\x07/g, '')     // OSC sequences
                  .replace(/\x1b[()][0-9A-B]/g, '')        // charset selection
                  .replace(/\x1b\[[\?]?[0-9;]*[hlsr]/g, '') // mode set/reset
                  .replace(/\r\n/g, '\n')                  // normalize CRLF to LF
                  .replace(/\r/g, '');                     // remove standalone CR
    }

    updateSessionThumbs() {
        for (const [commandId, buf] of this.sessionBuffers) {
            if (!buf.dirty) continue;
            buf.dirty = false;

            // Find the session card's thumb-term element
            const el = document.querySelector(`.session-card[data-session-id="${commandId}"] .thumb-term`);
            if (!el) continue;

            // Split accumulated text on newlines, render last 6 lines
            const lines = buf.text.split('\n');
            const visibleLines = lines.slice(-6);
            el.textContent = visibleLines.join('\n');
        }
    }

    esc(text) {
        return escAttr(text);
    }

    showToast(message, type = 'info') {
        const container = document.getElementById('toast-container');
        const toast = document.createElement('div');
        toast.className = `toast ${type}`;
        toast.textContent = message;
        container.appendChild(toast);
        setTimeout(() => {
            toast.style.animation = 'slideIn 0.3s ease-out reverse';
            setTimeout(() => toast.remove(), 300);
        }, 4000);
    }
}

// === #246 Extension activation chips ===
// Renders color-coded chips per Task showing which A2A extensions were
// activated during that task's lifecycle. Detection is best-effort: we
// infer activation from artifacts left in Task.metadata by the server-side
// extension handlers (e.g. runtime/v1 injects metadata.runtime.*). For
// tasks created before #213's full wiring, absence of evidence is treated
// as "not active" rather than red-flagged.
//
// Color scheme:
//   green  (required-active)   — required extension that left activation evidence
//   yellow (optional-active)   — optional extension that left activation evidence
//   red    (required-missing)  — required extension with no activation evidence
//   (optional + not active is omitted from the chip strip)
//
// Exposed on window.A2AExtChips so the task-list/missions panels rendered
// by adjacent issues (#210, #245, #247) can call it without coupling.
const EXT_REGISTRY = {
    'runtime/v1': {
        uri: 'https://agentic-sandbox.aiwg.io/extensions/runtime/v1',
        required: true,
        label: 'runtime',
        purpose: 'VM/container metadata + instance routing',
        // runtime extension injects metadata.runtime.{instance_id,kind,host}
        detect: (task) => {
            const md = task && task.metadata;
            if (!md) return false;
            if (md.runtime && typeof md.runtime === 'object') {
                return !!(md.runtime.instance_id || md.runtime.kind || md.runtime.host);
            }
            // Flat-shape fallback in case clients flatten the runtime block.
            return !!(md['runtime.instance_id'] || md['runtime.kind'] || md['runtime.host']);
        },
    },
    'idempotency/v1': {
        uri: 'https://agentic-sandbox.aiwg.io/extensions/idempotency/v1',
        required: true,
        label: 'idempotency',
        purpose: '24h dedup on Message.message_id',
        detect: (task) => {
            if (!task) return false;
            const md = task.metadata || {};
            if (md.idempotency_key || md['Idempotent-Replayed']) return true;
            // Header echoed onto the task object in some shapes.
            if (task['Idempotent-Replayed']) return true;
            return false;
        },
    },
    'hitl-prompt/v1': {
        uri: 'https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1',
        required: false,
        label: 'hitl-prompt',
        purpose: 'Structured prompt envelope on INPUT_REQUIRED',
        detect: (task) => {
            if (!task) return false;
            if (task.status && task.status.state === 'input-required') return true;
            const history = task.history || [];
            return history.some((s) => s && s.state === 'input-required');
        },
    },
    'multi-tenant/v1': {
        uri: 'https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1',
        required: false,
        label: 'multi-tenant',
        purpose: 'tenant_id metadata (declared v2.0, enforced v2.2)',
        detect: (task) => !!(task && task.metadata && task.metadata.tenant_id),
    },
    'pty-extensions/v1': {
        uri: 'https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1',
        required: false,
        label: 'pty-ext',
        purpose: 'PTY session frames (controllers, replay)',
        // Best-effort: PTY tasks carry a session_id linking to the PTY stream.
        detect: (task) => !!(task && task.metadata && task.metadata.session_id),
    },
};

function renderExtensionChips(task) {
    const container = document.createElement('div');
    container.className = 'extension-chips';
    for (const [, ext] of Object.entries(EXT_REGISTRY)) {
        const active = ext.detect(task);
        const required = ext.required;
        let status;
        if (active && required) status = 'required-active';
        else if (active && !required) status = 'optional-active';
        else if (!active && required) status = 'required-missing';
        else continue; // not active + not required: omit
        const chip = document.createElement('span');
        chip.className = `ext-chip ext-chip--${status}`;
        chip.dataset.uri = ext.uri;
        chip.dataset.label = ext.label;
        chip.title = `${ext.uri}\n\n${ext.purpose}`;
        chip.textContent = ext.label;
        container.appendChild(chip);
    }
    return container;
}

// Filter a list of tasks to those where the given extension key is active.
// Returns the full list when extKey is falsy (the "All extensions" option).
function filterTasksByExtension(tasks, extKey) {
    if (!extKey) return tasks;
    const ext = EXT_REGISTRY[extKey];
    if (!ext) return tasks;
    return (tasks || []).filter((t) => ext.detect(t));
}

// Wire a <select id="task-ext-filter"> + a task list container together.
// Adjacent issues that render task rows can call this to gain a filter.
//
//   renderFn(tasks): rebuilds the list UI from the filtered tasks
//
// The select is populated from EXT_REGISTRY (any extensions added to the
// registry automatically appear). Calling this multiple times is safe;
// it replaces the previous change listener.
function initExtensionFilter(selectEl, getAllTasks, renderFn) {
    if (!selectEl) return;
    // Repopulate options idempotently so the function is safe to re-call.
    selectEl.innerHTML = '';
    const allOpt = document.createElement('option');
    allOpt.value = '';
    allOpt.textContent = 'All extensions';
    selectEl.appendChild(allOpt);
    for (const [key] of Object.entries(EXT_REGISTRY)) {
        const opt = document.createElement('option');
        opt.value = key;
        opt.textContent = `${key} active`;
        selectEl.appendChild(opt);
    }
    const handler = () => {
        const filtered = filterTasksByExtension(
            typeof getAllTasks === 'function' ? getAllTasks() : getAllTasks,
            selectEl.value,
        );
        if (typeof renderFn === 'function') renderFn(filtered);
    };
    // Replace any prior listener by stashing it on the element.
    if (selectEl._a2aExtHandler) {
        selectEl.removeEventListener('change', selectEl._a2aExtHandler);
    }
    selectEl._a2aExtHandler = handler;
    selectEl.addEventListener('change', handler);
}

// Expose for cross-panel use. Keeping the registry on the namespace means
// other modules/scripts can extend or read it without re-importing.
window.A2AExtChips = {
    REGISTRY: EXT_REGISTRY,
    render: renderExtensionChips,
    filter: filterTasksByExtension,
    initFilter: initExtensionFilter,
};
// === end #246 ===

// Wire the v1 Sunset deprecation banner (#244). Hidden by default; the
// ApiClient surfaces a Sunset header from a v1 fallback response and the
// banner becomes visible until dismissed (per-session via sessionStorage).
function _initSunsetBanner() {
    const banner = document.getElementById('sunset-banner');
    if (!banner) return;
    const dismissBtn = banner.querySelector('.sunset-banner-dismiss');
    const textEl = banner.querySelector('.sunset-banner-text');
    const linkEl = banner.querySelector('.sunset-banner-link');

    if (dismissBtn) {
        dismissBtn.addEventListener('click', () => {
            banner.classList.add('hidden');
            try { sessionStorage.setItem('sunset-dismissed', '1'); } catch (_) {}
        });
    }

    ApiClient.onSunset((path, sunsetDate, linkHeader) => {
        try {
            if (sessionStorage.getItem('sunset-dismissed') === '1') return;
        } catch (_) { /* sessionStorage unavailable — show banner */ }
        if (textEl) {
            textEl.textContent =
                `Deprecated v1 API in use (${path}). Migrate by ${sunsetDate}.`;
        }
        // If the Link header carries a successor-version URL, prefer it over the default.
        if (linkHeader && linkEl) {
            const match = /<([^>]+)>;\s*rel="successor-version"/i.exec(linkHeader);
            if (match) linkEl.href = match[1];
        }
        banner.classList.remove('hidden');
    });
}

// === #250 Deprecation tracking + panel ====================================
//
// Extends the #244 Sunset banner with:
//   1. A per-path hit counter (client-side, populated from ApiClient's
//      Sunset listener; merged with the server-side V1Counter snapshot
//      when GET /api/v2/admin/deprecation/v1-counters succeeds).
//   2. A "Show details" modal that renders the canonical v1→v2 path map
//      alongside the live hit counts.
//   3. Banner copy that includes a running session total — e.g. "v1 routes
//      deprecated by <date>. N v1 hits in this session."
//
// Server-side counts are preferred over client-side because they cover
// requests issued from other clients (sandboxctl, curl, alternate dashboards)
// against the same management process. Client-side counts fall back when
// the snapshot endpoint is unreachable.
// ===========================================================================
const DeprecationTracker = {
    _clientCounts: new Map(),      // path → count, populated by ApiClient.onSunset
    _serverData: null,              // last response from /v1-counters, or null
    _refreshTimer: null,

    init() {
        if (window.ApiClient && ApiClient.onSunset) {
            ApiClient.onSunset((path /*, sunsetDate, linkHeader */) => {
                const key = this._stripQuery(path);
                this._clientCounts.set(key, (this._clientCounts.get(key) || 0) + 1);
                this._updateBannerCount();
            });
        }
        // Wire the "Show details" button on the Sunset banner.
        const btn = document.getElementById('sunset-banner-details-btn');
        if (btn) {
            btn.addEventListener('click', () => this.openModal());
        }
        // Wire the deprecation modal's close button + overlay-click dismiss.
        const modal = document.getElementById('deprecation-modal');
        if (modal) {
            const close = modal.querySelector('.modal-close');
            if (close) close.addEventListener('click', () => this.closeModal());
            const overlay = modal.querySelector('.modal-overlay');
            if (overlay) overlay.addEventListener('click', () => this.closeModal());
        }
        // Poll the server snapshot every 30s so the banner total stays
        // consistent with other clients hitting the same management process.
        const initialFetch = this.fetchServerCounts();
        this._refreshTimer = setInterval(() => this.fetchServerCounts(), 30000);
        return initialFetch;
    },

    _stripQuery(p) {
        if (typeof p !== 'string') return '';
        const i = p.indexOf('?');
        return i === -1 ? p : p.slice(0, i);
    },

    async fetchServerCounts() {
        try {
            // Direct fetch — bypass ApiClient so we don't recursively trigger
            // a Sunset notification on a v2 admin path.
            const r = await fetch('/api/v2/admin/deprecation/v1-counters', {
                headers: { 'Accept': 'application/json' },
            });
            if (r.ok) {
                this._serverData = await r.json();
                this._updateBannerCount();
            } else {
                this._serverData = null;
            }
        } catch (_) {
            this._serverData = null;
        }
    },

    _defaultPathMap() {
        // Mirror compat_v1::path_map() — used when the server endpoint is
        // unreachable. Keep in sync with management/src/http/compat_v1.rs.
        return {
            '/api/v1/agents': '/api/v2/admin/instances',
            '/api/v1/vms': '/api/v2/admin/instances',
            '/api/v1/operations/{id}': '/api/v2/admin/operations/{id}',
            '/api/v1/storage/{scope}/{path}': '/api/v2/admin/storage/{scope}/{path}',
            '/api/v1/container-images': '/api/v2/admin/container-images',
            '/api/v1/sessions/{id}/dispatch': '/agents/{id}/v1/messages:send (A2A)',
            '/api/v1/ws/missions/{id}': '/agents/{id}/v1/tasks/{tid}/subscribe (SSE)',
            '/api/v1/hitl/{id}': 'input-required + hitl-prompt/v1 extension',
        };
    },

    /**
     * Merge server-side and client-side counts. Server counts win on path
     * overlap (they're authoritative across all clients). Client-only
     * paths (e.g. literal paths that don't match a server-side template)
     * are appended so nothing observed in this session is hidden.
     */
    _mergedCounts() {
        const out = {};
        if (this._serverData && this._serverData.counts) {
            for (const [k, v] of Object.entries(this._serverData.counts)) {
                out[k] = v;
            }
        }
        for (const [k, v] of this._clientCounts) {
            if (!(k in out)) out[k] = v;
        }
        return out;
    },

    _totalHits() {
        if (this._serverData && this._serverData.counts) {
            // Prefer server totals — covers requests from other clients.
            return Object.values(this._serverData.counts).reduce((a, b) => a + b, 0);
        }
        let n = 0;
        for (const v of this._clientCounts.values()) n += v;
        return n;
    },

    _updateBannerCount() {
        const banner = document.getElementById('sunset-banner');
        if (!banner) return;
        const text = banner.querySelector('.sunset-banner-text');
        if (!text) return;
        const total = this._totalHits();
        if (total <= 0) return; // leave the original banner copy in place
        const sunset = (this._serverData && this._serverData.sunset_date)
            || 'Sun, 09 May 2027 00:00:00 GMT';
        text.textContent =
            `v1 routes deprecated by ${sunset}. ${total} v1 hit${total === 1 ? '' : 's'} in this session.`;
    },

    async render() {
        // Refresh server data before painting so the modal reflects the
        // most recent snapshot (also catches the "first open" case where
        // the periodic refresh hasn't yet fired).
        await this.fetchServerCounts();

        const panel = document.getElementById('deprecation-panel');
        if (!panel) return;

        const sunset = (this._serverData && this._serverData.sunset_date)
            || 'Sun, 09 May 2027 00:00:00 GMT';
        const guide = (this._serverData && this._serverData.successor_url)
            || 'https://agentic-sandbox.aiwg.io/v2-migration-guide';
        const pathMap = (this._serverData && this._serverData.path_map)
            || this._defaultPathMap();
        const counts = this._mergedCounts();
        const source = this._serverData
            ? 'server (V1Counter)'
            : 'client (observed Sunset headers)';

        const sunsetEl = panel.querySelector('.deprecation-sunset');
        if (sunsetEl) sunsetEl.textContent = sunset;
        const guideEl = panel.querySelector('.deprecation-guide');
        if (guideEl) guideEl.href = guide;
        const sourceEl = panel.querySelector('.deprecation-source');
        if (sourceEl) sourceEl.textContent = source;

        const rows = panel.querySelector('.deprecation-rows');
        const empty = panel.querySelector('.deprecation-empty');
        if (!rows || !empty) return;
        rows.innerHTML = '';

        // Build the row set from the full path map plus any observed paths
        // that aren't in the map (literal paths from real requests vs.
        // templated entries like /api/v1/operations/{id}).
        const allPaths = new Set([
            ...Object.keys(pathMap),
            ...Object.keys(counts),
        ]);
        const entries = Array.from(allPaths)
            .map((p) => [p, counts[p] || 0])
            .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));

        const anyHits = entries.some(([, c]) => c > 0);
        if (!anyHits) {
            empty.classList.remove('hidden');
            return;
        }
        empty.classList.add('hidden');

        for (const [v1path, count] of entries) {
            if (count <= 0) continue; // hide zero-hit rows to keep the table focused
            const v2path = pathMap[v1path] || '(no v2 equivalent — semantic migration)';
            const tr = document.createElement('tr');
            tr.innerHTML =
                `<td><code>${escAttr(v1path)}</code></td>` +
                `<td><code>${escAttr(v2path)}</code></td>` +
                `<td>${count}</td>`;
            rows.appendChild(tr);
        }
    },

    openModal() {
        const modal = document.getElementById('deprecation-modal');
        if (!modal) return;
        this.render();
        modal.classList.remove('hidden');
    },

    closeModal() {
        const modal = document.getElementById('deprecation-modal');
        if (modal) modal.classList.add('hidden');
    },
};

if (typeof window !== 'undefined') window.DeprecationTracker = DeprecationTracker;
// === end #250 ===

// === #248 HITL prompt render ===
// Render the hitl-prompt/v1 envelope from an A2A Task in `input-required`
// state. Read-only: the dashboard observes prompts; responses flow through
// the orchestrator (AIWG) per docs/contracts/extensions/hitl-prompt/v1/spec.md.
//
// Usage (from a future task-detail view):
//   const panel = document.getElementById('hitl-panel-template')
//                   .content.firstElementChild.cloneNode(true);
//   container.appendChild(panel);
//   HitlPrompt.render(task, panel);
//
// `task` is the A2A Task object as returned by /agents/{instance_id}/v1/tasks/{tid}.

const HITL_URI = 'https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1';

const HitlPrompt = {
    URI: HITL_URI,

    /** Pull the hitl-prompt/v1 envelope from a Task.status.message.metadata. */
    extractEnvelope(task) {
        const meta = task && task.status && task.status.message
            ? task.status.message.metadata
            : null;
        if (!meta) return null;
        return meta[HITL_URI] || null;
    },

    /**
     * Minimal markdown-safe renderer. Escapes HTML, then applies a tiny
     * subset of inline markdown (backtick code, **bold**) and preserves
     * newlines as <br>. Intentionally not a full markdown engine — the
     * prompt is operator-facing diagnostic text, not rich content.
     */
    renderMarkdownSafe(text) {
        if (text == null) return '';
        return String(text)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/`([^`]+)`/g, '<code>$1</code>')
            .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
            .replace(/\n/g, '<br>');
    },

    humanizeDuration(ms) {
        const s = Math.floor(Math.abs(ms) / 1000);
        if (s < 60) return `${s}s`;
        if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
        if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
        return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
    },

    /** Start a 1s ticker on `el` rendering time-to/-since the deadline. */
    renderDeadlineCountdown(el, deadlineStr) {
        if (!el) return;
        if (el._hitlTimer) {
            clearInterval(el._hitlTimer);
            el._hitlTimer = null;
        }
        const deadline = new Date(deadlineStr);
        if (Number.isNaN(deadline.getTime())) {
            el.textContent = `deadline: ${deadlineStr} (unparseable)`;
            el.dataset.state = 'invalid';
            return;
        }
        const tick = () => {
            const ms = deadline.getTime() - Date.now();
            if (ms <= 0) {
                el.textContent = `expired ${HitlPrompt.humanizeDuration(ms)} ago`;
                el.dataset.state = 'expired';
                if (el._hitlTimer) {
                    clearInterval(el._hitlTimer);
                    el._hitlTimer = null;
                }
                return;
            }
            el.textContent = `due in ${HitlPrompt.humanizeDuration(ms)}`;
            el.dataset.state = ms < 60000 ? 'urgent' : 'normal';
        };
        el._hitlTimer = setInterval(tick, 1000);
        tick();
    },

    /** Stop any countdown ticker attached to a panel (call on detach). */
    teardown(panel) {
        if (!panel) return;
        const el = panel.querySelector('.hitl-deadline');
        if (el && el._hitlTimer) {
            clearInterval(el._hitlTimer);
            el._hitlTimer = null;
        }
    },

    /**
     * Render the panel for the given task. Shows the panel only when the
     * task is `input-required` or a historical envelope is present.
     * Returns true if rendered, false if hidden.
     */
    render(task, panel) {
        if (!panel) return false;
        const env = HitlPrompt.extractEnvelope(task);
        const state = task && task.status ? task.status.state : null;
        const isInputRequired = state === 'input-required';

        // Render history regardless — terminal tasks may carry past prompts.
        HitlPrompt._renderHistory(task, panel);

        if (!env && !isInputRequired) {
            panel.style.display = 'none';
            return false;
        }
        panel.style.display = '';

        const promptText = panel.querySelector('.hitl-prompt-text');
        const promptIdEl = panel.querySelector('.hitl-prompt-id');
        const deadlineEl = panel.querySelector('.hitl-deadline');
        const respondersEl = panel.querySelector('.hitl-responders');
        const schemaEl = panel.querySelector('.hitl-schema-json');
        const linkEl = panel.querySelector('.hitl-open-orchestrator');

        if (!env) {
            // INPUT_REQUIRED but the envelope is missing — surface that clearly.
            if (promptText) {
                promptText.textContent =
                    'INPUT_REQUIRED but no hitl-prompt/v1 envelope found in metadata.';
            }
            if (promptIdEl) promptIdEl.textContent = '';
            if (deadlineEl) {
                if (deadlineEl._hitlTimer) {
                    clearInterval(deadlineEl._hitlTimer);
                    deadlineEl._hitlTimer = null;
                }
                deadlineEl.textContent = '';
                deadlineEl.dataset.state = '';
            }
            if (respondersEl) respondersEl.textContent = '';
            if (schemaEl) schemaEl.textContent = '';
            if (linkEl) linkEl.style.display = 'none';
            return true;
        }

        if (promptText) {
            promptText.innerHTML = HitlPrompt.renderMarkdownSafe(
                env.prompt || '(no prompt text)',
            );
        }

        if (promptIdEl) {
            promptIdEl.textContent = `prompt_id: ${env.prompt_id || '(missing)'}`;
        }

        if (deadlineEl) {
            if (env.deadline) {
                HitlPrompt.renderDeadlineCountdown(deadlineEl, env.deadline);
            } else {
                if (deadlineEl._hitlTimer) {
                    clearInterval(deadlineEl._hitlTimer);
                    deadlineEl._hitlTimer = null;
                }
                deadlineEl.textContent = '(no deadline)';
                deadlineEl.dataset.state = '';
            }
        }

        if (respondersEl) {
            const responders = Array.isArray(env.allowed_responders) && env.allowed_responders.length
                ? env.allowed_responders
                : ['any'];
            respondersEl.textContent = `responders: ${responders.join(', ')}`;
        }

        if (schemaEl) {
            try {
                schemaEl.textContent = JSON.stringify(
                    env.response_schema || {}, null, 2,
                );
            } catch (e) {
                schemaEl.textContent = '(schema not serializable)';
            }
        }

        if (linkEl) {
            const orchUrl = task && task.metadata ? task.metadata.orchestrator_url : null;
            if (orchUrl) {
                linkEl.href = orchUrl;
                linkEl.style.display = '';
            } else {
                linkEl.removeAttribute('href');
                linkEl.style.display = 'none';
            }
        }

        return true;
    },

    /**
     * Render past input-required statuses from task.history (if present)
     * as a read-only "Prompt history" subsection on terminal tasks.
     */
    _renderHistory(task, panel) {
        const historyContainer = panel.querySelector('.hitl-history');
        const historyList = panel.querySelector('.hitl-history-list');
        if (!historyContainer || !historyList) return;

        const history = task && Array.isArray(task.history) ? task.history : [];
        const pastPrompts = [];
        for (const status of history) {
            if (!status || status.state !== 'input-required') continue;
            const meta = status.message && status.message.metadata
                ? status.message.metadata
                : null;
            const env = meta ? meta[HITL_URI] : null;
            if (!env) continue;
            pastPrompts.push({
                env,
                timestamp: status.timestamp || status.transitioned_at || status.updated_at || null,
                resumed_at: status.resumed_at || null,
            });
        }

        if (!pastPrompts.length) {
            historyContainer.classList.add('hidden');
            historyList.innerHTML = '';
            return;
        }
        historyContainer.classList.remove('hidden');
        historyList.innerHTML = '';
        for (const entry of pastPrompts) {
            const li = document.createElement('li');
            li.className = 'hitl-history-entry';
            const promptDiv = document.createElement('div');
            promptDiv.className = 'hitl-history-prompt';
            promptDiv.innerHTML = HitlPrompt.renderMarkdownSafe(
                entry.env.prompt || '(no prompt text)',
            );
            const metaDiv = document.createElement('div');
            metaDiv.className = 'hitl-history-meta';
            const bits = [];
            bits.push(`prompt_id: ${entry.env.prompt_id || '(missing)'}`);
            if (entry.timestamp) bits.push(`asked: ${entry.timestamp}`);
            if (entry.resumed_at) bits.push(`resumed: ${entry.resumed_at}`);
            metaDiv.textContent = bits.join(' · ');
            li.appendChild(promptDiv);
            li.appendChild(metaDiv);
            historyList.appendChild(li);
        }
    },
};

if (typeof window !== 'undefined') window.HitlPrompt = HitlPrompt;
// === end #248 ===

// === #249 Push notifications ===
// Push notification config CRUD UI for a given A2A task. Calls into the
// server-side handlers at /agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs
// (see management/agentic-sandbox-executor/src/handlers/push_notification.rs).
//
// Wire shape (per server handler):
//   GET    list   → { configs: [{ id, task_id, url, created_at, auth: { type, configured } }] }
//   POST   create → 201 + { id, ..., auth: { type, configured } }   (secret is write-only)
//   DELETE        → 204 no content; cross-task isolation enforced.
//
// Mutating routes require the `A2A-Extensions: runtime/v1` header per #236.
//
// Usage from a future task-detail view:
//   const panel = document.getElementById('push-notifications-panel-template')
//                   .content.firstElementChild.cloneNode(true);
//   container.appendChild(panel);
//   PushNotifications.render(instanceId, taskId, panel);

const PN_RUNTIME_EXT = 'https://agentic-sandbox.aiwg.io/extensions/runtime/v1';

const PushNotifications = {
    _base(instanceId, taskId) {
        return `/agents/${encodeURIComponent(instanceId)}/v1/tasks/${encodeURIComponent(taskId)}/pushNotificationConfigs`;
    },

    async list(instanceId, taskId) {
        const r = await ApiClient.request(this._base(instanceId, taskId));
        if (!r.response.ok) return null;
        return r.response.json(); // { configs: [...] }
    },

    async create(instanceId, taskId, body) {
        const r = await ApiClient.request(this._base(instanceId, taskId), {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'A2A-Extensions': PN_RUNTIME_EXT,
            },
            body: JSON.stringify(body),
        });
        const ok = r.response.ok;
        const body_ = ok ? await r.response.json() : await r.response.text();
        return { ok, status: r.response.status, body: body_ };
    },

    async delete(instanceId, taskId, configId) {
        const r = await ApiClient.request(
            `${this._base(instanceId, taskId)}/${encodeURIComponent(configId)}`,
            {
                method: 'DELETE',
                headers: { 'A2A-Extensions': PN_RUNTIME_EXT },
            }
        );
        return r.response.ok;
    },

    async _testDelivery(instanceId, taskId, configId) {
        // Server-side test delivery isn't implemented yet (separate concern).
        // Call a hypothetical /test endpoint; gracefully degrade on 404.
        const r = await ApiClient.request(
            `${this._base(instanceId, taskId)}/${encodeURIComponent(configId)}/test`,
            {
                method: 'POST',
                headers: { 'A2A-Extensions': PN_RUNTIME_EXT },
            }
        );
        if (r.response.status === 404) {
            return 'Test delivery not yet supported by server (404).';
        }
        if (!r.response.ok) {
            return `Test failed: ${r.response.status} ${r.response.statusText || ''}`.trim();
        }
        try {
            const b = await r.response.json();
            const attempts = b.attempts != null ? ` (attempts: ${b.attempts})` : '';
            return `Delivery: ${b.status_code || 'ok'}${attempts}`;
        } catch (_) {
            return 'Delivery: ok';
        }
    },

    async render(instanceId, taskId, container) {
        if (!container) return;
        const data = await this.list(instanceId, taskId);
        const tbody = container.querySelector('.pn-list');
        const empty = container.querySelector('.pn-empty');
        if (!tbody || !empty) return;
        tbody.innerHTML = '';
        const configs = (data && Array.isArray(data.configs)) ? data.configs : [];
        if (configs.length === 0) {
            empty.classList.remove('hidden');
        } else {
            empty.classList.add('hidden');
            for (const cfg of configs) {
                const tr = document.createElement('tr');
                const authType = (cfg.auth && cfg.auth.type) || 'none';
                const configured = !!(cfg.auth && cfg.auth.configured);
                const chip = configured ? ' <span class="pn-secret-chip" title="Secret configured">&#128274;</span>' : '';
                tr.innerHTML =
                    `<td><code>${escAttr(cfg.id)}</code></td>` +
                    `<td>${escAttr(cfg.url)}</td>` +
                    `<td>${escAttr(authType)}${chip}</td>` +
                    `<td><time>${escAttr(cfg.created_at)}</time></td>` +
                    `<td>` +
                    `<button type="button" class="pn-test-btn" data-id="${escAttr(cfg.id)}">Test</button> ` +
                    `<button type="button" class="pn-delete-btn" data-id="${escAttr(cfg.id)}" aria-label="Delete push notification ${escAttr(cfg.id)}">&times;</button>` +
                    `</td>`;
                tbody.appendChild(tr);
            }
        }
        this._wireActions(instanceId, taskId, container);
    },

    _wireActions(instanceId, taskId, container) {
        container.querySelectorAll('.pn-delete-btn').forEach(btn => {
            btn.onclick = async () => {
                if (!confirm(`Delete subscriber ${btn.dataset.id}?`)) return;
                const ok = await this.delete(instanceId, taskId, btn.dataset.id);
                if (ok) {
                    await this.render(instanceId, taskId, container);
                } else {
                    alert('Delete failed; check server logs.');
                }
            };
        });
        container.querySelectorAll('.pn-test-btn').forEach(btn => {
            btn.onclick = async () => {
                const result = await this._testDelivery(instanceId, taskId, btn.dataset.id);
                alert(result);
            };
        });
        const addBtn = container.querySelector('.pn-add-btn');
        if (addBtn) {
            addBtn.onclick = () => this._openAddModal(instanceId, taskId, container);
        }
    },

    _openAddModal(instanceId, taskId, container) {
        const dlg = document.getElementById('pn-add-modal');
        if (!dlg || typeof dlg.showModal !== 'function') {
            alert('Add-subscriber dialog unavailable.');
            return;
        }
        const form = dlg.querySelector('form');
        form.reset();
        const authSelect = form.querySelector('select[name="auth_type"]');
        const secretField = form.querySelector('.pn-secret-field');
        const secretNote = form.querySelector('.pn-secret-note');
        const secretInput = secretField.querySelector('input');
        const toggleSecret = () => {
            const need = authSelect.value !== 'none';
            secretField.classList.toggle('hidden', !need);
            secretNote.classList.toggle('hidden', !need);
            secretInput.required = need;
            if (!need) secretInput.value = '';
        };
        authSelect.onchange = toggleSecret;
        toggleSecret();
        dlg.onclose = async () => {
            if (dlg.returnValue !== 'confirm') return;
            const fd = new FormData(form);
            const authType = fd.get('auth_type');
            const body = {
                url: fd.get('url'),
                auth: authType === 'none'
                    ? { type: 'none' }
                    : { type: authType, secret: fd.get('secret') },
            };
            const result = await this.create(instanceId, taskId, body);
            if (result.ok) {
                this.render(instanceId, taskId, container);
            } else {
                alert(`Create failed (${result.status}): ${typeof result.body === 'string' ? result.body : JSON.stringify(result.body)}`);
            }
        };
        dlg.showModal();
    },
};

if (typeof window !== 'undefined') window.PushNotifications = PushNotifications;
// === end #249 ===

// === #247 PTY pty-ws.v1 client ===
//
// Per-session WebSocket attach to the v2 binding at
//   /agents/{instance_id}/sessions/{session_id}/attach
// negotiating subprotocol `pty-ws.v1`. Frames are JSON `{op, seq, ts, payload}`
// per docs/contracts/bindings/pty-ws/v1/spec.md (executor uses the simpler
// shape called out in the issue brief, not the longer envelope with `id`
// and `sequence`). Top-level `op` covers `binding_hello`, `output`,
// `resize`, `role_assigned`, `membership_changed`, `keyframe`, `closed`,
// `error`. Outbound verbs use the `pty.*` namespace from
// pty-extensions/v1.
//
// The class is transport-only. It is wired to an xterm Terminal by
// `openPtyV2Session` below, which is what panes use when the user opts
// into v2.

class PtyWsV1Client {
    constructor({
        host,
        instanceId,
        sessionId,
        terminal,
        replayFromSeq = null,
        clientLabel = null,
        requestRole = null,
        wsUrlOverride = null,
    }) {
        this.host = host;
        this.instanceId = instanceId;
        this.sessionId = sessionId;
        this.terminal = terminal;
        this.replayFromSeq = replayFromSeq;
        this.clientLabel = clientLabel;
        this.initialRoleRequest = requestRole;
        this.wsUrlOverride = wsUrlOverride;

        this.ws = null;
        this.lastSeq = 0;
        this.role = null;          // 'controller' | 'observer'
        this.clientId = null;
        this.members = [];
        this.activatedExtensions = [];
        this.supportedOperations = new Set();
        this.bindingHelloReceived = false;
        this.bindingHelloTimer = null;
        this.userInitiatedClose = false;
        this.serverCloseDelivered = false;

        // Callbacks (assigned by caller).
        this.onBindingHello = () => {};
        this.onConnectionChanged = () => {};
        this.onRoleChanged = () => {};
        this.onMembershipChanged = () => {};
        this.onClosed = () => {};
        this.onError = () => {};
        this.onUnknownFrame = () => {};
    }

    _buildUrl() {
        if (this.wsUrlOverride) {
            // Allow tests / custom deployments to point at any URL.
            let url = this.wsUrlOverride;
            if (this.replayFromSeq != null) {
                url += (url.includes('?') ? '&' : '?') + `replay_from=${this.replayFromSeq}`;
            }
            return url;
        }
        const proto = (typeof location !== 'undefined' && location.protocol === 'https:') ? 'wss:' : 'ws:';
        const host = this.host || (typeof location !== 'undefined' ? location.host : '');
        let url = `${proto}//${host}/agents/${encodeURIComponent(this.instanceId)}` +
                  `/sessions/${encodeURIComponent(this.sessionId)}/attach`;
        if (this.replayFromSeq != null) {
            url += `?replay_from=${this.replayFromSeq}`;
        }
        return url;
    }

    connect() {
        const url = this._buildUrl();
        try {
            this.ws = new WebSocket(url, ['pty-ws.v1']);
        } catch (e) {
            this.onError({ kind: 'connect', error: e.message || String(e) });
            return;
        }
        this.ws.binaryType = 'arraybuffer';
        this.ws.onopen = () => {
            if (this.ws.protocol !== 'pty-ws.v1') {
                this.onError({
                    kind: 'subprotocol',
                    expected: 'pty-ws.v1',
                    actual: this.ws.protocol || null,
                });
                try { this.ws.close(1002, 'pty-ws.v1 subprotocol required'); } catch (_) {}
                return;
            }
            this.onConnectionChanged('connected');
            clearTimeout(this.bindingHelloTimer);
            this.bindingHelloTimer = setTimeout(() => {
                if (this.bindingHelloReceived) return;
                this.onError({ kind: 'capability', message: 'Timed out waiting for binding_hello' });
                try { this.ws?.close(1002, 'binding_hello timeout'); } catch (_) {}
            }, 5000);
            // Don't send pty.join_session until we've seen binding_hello;
            // the spec allows it but the executor's session registry may
            // race — being polite gives the server time to flush hello
            // (and to surface a clean error if the subprotocol wasn't
            // echoed).
        };
        this.ws.onmessage = (e) => this._handleRawFrame(e.data);
        this.ws.onclose = (e) => {
            clearTimeout(this.bindingHelloTimer);
            this.bindingHelloTimer = null;
            this.onConnectionChanged('disconnected');
            if (this.serverCloseDelivered) return;
            const reason = e.reason || (this.userInitiatedClose ? 'leave' : 'transport');
            this.onClosed({ code: e.code, reason, userInitiated: this.userInitiatedClose });
        };
        this.ws.onerror = () => {
            this.onConnectionChanged('degraded');
            // The WebSocket spec hides the underlying reason from JS for
            // security; surface a generic transport error.
            this.onError({ kind: 'transport' });
        };
    }

    _handleRawFrame(data) {
        let frame;
        try {
            if (typeof data === 'string') {
                frame = JSON.parse(data);
            } else if (data instanceof ArrayBuffer) {
                frame = JSON.parse(new TextDecoder().decode(data));
            } else {
                // Blob — convert async; rare since we set binaryType=arraybuffer.
                return data.text().then((t) => this._handleRawFrame(t));
            }
        } catch (e) {
            this.onError({ kind: 'parse', error: e.message });
            try { this.ws?.close(1002, 'invalid pty-ws frame'); } catch (_) {}
            return;
        }
        if (frame && typeof frame.seq === 'number') {
            this.lastSeq = frame.seq;
        }
        this._dispatch(frame);
    }

    _dispatch(frame) {
        if (!frame || typeof frame.op !== 'string') {
            this.onUnknownFrame(frame);
            try { this.ws?.close(1002, 'invalid pty-ws frame'); } catch (_) {}
            return;
        }
        if (!this.bindingHelloReceived && frame.op !== 'binding_hello') {
            this.onError({ kind: 'capability', message: 'PTY frame received before binding_hello' });
            try { this.ws?.close(1002, 'frame before binding_hello'); } catch (_) {}
            return;
        }
        if (this.bindingHelloReceived && frame.op === 'binding_hello') {
            this.onError({ kind: 'capability', message: 'Duplicate binding_hello received' });
            try { this.ws?.close(1002, 'duplicate binding_hello'); } catch (_) {}
            return;
        }
        switch (frame.op) {
            case 'binding_hello':
                if (!this.ws || this.ws.protocol !== 'pty-ws.v1') {
                    this.onError({
                        kind: 'subprotocol',
                        expected: 'pty-ws.v1',
                        actual: this.ws?.protocol || null,
                    });
                    try { this.ws?.close(1002, 'pty-ws.v1 subprotocol required'); } catch (_) {}
                    return;
                }
                {
                    const payload = frame.payload || {};
                    const bindingMajor = String(payload.binding_version || '').split('.')[0];
                    const operations = Array.isArray(payload.supported_operations)
                        ? payload.supported_operations.filter((value) => typeof value === 'string')
                        : [];
                    if (payload.binding_uri !== 'https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1'
                        || bindingMajor !== '1' || !operations.includes('pty.join_session')) {
                        this.onError({
                            kind: 'capability',
                            message: 'Incompatible pty-ws binding advertisement',
                            bindingUri: payload.binding_uri || null,
                            bindingVersion: payload.binding_version || null,
                        });
                        try { this.ws?.close(1002, 'incompatible pty-ws binding'); } catch (_) {}
                        return;
                    }
                    this.bindingHelloReceived = true;
                    clearTimeout(this.bindingHelloTimer);
                    this.bindingHelloTimer = null;
                    this.activatedExtensions = Array.isArray(payload.activated_extensions)
                        ? payload.activated_extensions : [];
                    this.supportedOperations = new Set(operations);
                }
                this.onBindingHello(frame.payload || {});
                // Now safe to join.
                this._sendVerb('pty.join_session', this._buildJoinPayload());
                break;
            case 'output': {
                const data = frame.payload && frame.payload.data;
                if (data) this._writeBase64ToTerminal(data);
                break;
            }
            case 'resize': {
                const cols = frame.payload && frame.payload.cols;
                const rows = frame.payload && frame.payload.rows;
                if (cols && rows && this.terminal && typeof this.terminal.resize === 'function') {
                    try { this.terminal.resize(cols, rows); } catch (_) {}
                }
                break;
            }
            case 'role_assigned': {
                const p = frame.payload || {};
                this.role = p.role || this.role;
                if (p.client_id) this.clientId = p.client_id;
                this.onRoleChanged(this.role, this.clientId);
                break;
            }
            case 'membership_changed': {
                this.members = (frame.payload && frame.payload.members) || [];
                this.onMembershipChanged(this.members);
                break;
            }
            case 'keyframe': {
                // Executor packs replay buffer as nested {op, payload}
                // frames inside payload.frames; cursor is the last seq
                // folded in.
                const p = frame.payload || {};
                if (this.terminal && typeof this.terminal.reset === 'function') {
                    try { this.terminal.reset(); } catch (_) {}
                }
                const frames = Array.isArray(p.frames) ? p.frames : [];
                for (const f of frames) {
                    if (!f || typeof f !== 'object') continue;
                    if (f.op === 'output' && f.payload && f.payload.data) {
                        this._writeBase64ToTerminal(f.payload.data);
                    } else if (f.op === 'resize' && f.payload && f.payload.cols && f.payload.rows) {
                        if (this.terminal && typeof this.terminal.resize === 'function') {
                            try { this.terminal.resize(f.payload.cols, f.payload.rows); } catch (_) {}
                        }
                    }
                }
                if (typeof p.cursor === 'number') this.lastSeq = p.cursor;
                break;
            }
            case 'closed': {
                const reason = (frame.payload && frame.payload.reason) || 'session_ended';
                this.userInitiatedClose = false;
                this.serverCloseDelivered = true;
                this.onClosed({ reason, code: null, userInitiated: false, fromServer: true });
                try { if (this.ws) this.ws.close(); } catch (_) {}
                break;
            }
            case 'error': {
                const p = frame.payload || {};
                this.onError({
                    kind: 'server',
                    code: p.code,
                    message: p.message,
                    status: p.status,
                    oldest: p.oldest,
                });
                break;
            }
            default:
                // Could be a task/* response frame; surface to caller.
                this.onUnknownFrame(frame);
        }
    }

    _writeBase64ToTerminal(b64) {
        if (!this.terminal || typeof this.terminal.write !== 'function') return;
        try {
            const raw = atob(b64);
            const bytes = new Uint8Array(raw.length);
            for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
            this.terminal.write(bytes);
        } catch (e) {
            this.onError({ kind: 'decode', error: e.message });
        }
    }

    _buildJoinPayload() {
        const payload = {};
        if (this.initialRoleRequest === 'controller' || this.initialRoleRequest === 'observer') {
            payload.role = this.initialRoleRequest;
        }
        if (this.clientLabel) payload.client_label = this.clientLabel;
        if (this.terminal && this.terminal.cols && this.terminal.rows) {
            payload.cols = this.terminal.cols;
            payload.rows = this.terminal.rows;
        }
        return payload;
    }

    _sendVerb(op, payload) {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false;
        if (!this.bindingHelloReceived) return false;
        if (!this.supportedOperations.has(op)) {
            this.onError({ kind: 'capability', operation: op, message: `Unsupported PTY operation: ${op}` });
            return false;
        }
        if (this.ws.bufferedAmount > 512 * 1024) {
            this.onError({ kind: 'backpressure', bufferedAmount: this.ws.bufferedAmount });
            return false;
        }
        const frame = {
            op,
            ts: new Date().toISOString(),
            payload: payload || {},
        };
        try {
            this.ws.send(JSON.stringify(frame));
            return true;
        } catch (e) {
            this.onError({ kind: 'send', error: e.message });
            return false;
        }
    }

    // ── Public verb API ─────────────────────────────────────────────

    sendInput(text) {
        if (this.role !== 'controller') return false;
        if (typeof text !== 'string' || text.length === 0) return false;
        let b64;
        try {
            // btoa handles single-byte chars only; convert UTF-8 bytes.
            const enc = new TextEncoder().encode(text);
            let bin = '';
            for (let i = 0; i < enc.length; i++) bin += String.fromCharCode(enc[i]);
            b64 = btoa(bin);
        } catch (e) {
            this.onError({ kind: 'encode', error: e.message });
            return false;
        }
        return this._sendVerb('pty.session_input', { data: b64 });
    }

    resize(cols, rows) {
        const c = Number(cols);
        const r = Number(rows);
        if (!Number.isFinite(c) || !Number.isFinite(r) || c < 60 || r < 10) {
            console.log(`[pty.session_resize] dropped reason=floor dims=${cols}x${rows} session=${this.sessionId}`);
            return false;
        }
        console.log(`[pty.session_resize] accepted dims=${c}x${r} session=${this.sessionId}`);
        return this._sendVerb('pty.session_resize', { cols: c, rows: r });
    }

    requestKeyframe() {
        return this._sendVerb('pty.request_keyframe', {});
    }

    requestRole(role) {
        if (role !== 'controller' && role !== 'observer') return false;
        return this._sendVerb('pty.request_role', { role });
    }

    releaseRole() {
        // Spec uses pty.release_role; executor advertises it in
        // binding_hello.supported_operations.
        return this._sendVerb('pty.release_role', {});
    }

    leave() {
        this.userInitiatedClose = true;
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this._sendVerb('pty.leave_session', {});
            try { this.ws.close(1000, 'leave'); } catch (_) {}
        }
        this.ws = null;
    }
}

if (typeof window !== 'undefined') window.PtyWsV1Client = PtyWsV1Client;

// ── Dashboard glue helpers ─────────────────────────────────────────
//
// The pane already owns an xterm Terminal (createPane wires onData /
// onResize via the v1 message bus). For v2 we don't recreate the
// terminal — we open a PtyWsV1Client beside it and re-route its onData/
// onResize through the new client for the lifetime of the v2 attach.

function _ptyV2GetHost() {
    // The v2 binding is served by the executor. In dev, the executor
    // commonly binds at /agents/* on the same host as the dashboard.
    // Allow override via localStorage for non-co-located deployments.
    try {
        const o = localStorage.getItem('pty-v2-host');
        if (o) return o;
    } catch (_) {}
    if (typeof location !== 'undefined' && location.host) return location.host;
    return '';
}

function _ptyV2PreferLegacy() {
    try { return localStorage.getItem('pty-prefer-legacy') === '1'; } catch (_) { return false; }
}

function _ptyV2MaterializeListedUrl(url) {
    if (!url || typeof url !== 'string') return null;
    const host = _ptyV2GetHost();
    const scheme = (typeof location !== 'undefined' && location.protocol === 'https:') ? 'wss' : 'ws';
    return url
        .replace(/^wss:\/\/\{host\}/, `${scheme}://${host}`)
        .replace(/^ws:\/\/\{host\}/, `${scheme}://${host}`)
        .replace('{host}', host);
}

function _ptyV2UpdateRoleBadge(container, role) {
    if (!container) return;
    const badge = container.querySelector('.pty-role-badge');
    if (badge) {
        badge.textContent = `role: ${role || 'unknown'}`;
        badge.dataset.role = role || '';
    }
    const reqBtn = container.querySelector('.pty-request-controller-btn');
    const relBtn = container.querySelector('.pty-release-controller-btn');
    if (reqBtn) reqBtn.style.display = role === 'observer' ? '' : 'none';
    if (relBtn) relBtn.style.display = role === 'controller' ? '' : 'none';
    const terminal = container.querySelector('.xterm-wrapper');
    if (terminal) {
        terminal.dataset.role = role || 'unknown';
        terminal.dataset.readonly = role === 'controller' ? 'false' : 'true';
        _ptyV2RefreshTerminalLabel(terminal);
        terminal.querySelector('.xterm-helper-textarea')
            ?.setAttribute('aria-readonly', role === 'controller' ? 'false' : 'true');
    }
}

function _ptyV2RefreshTerminalLabel(terminal) {
    const agent = terminal.dataset.agentLabel || 'Session';
    const connection = terminal.dataset.connection || 'unknown';
    const role = terminal.dataset.role || 'unknown';
    const posture = terminal.dataset.readonly === 'false' ? 'interactive' : 'read-only';
    terminal.setAttribute('aria-label', `${agent} terminal; connection ${connection}; role ${role}; ${posture}`);
}

function _ptyV2UpdateConnection(container, connection) {
    const terminal = container?.querySelector('.xterm-wrapper');
    if (!terminal) return;
    terminal.dataset.connection = connection || 'unknown';
    _ptyV2RefreshTerminalLabel(terminal);
}

function _ptyV2UpdateMembers(container, members) {
    if (!container) return;
    const countEl = container.querySelector('.pty-member-count');
    if (countEl) countEl.textContent = String(members.length || 0);
    const list = container.querySelector('.pty-member-list');
    if (list) {
        list.innerHTML = '';
        for (const m of members) {
            const li = document.createElement('li');
            const label = m.label || m.client_id || '(unknown)';
            li.textContent = `${label} — ${m.role || 'observer'}`;
            list.appendChild(li);
        }
    }
}

function _ptyV2EnsureToolbar(pane) {
    // Idempotent: returns existing toolbar if already wired.
    if (!pane) return null;
    let toolbar = pane.querySelector('.pty-toolbar');
    if (toolbar) return toolbar;
    toolbar = document.createElement('div');
    toolbar.className = 'pty-toolbar';
    toolbar.innerHTML = `
        <span class="pty-role-badge" data-role="">role: unknown</span>
        <details class="pty-members">
            <summary>Members (<span class="pty-member-count">0</span>)</summary>
            <ul class="pty-member-list"></ul>
        </details>
        <button type="button" class="pty-keyframe-btn" title="Force a fresh keyframe — re-syncs terminal state without disconnecting">⟳ Resync (Keyframe)</button>
        <button type="button" class="pty-request-controller-btn" style="display:none;">Request controller</button>
        <button type="button" class="pty-release-controller-btn" style="display:none;">Release controller</button>
    `;
    // Insert between the pane header and the output region.
    const output = pane.querySelector('.pane-output');
    if (output && output.parentNode === pane) {
        pane.insertBefore(toolbar, output);
    } else {
        pane.appendChild(toolbar);
    }
    return toolbar;
}

// Open a v2 PTY attach against an existing pane. Returns the client.
// On disconnect (non-user-initiated) schedules a reconnect with
// replay_from = lastSeq.
function openPtyV2Session({ pane, agentId, instanceId, sessionId, terminal, replayFromSeq = null, wsUrlOverride = null }) {
    const terminalRegion = pane?.querySelector('.xterm-wrapper');
    if (terminalRegion) {
        terminalRegion.dataset.transport = 'pty-v2';
        terminalRegion.dataset.connection = 'connecting';
        _ptyV2RefreshTerminalLabel(terminalRegion);
    }
    _ptyV2EnsureToolbar(pane);
    // Dispose any prior v2 xterm listeners attached to this terminal so
    // a reconnect doesn't accumulate handlers (each forwards onData →
    // sendInput; duplicates would multi-send keystrokes).
    if (terminal && terminal.__ptyV2Disposables && Array.isArray(terminal.__ptyV2Disposables)) {
        for (const d of terminal.__ptyV2Disposables) {
            try { d && typeof d.dispose === 'function' && d.dispose(); } catch (_) {}
        }
    }
    terminal && (terminal.__ptyV2Disposables = []);
    if (terminal && typeof terminal.reset === 'function') {
        try { terminal.reset(); } catch (_) {}
        try { terminal.write('\x1b[2m[pty-ws.v1 attaching…]\x1b[0m\r\n'); } catch (_) {}
    }

    const client = new PtyWsV1Client({
        host: _ptyV2GetHost(),
        instanceId,
        sessionId,
        terminal,
        replayFromSeq,
        clientLabel: `dashboard@${agentId}`,
        requestRole: 'controller',
        wsUrlOverride,
    });

    // Wire UI callbacks.
    client.onRoleChanged = (role) => _ptyV2UpdateRoleBadge(pane, role);
    client.onConnectionChanged = (state) => _ptyV2UpdateConnection(pane, state);
    client.onMembershipChanged = (members) => _ptyV2UpdateMembers(pane, members);
    client.onError = (err) => {
        try {
            const msg = err.message || err.code || err.kind || 'error';
            terminal.write(`\r\n\x1b[31m[pty-ws error: ${msg}]\x1b[0m\r\n`);
        } catch (_) {}
    };
    client.onClosed = ({ reason, userInitiated, code }) => {
        try { terminal.write(`\r\n\x1b[2m[session disconnected: ${reason}]\x1b[0m\r\n`); } catch (_) {}
        if (userInitiated) return;
        // Only reconnect on unexpected closes. Keep the same
        // pane/terminal; bump replay_from to lastSeq for incremental
        // replay (executor emits a fresh keyframe if it's out of range).
        if (code === 1000) return; // normal closure
        setTimeout(() => {
            // Re-attach to the same session with replay cursor.
            openPtyV2Session({
                pane,
                agentId,
                instanceId,
                sessionId,
                terminal,
                replayFromSeq: client.lastSeq || replayFromSeq,
                wsUrlOverride,
            });
        }, 1000);
    };

    // Bridge xterm onData → client.sendInput. Disposable is tracked on
    // the terminal so a subsequent re-attach disposes it (see top of
    // function) — without that, listeners would stack across reconnects
    // and each keystroke would fan-out multiple session_input frames.
    if (terminal && typeof terminal.onData === 'function') {
        try {
            const dataDisposable = terminal.onData((d) => { client.sendInput(d); });
            client._dataDisposable = dataDisposable;
            terminal.__ptyV2Disposables.push(dataDisposable);
        } catch (_) {}
    }
    if (terminal && typeof terminal.onResize === 'function') {
        try {
            const resizeDisposable = terminal.onResize(({ cols, rows }) => { client.resize(cols, rows); });
            client._resizeDisposable = resizeDisposable;
            terminal.__ptyV2Disposables.push(resizeDisposable);
        } catch (_) {}
    }

    // Wire toolbar buttons (idempotent: replace via cloneNode pattern).
    const kf = pane.querySelector('.pty-keyframe-btn');
    if (kf) kf.onclick = () => client.requestKeyframe();
    const req = pane.querySelector('.pty-request-controller-btn');
    if (req) req.onclick = () => client.requestRole('controller');
    const rel = pane.querySelector('.pty-release-controller-btn');
    if (rel) rel.onclick = () => client.releaseRole();

    client.connect();
    return client;
}

if (typeof window !== 'undefined') {
    window.openPtyV2Session = openPtyV2Session;
    window._ptyV2PreferLegacy = _ptyV2PreferLegacy;
}

// === end #247 ===

document.addEventListener('DOMContentLoaded', async () => {
    _initSunsetBanner();
    // #250: must run after _initSunsetBanner so the banner exists for
    // count-updates. Safe to call even if the API endpoint 503s — the
    // tracker falls back to client-side counts from Sunset listeners.
    // DeprecationTracker disabled — pre-launch, no v1 consumers exist yet.
    // Re-enable by uncommenting when external clients start hitting v1.
    // try { DeprecationTracker.init(); } catch (e) { console.error('DeprecationTracker init failed', e); }
    try {
        await window.ManagementUIReady;
        window.dashboard = new AgenticDashboard();
    } catch (error) {
        console.error('Management UI contract boundary failed to initialize', error);
        return;
    }
    // === #247 wire settings toggle (idempotent) ===
    try {
        const toggle = document.getElementById('pty-legacy-toggle');
        if (toggle) {
            toggle.checked = _ptyV2PreferLegacy();
            toggle.addEventListener('change', (e) => {
                try {
                    localStorage.setItem('pty-prefer-legacy', e.target.checked ? '1' : '0');
                } catch (_) {}
            });
        }
    } catch (_) {}
    // === end #247 ===
});
