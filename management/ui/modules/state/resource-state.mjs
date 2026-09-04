export class ResourceState {
    constructor() {
        this.selected = new Map();
        this.resources = new Map();
        this.operations = new Map();
    }

    select(domain, id) {
        this.selected.set(domain, id == null ? null : String(id));
    }

    upsert(domain, value) {
        const id = value?.id ?? value?.operation_id;
        if (id == null) throw new TypeError(`${domain} resource is missing an id`);
        if (!this.resources.has(domain)) this.resources.set(domain, new Map());
        this.resources.get(domain).set(String(id), Object.freeze({ ...value }));
        return value;
    }

    trackOperation(operation) {
        const id = operation?.id ?? operation?.operation_id;
        if (id == null) throw new TypeError('operation is missing an id');
        this.operations.set(String(id), Object.freeze({ ...operation }));
        return operation;
    }
}
