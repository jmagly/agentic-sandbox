import { requireMajorVersion } from '../shared/version.mjs';

const RUNTIME_SELECTIONS = Object.freeze({ vm: 'qemu', container: 'docker', host: 'host' });

export class RuntimeUnavailableError extends Error {
    constructor(runtime, reason) {
        super(`Cannot create ${runtime}: ${reason}`);
        this.name = 'RuntimeUnavailableError';
        this.code = 'runtime_unavailable';
        this.runtime = runtime;
        this.reason = reason;
    }
}

export function requireAvailableRuntime(runtimeAvailability, selection) {
    const runtime = RUNTIME_SELECTIONS[selection];
    const descriptor = runtimeAvailability?.get?.(runtime);
    if (!runtime || !descriptor?.available) {
        throw new RuntimeUnavailableError(
            runtime || String(selection || 'unknown'),
            descriptor?.unavailable_reason || descriptor?.unavailable_code
                || 'runtime capability discovery has not confirmed availability',
        );
    }
    return descriptor;
}

export function parseInstanceCollection(body) {
    requireMajorVersion(body, {
        contract: 'admin instance collection',
        field: 'schema_version',
        supportedMajor: 1,
        required: false,
    });
    const instances = Array.isArray(body) ? body : (body?.items ?? body?.instances);
    if (!Array.isArray(instances)) throw new TypeError('instance collection is missing items[]');
    return instances;
}

export function operationFromAccepted(outcome, { target, kind } = {}) {
    if (outcome?.kind !== 'accepted') return null;
    const operation = outcome.body?.operation || outcome.body;
    const id = outcome.operationId || operation?.id || operation?.operation_id;
    if (!id) throw new TypeError('202 response is missing an operation identifier');
    return {
        ...operation,
        id,
        target: operation?.target || target || null,
        kind: operation?.kind || kind || 'unknown',
        trace_id: operation?.trace_id || outcome.traceId || null,
        request_id: operation?.request_id || outcome.requestId || null,
        idempotency_replayed: outcome.idempotencyReplayed === true,
    };
}

export function reconcileUnknownInstanceOperation(operation, instances, checkedAt = new Date().toISOString()) {
    const inventory = instances instanceof Map ? [...instances.values()] : [...(instances || [])];
    const instance = inventory.find((candidate) =>
        candidate?.id === operation?.target_id || candidate?.name === operation?.target);
    const observedState = instance?.observed_state || instance?.state || null;
    const before = operation?.reconciliation_before;
    const reconciled = operation?.kind === 'instance.provision'
        ? before?.authoritative === true && before.instance_present === false && Boolean(instance)
        : operation?.kind === 'instance.start'
            ? observedState === 'running'
            : operation?.kind === 'instance.stop'
                ? ['stopped', 'shut off'].includes(observedState)
                : operation?.kind === 'instance.destroy'
                    ? before?.authoritative === true && before.instance_present === true && !instance
                    : false;
    return {
        reconciled,
        evidence: {
            checked_at: checkedAt,
            source: '/api/v2/admin/instances',
            instance_present: Boolean(instance),
            observed_state: observedState,
            terminal_match: reconciled,
            baseline_authoritative: before?.authoritative === true,
            baseline_instance_present: before?.instance_present ?? null,
            baseline_instance_id: before?.instance_id ?? null,
        },
        result: reconciled ? {
            reconciled_from: 'canonical instance inventory',
            observed_state: observedState,
            instance_present: Boolean(instance),
        } : null,
    };
}
