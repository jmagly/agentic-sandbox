import { text } from '../shared/dom.mjs';

export function renderOutcome(element, outcome) {
    const detail = outcome?.message || outcome?.body?.detail || outcome?.kind || 'Unknown state';
    element.dataset.state = outcome?.kind || 'unknown';
    text(element, detail);
    return element;
}

export function renderWorkspaceStatus(element, state, message) {
    element.className = `workspace-status ${state}`;
    text(element, message);
    return element;
}
