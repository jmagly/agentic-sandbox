const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS']);

export class HttpOutcomeError extends Error {
    constructor(outcome, options = {}) {
        super(outcome.message || `HTTP ${outcome.status || 'request failure'}`, options);
        this.name = 'HttpOutcomeError';
        this.code = outcome.code;
        this.outcome = outcome;
    }
}

export class StaleResponseError extends Error {
    constructor(owner) {
        super(`Response no longer belongs to the active ${owner} selection`);
        this.name = 'StaleResponseError';
        this.code = 'stale_response';
        this.owner = owner;
    }
}

export class UnknownMutationOutcomeError extends Error {
    constructor({ method, url, idempotencyKey = null, cause }) {
        super(`${method} ${url} may have reached the server; reconcile before retrying`, { cause });
        this.name = 'UnknownMutationOutcomeError';
        this.code = 'mutation_outcome_unknown';
        this.method = method;
        this.url = String(url);
        this.idempotencyKey = idempotencyKey;
        this.outcomeUnknown = true;
        this.shouldReconcile = true;
        this.replayAllowed = false;
    }
}

function header(response, name) {
    return response?.headers?.get?.(name) || null;
}

function retryAfterMs(value, now = Date.now()) {
    if (!value) return null;
    const seconds = Number(value);
    if (Number.isFinite(seconds)) return Math.max(0, seconds * 1000);
    const at = Date.parse(value);
    return Number.isFinite(at) ? Math.max(0, at - now) : null;
}

async function readBody(response) {
    if (response.status === 204 || response.status === 205) return { body: null, bodyKind: 'empty' };
    const contentType = header(response, 'content-type') || '';
    const raw = await response.text();
    if (!raw) return { body: null, bodyKind: 'empty' };
    if (/\bjson\b/i.test(contentType)) {
        try { return { body: JSON.parse(raw), bodyKind: 'json' }; } catch (_) {}
    }
    return { body: raw, bodyKind: 'text' };
}

function identifiers(response, body) {
    return {
        requestId: header(response, 'x-request-id') || body?.request_id || body?.requestId || null,
        traceId: header(response, 'trace-id') || body?.trace_id || body?.traceId || null,
        operationId: header(response, 'operation-id') || body?.operation_id || body?.operationId
            || body?.operation?.id || null,
        idempotencyReplayed: header(response, 'idempotency-replayed') === 'true',
    };
}

export async function normalizeResponse(response) {
    const { body, bodyKind } = await readBody(response);
    const ids = identifiers(response, bodyKind === 'json' ? body : null);
    if (response.ok) {
        return {
            ok: true,
            kind: response.status === 202 ? 'accepted' : 'success',
            status: response.status,
            body,
            bodyKind,
            ...ids,
        };
    }

    const problem = bodyKind === 'json' && body && typeof body === 'object' ? body : {};
    const detail = problem.error && typeof problem.error === 'object' ? problem.error : problem;
    let kind = 'http_error';
    if ([401, 403].includes(response.status)) kind = 'forbidden';
    else if ([409, 412].includes(response.status)) kind = 'conflict';
    else if (response.status === 429) kind = 'rate_limited';
    else if ([502, 503, 504].includes(response.status)) kind = 'unavailable';
    else if (bodyKind === 'text') kind = 'non_json_error';

    return {
        ok: false,
        kind,
        status: response.status,
        code: detail.code || detail.type || `http_${response.status}`,
        message: detail.detail || detail.message || detail.title
            || (typeof problem.error === 'string' ? problem.error : null)
            || (bodyKind === 'text' ? body : response.statusText),
        retryAfterMs: retryAfterMs(header(response, 'retry-after')),
        problem,
        bodyKind,
        ...ids,
    };
}

function abortOutcome(error) {
    return {
        ok: false,
        kind: 'aborted',
        status: null,
        code: 'request_aborted',
        message: error?.message || 'Request aborted',
        requestId: null,
        traceId: null,
        operationId: null,
    };
}

/** Owns one in-flight request per logical view/resource selection. */
export class RequestOwnership {
    constructor() {
        this.entries = new Map();
        this.sequence = 0;
    }

    begin(owner, { signal, timeoutMs = 15000 } = {}) {
        this.cancel(owner, 'superseded');
        const controller = new AbortController();
        const token = ++this.sequence;
        const relay = () => controller.abort(signal?.reason || new DOMException('Aborted', 'AbortError'));
        if (signal?.aborted) relay();
        else signal?.addEventListener?.('abort', relay, { once: true });
        const timer = timeoutMs > 0
            ? setTimeout(() => controller.abort(new DOMException('Request timed out', 'TimeoutError')), timeoutMs)
            : null;
        this.entries.set(owner, { controller, token, timer, signal, relay });
        return { token, signal: controller.signal };
    }

    assertCurrent(owner, token) {
        if (this.entries.get(owner)?.token !== token) throw new StaleResponseError(owner);
    }

    finish(owner, token) {
        const entry = this.entries.get(owner);
        if (!entry || entry.token !== token) return false;
        if (entry.timer) clearTimeout(entry.timer);
        entry.signal?.removeEventListener?.('abort', entry.relay);
        this.entries.delete(owner);
        return true;
    }

    cancel(owner, reason = 'cancelled') {
        const entry = this.entries.get(owner);
        if (!entry) return false;
        entry.controller.abort(new DOMException(reason, 'AbortError'));
        this.finish(owner, entry.token);
        return true;
    }
}

export class HttpTransport {
    constructor({ fetchImpl = globalThis.fetch, ownership = new RequestOwnership() } = {}) {
        this.fetchImpl = fetchImpl;
        this.ownership = ownership;
    }

    async request(url, options = {}) {
        const owner = options.owner || String(url);
        const method = String(options.method || 'GET').toUpperCase();
        const { token, signal } = this.ownership.begin(owner, options);
        const request = { ...options, method, signal };
        delete request.owner;
        delete request.timeoutMs;
        delete request.expectJson;
        let dispatched = false;
        try {
            // A pre-aborted intent has not reached the network. Once fetch is
            // invoked, cancellation cannot prove the server rolled it back.
            if (signal.aborted) throw signal.reason;
            dispatched = true;
            const response = await this.fetchImpl(url, request);
            const outcome = await normalizeResponse(response);
            this.ownership.assertCurrent(owner, token);
            if (!outcome.ok) throw new HttpOutcomeError(outcome);
            if (options.expectJson && outcome.bodyKind !== 'json') {
                throw new TypeError('Expected a JSON response from the management API');
            }
            return outcome;
        } catch (error) {
            if (dispatched && !HttpTransport.isSafeMethod(method)
                && !(error instanceof HttpOutcomeError)) {
                throw new UnknownMutationOutcomeError({
                    method, url, cause: error,
                    idempotencyKey: new Headers(request.headers).get('Idempotency-Key'),
                });
            }
            if (signal.aborted && !(error instanceof StaleResponseError)) {
                throw new HttpOutcomeError(abortOutcome(error), { cause: error });
            }
            throw error;
        } finally {
            this.ownership.finish(owner, token);
        }
    }

    static isSafeMethod(method) {
        return SAFE_METHODS.has(String(method || 'GET').toUpperCase());
    }
}
