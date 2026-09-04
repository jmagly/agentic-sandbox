function appendText(document, parent, tag, className, value) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    element.textContent = value == null ? '' : String(value);
    parent.append(element);
    return element;
}

export function isTerminalOperation(operation) {
    return ['succeeded', 'failed', 'canceled', 'cancelled'].includes(operation?.state);
}

function reconciliationSummary(evidence) {
    const fields = [];
    if ('instance_present' in evidence) {
        fields.push(`instance ${evidence.instance_present ? 'present' : 'absent'}`);
    }
    if (evidence.observed_state) fields.push(`observed ${evidence.observed_state}`);
    if (evidence.effect_status) fields.push(`effect ${evidence.effect_status}`);
    if (evidence.generation != null) fields.push(`generation ${evidence.generation}`);
    if (evidence.source) fields.push(`source ${evidence.source}`);
    return `reconciled ${evidence.checked_at || 'at an unknown time'}${fields.length ? `: ${fields.join(', ')}` : ''}`;
}

export function renderOperationList(list, operations, {
    selectedOperation = null,
    onSelect = () => {},
    onEvidence = () => {},
    onReconcile = () => {},
    onRetry = () => {},
} = {}) {
    list.replaceChildren();
    const document = list.ownerDocument;
    const sorted = [...operations].sort((a, b) =>
        String(b.created_at || '').localeCompare(String(a.created_at || '')));
    if (!sorted.length) {
        appendText(document, list, 'p', 'operation-empty', 'No tracked operations.');
        return;
    }

    for (const operation of sorted) {
        const row = document.createElement('article');
        row.className = `operation-row${operation.id === selectedOperation ? ' selected' : ''}`;
        row.dataset.state = operation.state || 'unknown';

        const header = document.createElement('div');
        header.className = 'operation-row-header';
        appendText(document, header, 'strong', '', operation.kind || 'operation');
        appendText(document, header, 'span', '', operation.state || 'unknown');
        row.append(header);
        appendText(
            document,
            row,
            'div',
            'operation-meta',
            `${operation.target || 'unknown target'} · ${operation.id}${operation.idempotency_replayed ? ' · recovered idempotent replay' : ''}`,
        );

        const percent = Number(operation.progress?.percent);
        if (Number.isFinite(percent)) {
            const progress = document.createElement('progress');
            progress.max = 100;
            progress.value = Math.max(0, Math.min(100, percent));
            row.append(progress);
        }
        const detail = operation.error?.detail || operation.progress?.phase || operation.reconciliation_error;
        if (detail) appendText(document, row, 'div', 'operation-meta', detail);
        appendText(
            document,
            row,
            'div',
            'operation-meta',
            `created ${operation.created_at || 'unknown'} · completed ${operation.completed_at || 'not terminal'}`,
        );
        if (operation.trace_id || operation.request_id) {
            appendText(
                document,
                row,
                'div',
                'operation-meta',
                `trace ${operation.trace_id || 'not reported'} · request ${operation.request_id || 'not reported'}`,
            );
        }
        if (operation.reconciliation_evidence) {
            appendText(
                document,
                row,
                'div',
                'operation-meta',
                reconciliationSummary(operation.reconciliation_evidence),
            );
        }
        if (isTerminalOperation(operation) && operation.result != null) {
            appendText(document, row, 'pre', 'operation-terminal-evidence', JSON.stringify(operation.result, null, 2));
        }

        const controls = document.createElement('div');
        controls.className = 'operation-controls';
        const button = (label, callback) => {
            const control = document.createElement('button');
            control.type = 'button';
            control.textContent = label;
            control.addEventListener('click', (event) => {
                event.stopPropagation();
                callback(operation);
            });
            controls.append(control);
            return control;
        };
        button('Activity', onEvidence);
        button('Reconcile', onReconcile);
        const retry = button('Retry', onRetry);
        retry.disabled = !String(operation.id).startsWith('unknown:')
            || operation.state !== 'unknown'
            || !operation.intent_key || !operation.retry_request;
        retry.title = retry.disabled
            ? 'No safe replay intent is available'
            : 'Replay the exact request with its original idempotency key';
        const cancel = button('Cancel', () => {});
        cancel.disabled = true;
        cancel.title = 'No operation cancel capability is advertised by this server';
        row.append(controls);
        row.addEventListener('click', () => onSelect(operation));
        list.append(row);
    }
}
