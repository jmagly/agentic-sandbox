export class UnsupportedContractVersionError extends Error {
    constructor({ contract, received, supportedMajor }) {
        super(`${contract} major version ${received} is unsupported; expected ${supportedMajor}.x`);
        this.name = 'UnsupportedContractVersionError';
        this.code = 'unsupported_contract_version';
        this.contract = contract;
        this.received = received;
        this.supportedMajor = supportedMajor;
    }
}

function majorOf(value) {
    if (Number.isInteger(value)) return value;
    const match = String(value ?? '').match(/^(?:v)?(\d+)(?:\.|$)/i);
    return match ? Number(match[1]) : null;
}

/**
 * Reject unknown required major versions while leaving additive fields alone.
 * Callers name the discriminator because current contracts use a mixture of
 * `version`, `schema_version`, and `protocol_version` during migration.
 */
export function requireMajorVersion(document, {
    contract,
    field = 'version',
    supportedMajor = 1,
    required = true,
} = {}) {
    const value = document?.[field];
    if ((value === undefined || value === null || value === '') && !required) return document;
    const major = majorOf(value);
    if (major !== supportedMajor) {
        throw new UnsupportedContractVersionError({
            contract: contract || field,
            received: value ?? 'missing',
            supportedMajor,
        });
    }
    return document;
}
