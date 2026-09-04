function text(value, fallback = '') {
    return value == null ? fallback : String(value);
}

function stringList(value) {
    return Array.isArray(value) ? value.map(String) : [];
}

export function effectiveLeaseState(state, expiresAt, now = Date.now()) {
    return state === 'active' && expiresAt && Date.parse(expiresAt) <= now ? 'expired' : text(state, 'unknown');
}

export function projectCredentialMetadata(value = {}) {
    return {
        id: text(value.id), provider: text(value.provider), type: text(value.type),
        owner: value.owner == null ? null : text(value.owner), scopes: stringList(value.scopes),
        allowed_uses: stringList(value.allowed_uses),
        backend_kind: value.backend?.kind == null ? null : text(value.backend.kind),
        configured: Boolean(value.configured), created_at: value.created_at,
        updated_at: value.updated_at, last_rotated_at: value.last_rotated_at,
    };
}

export function projectCredentialLeaseMetadata(value = {}, now = Date.now()) {
    return {
        id: text(value.id), credential_id: text(value.credential_id), agent_id: text(value.agent_id),
        instance_id: text(value.instance_id), session_id: text(value.session_id), provider: text(value.provider),
        allowed_use: text(value.allowed_use), issued_at: value.issued_at, expires_at: value.expires_at,
        state: effectiveLeaseState(value.state, value.expires_at, now), revoked_at: value.revoked_at,
        proxy_posture: value.proxy_policy ? 'configured' : 'not configured',
    };
}

export function projectSshLeaseMetadata(value = {}, now = Date.now()) {
    return {
        id: text(value.id), actor: text(value.actor), instance_id: text(value.instance_id),
        principal: text(value.principal), access_mode: text(value.access_mode),
        public_key_sha256: text(value.public_key_sha256), certificate_key_id: value.certificate_key_id || null,
        certificate_sha256: value.certificate_sha256 || null, issued_at: value.issued_at,
        expires_at: value.expires_at, ttl_seconds: Number(value.ttl_seconds),
        state: effectiveLeaseState(value.state, value.expires_at, now), revoked_at: value.revoked_at,
        revocation_effect: text(value.revocation_effect, 'unknown'),
    };
}

export function projectAccessAuditMetadata(value = {}) {
    return {
        id: text(value.id), sequence: Number(value.sequence), timestamp: value.timestamp,
        event_type: text(value.event_type), actor: text(value.actor), resource: text(value.resource),
        correlation_id: value.correlation_id || null, action: text(value.action),
        outcome: text(value.outcome), trace_id: value.trace_id || null,
    };
}

export function encodedAccessPathSegment(value) {
    return encodeURIComponent(text(value));
}
