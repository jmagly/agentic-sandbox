export function resolveWebSocketUrl(pageUrl, advertisedEndpoint = null) {
    const page = new URL(pageUrl);
    const advertised = String(advertisedEndpoint || '/').replaceAll('{host}', page.hostname);
    const endpoint = new URL(advertised, page);
    if (endpoint.protocol === 'http:') endpoint.protocol = 'ws:';
    else if (endpoint.protocol === 'https:') endpoint.protocol = 'wss:';
    if (!['ws:', 'wss:'].includes(endpoint.protocol)) {
        throw new TypeError(`Unsupported WebSocket endpoint scheme: ${endpoint.protocol}`);
    }
    if (page.protocol === 'https:' && endpoint.protocol !== 'wss:') {
        throw new TypeError('HTTPS dashboards require a wss: management endpoint');
    }
    return endpoint.toString();
}
