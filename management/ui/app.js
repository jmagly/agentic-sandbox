/**
 * Agentic Sandbox Control Plane
 * Per-agent pane dashboard with independent output tracking
 */

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
     * Make a request. Tries v2 path first when a mapping exists; on 404 or
     * network failure, falls back to v1. Surfaces Sunset/Link headers when
     * v1 was used.
     *
     * Returns { response, viaV1: bool, sunsetDate: string | null }.
     *
     * NOTE: This wrapper does NOT consume the response body — the caller
     * still calls resp.json()/resp.text() as before.
     */
    async request(path, opts = {}) {
        const v2Path = ApiClient.toV2(path);
        if (v2Path) {
            try {
                const r = await fetch(v2Path, opts);
                if (r.status !== 404) {
                    return { response: r, viaV1: false, sunsetDate: null };
                }
                // v2 mapped but not mounted yet (or path-level miss) — fall through.
            } catch (e) {
                // Network error on v2 — try v1 as fallback.
                console.warn('v2 request failed; falling back to v1', { v2Path, error: e.message });
            }
        }
        const r = await fetch(path, opts);
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

// Expose for console diagnostics + unit-test page.
if (typeof window !== 'undefined') window.ApiClient = ApiClient;

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
};

function escAttr(s) {
    return String(s == null ? '' : s)
        .replace(/&/g, '&amp;').replace(/"/g, '&quot;')
        .replace(/</g, '&lt;').replace(/>/g, '&gt;');
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
        this.maxReconnectAttempts = 10;
        this.reconnectDelay = 1000;
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

        // VM list state
        this.vms = new Map();  // vm_name -> VM info
        this.containers = new Map();  // container_name -> container info (#178)

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
        this.setupGlobalListeners();
        this.setupLogSidebar();
        this.setupBladeNav();
        this.connect();
        this.fetchAgents();
        this.fetchEvents().then(() => this.startEventStream());
        this.fetchVms();
        this.fetchContainers();
        this.fetchLoadouts();
        this.fetchLoadoutRegistry();
        this.fetchSystemLogs();

        // Refresh session thumbnails every second
        setInterval(() => this.updateSessionThumbs(), 1000);

        // Poll AIWG serve connection status every 5 s
        this.pollAiwgStatus();
        setInterval(() => this.pollAiwgStatus(), 5000);

        // Reconnect button
        document.getElementById('aiwg-reconnect-btn')?.addEventListener('click', () => this.triggerAiwgReconnect());
    }

    // =========================================================================
    // WebSocket
    // =========================================================================

    connect() {
        const wsPort = parseInt(window.location.port || '8122') - 1;
        const wsUrl = `ws://${window.location.hostname}:${wsPort}`;
        console.log(`Connecting to WebSocket at ${wsUrl}`);

        try {
            this.ws = new WebSocket(wsUrl);
            this.ws.onopen = () => this.onOpen();
            this.ws.onmessage = (e) => this.onMessage(e);
            this.ws.onclose = (e) => this.onClose(e);
            this.ws.onerror = (e) => console.error('WebSocket error:', e);
        } catch (error) {
            console.error('WebSocket connection failed:', error);
            this.scheduleReconnect();
        }
    }

    onOpen() {
        console.log('WebSocket connected');
        this.reconnectAttempts = 0;
        this.updateConnectionStatus(true);
        // Clear stale PTY state — server's in-memory command registry resets on restart.
        // Existing panes will rediscover sessions via list_sessions when the agent list arrives.
        this.shellCommandIds.clear();
        this.activeCommandIds.clear();
        this.pendingFirstOutput.clear();
        this.pendingStartupAttach.clear();
        this.send({ type: 'subscribe', agent_id: '*' });
        this.send({ type: 'list_agents' });
    }

    onMessage(event) {
        try {
            const msg = JSON.parse(event.data);
            this.handleMessage(msg);
        } catch (e) {
            console.error('Failed to parse message:', e);
        }
    }

    onClose(event) {
        console.log('WebSocket closed:', event.code);
        this.updateConnectionStatus(false);
        this.scheduleReconnect();
    }

    scheduleReconnect() {
        if (this.reconnectAttempts >= this.maxReconnectAttempts) {
            this.showToast('Connection lost. Refresh the page.', 'error');
            return;
        }
        const delay = this.reconnectDelay * Math.pow(2, this.reconnectAttempts);
        this.reconnectAttempts++;
        console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);
        setTimeout(() => this.connect(), delay);
    }

    send(msg) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(msg));
        }
    }

    // =========================================================================
    // Message dispatch
    // =========================================================================

    handleMessage(msg) {
        switch (msg.type) {
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
                    <button class="pane-vm-btn pane-vm-restart" title="Restart VM (graceful reboot)" data-action="restart">&#10227;</button>
                    <button class="pane-vm-btn pane-vm-stop" title="Stop VM (graceful shutdown — restart from VM list)" data-action="stop">&#9208;</button>
                    <button class="pane-vm-btn pane-vm-kill" title="Force off (hard power off — VM stays defined)" data-action="force-off">&#9211;</button>
                    <button class="pane-shell-btn pane-resync-btn" title="Resync terminal — reset xterm state and re-attach (#180 escape hatch)" data-action="resync">⟳</button>
                    <button class="pane-shell-btn" title="Reconnect to tmux session">Reconnect</button>
                </div>
            </div>
            <div class="pane-setup-progress" style="display:none">
                <div class="setup-progress-header">
                    <span class="setup-progress-icon">&#9881;</span>
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
        outputEl.appendChild(xtermWrapper);
        term.open(xtermWrapper);

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
        // Find VM name from agent ID (convention: agent ID matches VM name)
        const vmName = agentId;

        if (action === 'force-off') {
            this.showConfirmDialog({
                title: 'Force off VM?',
                message: `Hard power off ${vmName}. Any unsaved work will be lost. The VM stays defined and can be restarted.`,
                confirmText: 'Force off',
                confirmClass: 'danger',
                onConfirm: () => this.forceOffVm(vmName)
            });
        } else if (action === 'delete') {
            this.confirmDeleteVm(vmName, /*running=*/true);
        } else if (action === 'restart') {
            this.restartVm(vmName);
        } else if (action === 'stop') {
            this.stopVm(vmName);
        } else if (action === 'start') {
            this.startVm(vmName);
        } else if (action === 'deploy') {
            this.deployAgent(vmName);
        }
    }

    async startVm(name) {
        this.showToast(`Starting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            // === call-sites migrated to ApiClient.request() per #244 ===
            const resp = (await ApiClient.request(`/api/v1/vms/${name}/start`, { method: 'POST' })).response;
            if (resp.ok) {
                this.showToast(`${name} started successfully`, 'success');
                setTimeout(() => this.fetchVms(), 1000);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to start ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Start VM error:', e);
            this.showToast(`Failed to start ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async stopVm(name) {
        this.showToast(`Shutting down ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const stopResp = (await ApiClient.request(`/api/v1/vms/${name}/stop`, { method: 'POST' })).response;
            if (stopResp.ok) {
                this.showToast(`${name} stopped`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await stopResp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to stop ${name}: ${data.error || stopResp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Stop VM error:', e);
            this.showToast(`Failed to stop ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async forceOffVm(name) {
        this.showToast(`Forcing off ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = (await ApiClient.request(`/api/v1/vms/${name}/destroy`, { method: 'POST' })).response;
            if (resp.ok) {
                this.showToast(`${name} powered off`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to force off ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Force off VM error:', e);
            this.showToast(`Failed to force off ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    confirmDeleteVm(name, isRunning) {
        const runningWarn = isRunning
            ? `${name} is currently running and will be force-killed first. `
            : '';
        this.showConfirmDialog({
            title: 'Delete VM?',
            message: `${runningWarn}This permanently undefines ${name} and deletes its disk. Inbox contents are archived to /srv/agentshare/archived/.`,
            confirmText: 'Delete',
            confirmClass: 'danger',
            onConfirm: () => this.deleteVm(name, { force: isRunning, deleteDisk: true })
        });
    }

    async restartVm(name, opts = {}) {
        const { mode = 'graceful', timeoutSeconds = 60 } = opts;
        this.showToast(`Restarting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = (await ApiClient.request(`/api/v1/vms/${name}/restart`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ mode, timeout_seconds: timeoutSeconds })
            })).response;
            if (resp.ok) {
                this.showToast(`${name} restarted`, 'success');
                setTimeout(() => this.fetchVms(), 1000);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to restart ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Restart VM error:', e);
            this.showToast(`Failed to restart ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    setVmButtonsDisabled(vmName, disabled) {
        // Disable buttons in agent pane
        const entry = this.panes.get(vmName);
        if (entry) {
            const buttons = entry.pane.querySelectorAll('.pane-vm-btn');
            buttons.forEach(btn => btn.disabled = disabled);
        }

        // Disable buttons in VM list
        const vmEntry = document.querySelector(`[data-vm-name="${vmName}"]`);
        if (vmEntry) {
            const buttons = vmEntry.querySelectorAll('.vm-ctrl-btn');
            buttons.forEach(btn => btn.disabled = disabled);
        }
    }

    async deleteVm(name, opts = {}) {
        const { force = false, deleteDisk = true } = opts;
        const params = new URLSearchParams();
        if (force) params.set('force', 'true');
        if (deleteDisk) params.set('delete_disk', 'true');

        this.showToast(`Deleting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = (await ApiClient.request(`/api/v1/vms/${name}?${params.toString()}`, { method: 'DELETE' })).response;
            if (resp.ok) {
                this.showToast(`${name} deleted`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to delete ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Delete VM error:', e);
            this.showToast(`Failed to delete ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async deployAgent(name) {
        this.showToast(`Deploying agent to ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = (await ApiClient.request(`/api/v1/vms/${name}/deploy-agent`, { method: 'POST' })).response;
            if (resp.ok || resp.status === 202) {
                const data = await resp.json();
                if (data.operation) {
                    this.showToast(`Agent deployment started on ${name}`, 'success');
                    this.pollDeployOperation(data.operation.id, name);
                } else {
                    this.showToast(`Agent deployed to ${name}`, 'success');
                    setTimeout(() => this.fetchAgents(), 2000);
                }
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to deploy agent: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Deploy agent error:', e);
            this.showToast(`Failed to deploy agent: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async pollDeployOperation(opId, vmName) {
        const maxAttempts = 60; // 5 minutes at 5s intervals
        let attempts = 0;

        const poll = async () => {
            try {
                const resp = (await ApiClient.request(`/api/v1/operations/${opId}`)).response;
                if (!resp.ok) return;

                const op = await resp.json();
                if (op.state === 'completed') {
                    this.showToast(`Agent deployed to ${vmName}!`, 'success');
                    this.fetchAgents();
                    return;
                } else if (op.state === 'failed') {
                    this.showToast(`Agent deployment failed: ${op.error || 'Unknown'}`, 'error');
                    return;
                }

                attempts++;
                if (attempts < maxAttempts) {
                    setTimeout(poll, 5000);
                } else {
                    this.showToast(`Deployment timed out. Check logs.`, 'warning');
                }
            } catch (e) {
                console.error('Poll deploy operation error:', e);
            }
        };

        setTimeout(poll, 3000);
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
            tags.push(`<span class="loadout-tag ${cls}">${loadout.network_mode} network</span>`);
        }

        // AI tools
        for (const tool of (loadout.ai_tools || [])) {
            tags.push(`<span class="loadout-tag tag-ai">${tool.replace(/_/g, ' ')}</span>`);
        }

        // Frameworks
        for (const fw of (loadout.frameworks || [])) {
            tags.push(`<span class="loadout-tag tag-fw">${fw.name}</span>`);
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
        if (submit) submit.textContent = runtime === 'container' ? 'Create container' : 'Create VM';
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
        if (runtime === 'container') return this.handleCreateContainer();
        return this.handleCreateVm();
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

        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();
        this._applyRuntimeVisibility();

        this.showToast(`Creating container ${name}…`, 'info');
        try {
            const resp = (await ApiClient.request('/api/v1/containers', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ name, image }),
            })).response;
            if (resp.ok || resp.status === 201) {
                this.showToast(`${name} created`, 'success');
                setTimeout(() => this.fetchContainers(), 500);
            } else {
                const data = await resp.json().catch(() => ({}));
                this.showToast(`Failed to create ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Create container error:', e);
            this.showToast(`Failed to create ${name}: ${e.message}`, 'error');
        }
    }

    async fetchContainers() {
        try {
            const resp = (await ApiClient.request('/api/v1/containers')).response;
            if (!resp.ok) return;
            const data = await resp.json();
            const list = data.containers || data || [];
            // Stash on instance and re-render the merged list.
            this.containers = new Map(list.map(c => [c.name || c.id, c]));
            this.renderVmList();
        } catch (e) {
            console.error('fetchContainers error:', e);
        }
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
            const init = document.getElementById('vm-init')?.value || 'ubuntu';
            const frameworks = this.getSelectedChips('vm-frameworks');
            const providers = this.getSelectedChips('vm-providers');
            if (!providers.length) {
                this.showToast('Select at least one provider', 'error');
                return;
            }
            body = { name, profile: '', loadout: '', composition: { init, aiwg: { frameworks, providers } }, vcpus, memory_mb, disk_gb, agentshare, start };
        } else {
            const loadout = document.getElementById('vm-loadout').value;
            if (!loadout) {
                this.showToast('Please select a loadout', 'error');
                return;
            }
            body = { name, profile: '', loadout, vcpus, memory_mb, disk_gb, agentshare, start };
        }

        // Close modal
        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();

        // Show progress toast
        this.showToast(`Creating ${name}... This may take several minutes.`, 'info');

        try {
            const resp = (await ApiClient.request('/api/v1/vms', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body)
            })).response;

            if (resp.ok || resp.status === 202) {
                const data = await resp.json();
                if (data.operation) {
                    this.showToast(`${name} provisioning started. Operation: ${data.operation.id}`, 'success');
                    // Poll for operation status
                    this.pollOperation(data.operation.id, name);
                } else {
                    this.showToast(`${name} created successfully`, 'success');
                    setTimeout(() => this.fetchVms(), 2000);
                }
            } else {
                const data = await resp.json().catch(() => ({}));
                const msg = data.error?.message || data.error || resp.statusText;
                this.showToast(`Failed to create ${name}: ${msg}`, 'error');
            }
        } catch (e) {
            console.error('Create VM error:', e);
            this.showToast(`Failed to create ${name}: ${e.message}`, 'error');
        }
    }

    async pollOperation(opId, vmName) {
        const maxAttempts = 120; // 10 minutes at 5s intervals
        let attempts = 0;

        const poll = async () => {
            try {
                const resp = (await ApiClient.request(`/api/v1/operations/${opId}`)).response;
                if (!resp.ok) {
                    console.error('Failed to poll operation:', resp.status);
                    return;
                }

                const op = await resp.json();

                if (op.state === 'completed') {
                    this.showToast(`${vmName} created successfully!`, 'success');
                    this.fetchVms();
                    return;
                } else if (op.state === 'failed') {
                    this.showToast(`${vmName} creation failed: ${op.error || 'Unknown error'}`, 'error');
                    return;
                }

                // Still running, poll again
                attempts++;
                if (attempts < maxAttempts) {
                    setTimeout(poll, 5000);
                } else {
                    this.showToast(`${vmName} creation timed out. Check logs.`, 'warning');
                }
            } catch (e) {
                console.error('Poll operation error:', e);
            }
        };

        // Start polling after 5 seconds
        setTimeout(poll, 5000);
    }

    async fetchVms() {
        try {
            // Only fetch agent-* VMs (default prefix filter)
            const resp = (await ApiClient.request('/api/v1/vms')).response;
            if (!resp.ok) {
                // API not implemented yet
                if (resp.status === 404) {
                    console.log('VM list API not yet implemented');
                    return;
                }
                // 408/503 → libvirt is degraded; fall back to agent-derived rows (#189)
                if (resp.status === 408 || resp.status === 503) {
                    this.vmsDegraded = true;
                    this.renderVmList();
                    return;
                }
                throw new Error(`HTTP ${resp.status}`);
            }
            const data = await resp.json();
            this.vmsDegraded = false;
            if (data.vms) {
                this.updateVmList(data.vms);
            }
        } catch (e) {
            // Network error / fetch threw — treat as libvirt-degraded so the
            // sidebar still shows agent rows instead of going blank (#189).
            this.vmsDegraded = true;
            this.renderVmList();
            console.error('Failed to fetch VMs:', e);
        }
    }

    updateVmList(vms) {
        // Update internal state then defer to renderVmList for the merged
        // VM + container view (#178).
        this.vms.clear();
        vms.forEach(vm => this.vms.set(vm.name, vm));
        this.renderVmList();
    }

    // Render the merged Instances list (VMs + containers). Called both by
    // updateVmList (after a VM poll) and fetchContainers (after a container poll).
    renderVmList() {
        const list = document.getElementById('vm-list');
        if (!list) return;

        const vmEntries = Array.from(this.vms.values()).map(vm => ({
            name: vm.name,
            runtime: 'vm',
            state: vm.state,
            raw: vm,
        }));
        const containerEntries = Array.from((this.containers || new Map()).values()).map(c => ({
            name: c.name || c.id,
            runtime: 'container',
            state: c.state || c.status || 'running',
            raw: c,
        }));

        // Libvirt-degraded fallback (#189): when /api/v1/vms is unavailable
        // (timeout / 5xx) the operator still needs to see agents that ARE
        // gRPC-connected. Synthesize a VM row for each known agent that
        // isn't already represented (and isn't a container). The synthesized
        // rows carry `_degraded: true` so renderVmEntry can show a chip and
        // disable lifecycle controls that need libvirt RPC.
        if (this.vmsDegraded) {
            const knownNames = new Set([
                ...vmEntries.map(e => e.name),
                ...containerEntries.map(e => e.name),
            ]);
            for (const [agentId, agentInfo] of this.agents.entries()) {
                if (knownNames.has(agentId)) continue;
                vmEntries.push({
                    name: agentId,
                    runtime: 'vm',
                    state: 'running',
                    raw: {
                        name: agentId,
                        state: 'running',
                        _degraded: true,
                        _agentInfo: agentInfo,
                    },
                });
            }
        }

        const all = [...vmEntries, ...containerEntries].sort((a, b) => a.name.localeCompare(b.name));

        if (all.length === 0) {
            list.innerHTML = '<div class="vm-placeholder">No instances found</div>';
            this.updateVmCount();
            return;
        }

        // Top-of-sidebar banner when libvirt is degraded.
        const degradedBanner = this.vmsDegraded
            ? `<div class="vm-degraded-banner" title="GET /api/v1/vms is unavailable. Lifecycle controls disabled until libvirt recovers.">⚠ libvirt unresponsive — VM lifecycle controls unavailable</div>`
            : '';

        list.innerHTML = degradedBanner + all.map(e => e.runtime === 'container'
            ? this.renderContainerEntry(e.raw)
            : this.renderVmEntry(e.raw)).join('');

        list.querySelectorAll('.blade-item').forEach(item => {
            const name = item.dataset.vmName;
            const runtime = item.dataset.runtime || 'vm';
            const entry = runtime === 'container'
                ? (this.containers && this.containers.get(name))
                : this.vms.get(name);
            if (!entry) return;

            item.addEventListener('click', (e) => {
                if (e.target.closest('.vm-controls')) return;
                if (this.panes.has(name)) {
                    this.openSessionsBlade(name);
                } else if (runtime === 'vm' && entry.state !== 'running') {
                    this.showToast(`${name} is not running`, 'info');
                } else {
                    this.showToast(`${name} agent not connected`, 'info');
                }
            });

            // Shared controls
            item.querySelector('.vm-stop')?.addEventListener('click', (e) => {
                e.stopPropagation();
                if (runtime === 'container') return this.stopContainer(name);
                this.stopVm(name);
            });
            item.querySelector('.vm-delete')?.addEventListener('click', (e) => {
                e.stopPropagation();
                if (runtime === 'container') return this.confirmDeleteContainer(name);
                this.confirmDeleteVm(name, entry.state === 'running');
            });

            // VM-only controls
            if (runtime === 'vm') {
                item.querySelector('.vm-start')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this.startVm(name);
                });
                item.querySelector('.vm-restart')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this.restartVm(name);
                });
                item.querySelector('.vm-force-off')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this.showConfirmDialog({
                        title: 'Force off VM?',
                        message: `Hard power off ${name}. Any unsaved work will be lost. The VM stays defined and can be restarted.`,
                        confirmText: 'Force off',
                        confirmClass: 'danger',
                        onConfirm: () => this.forceOffVm(name)
                    });
                });
                item.querySelector('.vm-deploy')?.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this.deployAgent(name);
                });
            }
        });

        this.updateVmCount();
    }

    renderVmEntry(vm) {
        const stateClass = vm.state.toLowerCase().replace(' ', '-');
        const isRunning = vm.state === 'running';
        const isStopped = vm.state === 'shut off' || vm.state === 'stopped';
        const hasAgent = this.panes.has(vm.name);
        const sessionCount = this.vmSessions.get(vm.name)?.length || 0;
        const selected = vm.name === this.selectedAgent ? 'selected' : '';

        // Status icon
        const statusIcon = isRunning ? (hasAgent ? '●' : '○') : '○';
        const statusClass = isRunning ? (hasAgent ? 'running' : '') : 'stopped';

        // Session badge
        const badgeStyle = sessionCount > 0 ? '' : 'display:none';
        const badge = hasAgent ? `<span class="blade-item-badge" style="${badgeStyle}">${sessionCount}</span>` : '';

        // Libvirt-degraded marker (#189): when this row was synthesized from
        // /api/v1/agents because /api/v1/vms is unavailable, lifecycle buttons
        // that require libvirt RPC are disabled with a tooltip explaining why.
        // Reconnect/Deploy stay live since they don't need libvirt.
        const degraded = vm._degraded === true;
        const degradedAttr = degraded ? 'disabled title="libvirt unresponsive"' : '';
        const degradedChip = degraded
            ? `<span class="runtime-badge runtime-degraded" title="libvirt unresponsive — agent visible via gRPC heartbeat">⚠</span>`
            : '';

        // VM control buttons based on state
        let vmControls = '';
        if (isRunning) {
            const deployBtn = !hasAgent
                ? `<button class="vm-ctrl-btn vm-deploy" title="Deploy Agent">⚡</button>`
                : '';
            vmControls = `
                <div class="vm-controls">
                    ${deployBtn}
                    <button class="vm-ctrl-btn vm-restart" title="Restart VM (graceful reboot)" ${degradedAttr}>↻</button>
                    <button class="vm-ctrl-btn vm-stop" title="Stop VM (graceful shutdown)" ${degradedAttr}>■</button>
                    <button class="vm-ctrl-btn vm-force-off" title="Force off (hard power off — VM stays defined)" ${degradedAttr}>⏻</button>
                    <button class="vm-ctrl-btn vm-delete" title="Delete VM (permanent — wipes disk)" ${degradedAttr}>✕</button>
                </div>
            `;
        } else if (isStopped) {
            vmControls = `
                <div class="vm-controls">
                    <button class="vm-ctrl-btn vm-start" title="Start VM" ${degradedAttr}>▶</button>
                    <button class="vm-ctrl-btn vm-delete" title="Delete VM (permanent — wipes disk)" ${degradedAttr}>🗑</button>
                </div>
            `;
        }

        // Loadout label from agent data
        const agentData = this.agents.get(vm.name);
        const loadoutLabel = agentData?.loadout ? `<span class="blade-item-loadout">${this.esc(agentData.loadout)}</span>` : '';

        return `
            <div class="blade-item ${statusClass} ${selected}" data-vm-name="${this.esc(vm.name)}" data-runtime="vm">
                <span class="blade-item-icon">${statusIcon}</span>
                <div class="blade-item-info">
                    <span class="blade-item-name">${this.esc(vm.name)}<span class="runtime-badge runtime-vm" title="VM (libvirt)">VM</span>${degradedChip}${badge}</span>
                    ${loadoutLabel}
                </div>
                ${vmControls}
            </div>
        `;
    }

    renderContainerEntry(c) {
        const name = c.name || c.id;
        const isRunning = (c.state || c.status || '').toLowerCase().startsWith('running');
        const hasAgent = this.panes.has(name);
        const sessionCount = this.vmSessions.get(name)?.length || 0;
        const selected = name === this.selectedAgent ? 'selected' : '';
        const statusIcon = isRunning ? (hasAgent ? '●' : '○') : '○';
        const statusClass = isRunning ? (hasAgent ? 'running' : '') : 'stopped';
        const badgeStyle = sessionCount > 0 ? '' : 'display:none';
        const badge = hasAgent ? `<span class="blade-item-badge" style="${badgeStyle}">${sessionCount}</span>` : '';
        const imageLabel = c.image ? `<span class="blade-item-loadout">${this.esc(c.image)}</span>` : '';

        // Containers expose Stop + Delete only. No Force off (use Delete with running),
        // no Restart yet (no /containers/{name}/restart route), no Deploy (image is baked).
        const controls = isRunning
            ? `<div class="vm-controls">
                 <button class="vm-ctrl-btn vm-stop" title="Stop container (graceful)">■</button>
                 <button class="vm-ctrl-btn vm-delete" title="Delete container (force-removes if running)">✕</button>
               </div>`
            : `<div class="vm-controls">
                 <button class="vm-ctrl-btn vm-delete" title="Delete container">🗑</button>
               </div>`;

        return `
            <div class="blade-item ${statusClass} ${selected}" data-vm-name="${this.esc(name)}" data-runtime="container">
                <span class="blade-item-icon">${statusIcon}</span>
                <div class="blade-item-info">
                    <span class="blade-item-name">${this.esc(name)}<span class="runtime-badge runtime-ct" title="Container (Docker)">CT</span>${badge}</span>
                    ${imageLabel}
                </div>
                ${controls}
            </div>
        `;
    }

    async stopContainer(name) {
        this.showToast(`Stopping ${name}…`, 'info');
        try {
            const resp = (await ApiClient.request(`/api/v1/containers/${name}/stop`, { method: 'POST' })).response;
            if (resp.ok) {
                this.showToast(`${name} stopped`, 'success');
                setTimeout(() => this.fetchContainers(), 500);
            } else {
                const data = await resp.json().catch(() => ({}));
                this.showToast(`Failed to stop ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Stop container error:', e);
            this.showToast(`Failed to stop ${name}: ${e.message}`, 'error');
        }
    }

    confirmDeleteContainer(name) {
        this.showConfirmDialog({
            title: 'Delete container?',
            message: `Force-remove container ${name}. Any unsaved data inside the container will be lost.`,
            confirmText: 'Delete',
            confirmClass: 'danger',
            onConfirm: () => this.deleteContainer(name),
        });
    }

    async deleteContainer(name) {
        this.showToast(`Deleting ${name}…`, 'info');
        try {
            const resp = (await ApiClient.request(`/api/v1/containers/${name}`, { method: 'DELETE' })).response;
            if (resp.ok) {
                this.showToast(`${name} deleted`, 'success');
                setTimeout(() => this.fetchContainers(), 500);
            } else {
                const data = await resp.json().catch(() => ({}));
                this.showToast(`Failed to delete ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Delete container error:', e);
            this.showToast(`Failed to delete ${name}: ${e.message}`, 'error');
        }
    }

    getVmStateIcon(state) {
        switch (state.toLowerCase()) {
            case 'running': return '&#9679;'; // filled circle
            case 'shut off':
            case 'stopped': return '&#9675;'; // empty circle
            case 'crashed':
            case 'paused': return '&#9888;'; // warning triangle
            default: return '&#9676;'; // dotted circle
        }
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

        const close = () => modal.classList.add('hidden');
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

        // Identity
        sections.push(`
            <div class="detail-section">
                <div class="detail-section-title">Identity</div>
                <div class="detail-grid">
                    <div class="detail-label">Agent ID</div><div class="detail-value">${this.esc(agent.id)}</div>
                    <div class="detail-label">Hostname</div><div class="detail-value">${this.esc(agent.hostname)}</div>
                    <div class="detail-label">IP Address</div><div class="detail-value">${this.esc(agent.ip_address)}</div>
                    <div class="detail-label">Status</div><div class="detail-value"><span class="detail-status-badge ${agent.status.toLowerCase()}">${this.esc(agent.status)}</span></div>
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

    updateConnectionStatus(connected) {
        const el = document.getElementById('connection-status');
        const text = el.querySelector('.status-text');
        if (connected) {
            el.className = 'status-connected';
            text.textContent = 'Connected';
        } else {
            el.className = 'status-disconnected';
            text.textContent = 'Disconnected';
        }
    }

    updateAgentCount() {
        document.getElementById('agent-count').textContent =
            `${this.agents.size} agent${this.agents.size !== 1 ? 's' : ''}`;
    }

    updateVmCount() {
        const vmCountEl = document.getElementById('vm-count');
        if (vmCountEl) {
            const total = this.vms.size;
            const running = Array.from(this.vms.values()).filter(vm =>
                vm.state === 'running' || vm.state === 'Running'
            ).length;
            vmCountEl.textContent = running === total
                ? `${total} VM${total !== 1 ? 's' : ''}`
                : `${running}/${total} VMs`;
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
            const typeClass = (session.session_type || 'background').toLowerCase();
            const name = session.session_name || session.command_id?.slice(0, 12) || 'session';

            // Pre-populate thumbnail from existing buffer if available
            const buf = this.sessionBuffers.get(session.command_id);
            const thumbText = buf ? this.esc(buf.text.split('\n').slice(-6).join('\n')) : '';

            return `
                <div class="session-card" data-session-id="${this.esc(session.command_id)}">
                    <div class="session-thumb" data-command-id="${this.esc(session.command_id)}">
                        <pre class="thumb-term">${thumbText}</pre>
                    </div>
                    <div class="session-card-info">
                        <span class="session-card-name">${this.esc(name)}</span>
                        <span class="session-card-type ${typeClass}">${typeClass.slice(0, 3)}</span>
                        <button class="session-card-kill" title="Kill">✕</button>
                    </div>
                </div>
            `;
        }).join('');

        // Attach handlers
        const vmName = this.selectedVmForSessions;
        list.querySelectorAll('.session-card').forEach(card => {
            const sessionId = card.dataset.sessionId;

            card.addEventListener('click', (e) => {
                if (e.target.classList.contains('session-card-kill')) return;
                this.connectToSession(vmName, sessionId);
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
    // Log Sidebar
    // =========================================================================

    setupLogSidebar() {
        const sidebar = document.getElementById('log-sidebar');
        const toggle = sidebar.querySelector('.sidebar-toggle');
        const tabs = sidebar.querySelectorAll('.tab-btn');
        const filterSelect = document.getElementById('event-filter');
        const clearBtn = document.getElementById('clear-events');
        const autoScrollCheckbox = document.getElementById('auto-scroll');

        // Toggle sidebar
        toggle.addEventListener('click', () => {
            sidebar.classList.toggle('collapsed');
        });

        // Tab switching
        tabs.forEach(tab => {
            tab.addEventListener('click', () => {
                tabs.forEach(t => t.classList.remove('active'));
                tab.classList.add('active');

                const panels = sidebar.querySelectorAll('.log-panel');
                panels.forEach(p => p.classList.remove('active'));

                const targetPanel = document.getElementById(`log-${tab.dataset.tab}`);
                if (targetPanel) targetPanel.classList.add('active');
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

        // Clear events — wipe data, dedup set, and DOM.
        clearBtn.addEventListener('click', () => {
            this.logEvents = [];
            this._eventSeenKeys = new Set();
            this.rebuildEventList();
        });

        // Auto-scroll toggle
        autoScrollCheckbox.addEventListener('change', (e) => {
            this.autoScroll = e.target.checked;
        });

        // Copy events to clipboard
        const copyBtn = document.getElementById('copy-events');
        copyBtn.addEventListener('click', () => this.copyEventsToClipboard());
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

    async fetchEvents() {
        try {
            const resp = (await ApiClient.request('/api/v1/events')).response;
            const data = await resp.json();
            if (data.last_event_id && data.last_event_id === this.lastEventId) return;
            this.lastEventId = data.last_event_id || 0;
            this.mergeEvents(data.events || []);
        } catch (e) {
            console.error('Failed to fetch events:', e);
        }
    }

    // Live event stream via SSE. Fetches an initial snapshot first, then opens
    // a follow stream so new events render immediately. Falls back to the 5s
    // polling timer if the stream drops.
    startEventStream() {
        if (this._eventSource) return;
        try {
            const since = encodeURIComponent(new Date().toISOString());
            const es = new EventSource(`/api/v1/events?follow=true&since=${since}`);
            this._eventSource = es;

            es.onmessage = (msg) => {
                if (!msg.data) return;
                try {
                    const ev = JSON.parse(msg.data);
                    this.addEvent(ev);
                } catch (e) {
                    console.warn('Bad SSE event payload:', e);
                }
            };

            es.addEventListener('lagged', (msg) => {
                console.warn('Event stream lagged:', msg.data);
                // Reconnect with a fresh snapshot so we don't miss the gap.
                this.stopEventStream();
                this.fetchEvents().then(() => this.startEventStream());
            });

            es.onerror = () => {
                // Browser will auto-retry; nothing to do but log.
                console.warn('Event stream disconnected; polling fallback continues');
            };
        } catch (e) {
            console.error('Failed to start event stream:', e);
        }
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
        return `${e.timestamp}|${e.event_type}|${e.agent_id || e.vm_name || ''}`;
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
        const cssClass = `event-${shortType.replace(/[._]/g, '-')}`;

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
        return `${l.timestamp}|${l.target}|${l.message}`;
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
        const levelClass = `log-level-${level.toLowerCase()}`;

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

    async fetchSystemLogs() {
        try {
            const resp = (await ApiClient.request('/api/v1/logs?limit=200')).response;
            if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
            const data = await resp.json();
            this.mergeSystemLogs(data.logs || []);
        } catch (e) {
            console.error('Failed to fetch system logs:', e);
        }
    }

    // =========================================================================
    // Global event listeners
    // =========================================================================

    setupGlobalListeners() {
        // OAuth modal
        document.querySelector('.modal-close').addEventListener('click', () => this.hideOAuthModal());
        document.querySelector('.modal-overlay').addEventListener('click', () => this.hideOAuthModal());
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

        // Periodic VM list refresh
        setInterval(() => this.fetchVms(), 10000);
        // Mirror VM polling for containers (#178). Backend SSE for container.* events
        // would let us drop polling entirely — see follow-up.
        setInterval(() => this.fetchContainers(), 10000);

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
        const d = document.createElement('div');
        d.textContent = text;
        return d.innerHTML;
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
        this.fetchServerCounts();
        this._refreshTimer = setInterval(() => this.fetchServerCounts(), 30000);
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
                    `<button type="button" class="pn-delete-btn" data-id="${escAttr(cfg.id)}">&times;</button>` +
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
                    this.render(instanceId, taskId, container);
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
        this.bindingHelloReceived = false;
        this.userInitiatedClose = false;

        // Callbacks (assigned by caller).
        this.onBindingHello = () => {};
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
            // Don't send pty.join_session until we've seen binding_hello;
            // the spec allows it but the executor's session registry may
            // race — being polite gives the server time to flush hello
            // (and to surface a clean error if the subprotocol wasn't
            // echoed).
        };
        this.ws.onmessage = (e) => this._handleRawFrame(e.data);
        this.ws.onclose = (e) => {
            const reason = e.reason || (this.userInitiatedClose ? 'leave' : 'transport');
            this.onClosed({ code: e.code, reason, userInitiated: this.userInitiatedClose });
        };
        this.ws.onerror = () => {
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
            return;
        }
        switch (frame.op) {
            case 'binding_hello':
                this.bindingHelloReceived = true;
                this.activatedExtensions = (frame.payload && frame.payload.activated_extensions) || [];
                if (this.ws && this.ws.protocol && this.ws.protocol !== 'pty-ws.v1') {
                    console.warn('[pty-ws] server did not echo subprotocol pty-ws.v1; got:', this.ws.protocol);
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

document.addEventListener('DOMContentLoaded', () => {
    _initSunsetBanner();
    // #250: must run after _initSunsetBanner so the banner exists for
    // count-updates. Safe to call even if the API endpoint 503s — the
    // tracker falls back to client-side counts from Sunset listeners.
    // DeprecationTracker disabled — pre-launch, no v1 consumers exist yet.
    // Re-enable by uncommenting when external clients start hitting v1.
    // try { DeprecationTracker.init(); } catch (e) { console.error('DeprecationTracker init failed', e); }
    window.dashboard = new AgenticDashboard();
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
