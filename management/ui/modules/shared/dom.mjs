/** Render untrusted operator/API text without creating an HTML sink. */
export function text(element, value) {
    element.textContent = value == null ? '' : String(value);
    return element;
}

export function link(element, href, label) {
    const url = new URL(String(href), window.location.href);
    if (!['http:', 'https:'].includes(url.protocol)) {
        throw new TypeError(`unsupported link protocol: ${url.protocol}`);
    }
    element.href = url.href;
    element.rel = 'noopener noreferrer';
    text(element, label);
    return element;
}
