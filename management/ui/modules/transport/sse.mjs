/** Bounded, duplicate-suppressing state for resumable SSE projections. */
export class ResumableEventBuffer {
    constructor({ limit = 1000 } = {}) {
        this.limit = limit;
        this.items = [];
        this.seen = new Set();
        this.lastEventId = null;
        this.incomplete = false;
    }

    append(event) {
        const id = String(event?.id ?? event?.event_id ?? '');
        if (id && this.seen.has(id)) return false;
        if (id) {
            this.seen.add(id);
            this.lastEventId = id;
        }
        this.items.push(event);
        while (this.items.length > this.limit) {
            const removed = this.items.shift();
            const removedId = String(removed?.id ?? removed?.event_id ?? '');
            if (removedId) this.seen.delete(removedId);
        }
        return true;
    }

    markGap(detail) {
        this.incomplete = true;
        this.gap = detail;
    }
}
