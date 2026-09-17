export type LiveReloadPlatform = 'android' | 'ios';

/** Early feedback; the engine validates the URL again before building or connecting. */
export function liveReloadUrlIssue(value: string, platform: LiveReloadPlatform): string | null {
    if (/\\|\p{Cc}/u.test(value))
        return 'Use a dev server URL without backslashes or control characters.';
    let url: URL;
    try {
        url = new URL(value.trim());
    } catch {
        return platform === 'ios'
            ? 'Enter the full dev server URL, such as http://192.168.1.10:5173.'
            : 'Enter the full dev server URL, such as http://localhost:5173.';
    }
    if (!['http:', 'https:'].includes(url.protocol) || !url.hostname)
        return 'Use an HTTP or HTTPS dev server URL.';
    if (url.username || url.password || value.includes('?') || value.includes('#'))
        return 'Use a dev server URL without credentials, a query, or a fragment.';
    const host = url.hostname.toLowerCase().replace(/\.$/, '');
    if (host === '0.0.0.0' || host === '[::]')
        return 'Use an address the device can reach instead of a wildcard address.';
    if (
        platform === 'ios' &&
        (host === 'localhost' ||
            host.endsWith('.localhost') ||
            /^127\.\d+\.\d+\.\d+$/.test(host) ||
            host === '[::1]' ||
            /^\[::ffff:7f[0-9a-f]{2}:[0-9a-f]+\]$/.test(host))
    )
        return 'Use this computer’s LAN address or an HTTPS dev server the iPhone can reach. Localhost points to the iPhone itself.';
    if (platform === 'android' && host === '[::1]')
        return 'Use localhost or 127.0.0.1 for the ADB connection to this computer.';
    if (url.port === '0') return 'Use the port your dev server is listening on, greater than zero.';
    return null;
}

export function appleRunStopDescription(liveReloadUrl: string | null): string {
    return liveReloadUrl
        ? 'Ends the console session. The app stays installed on the iPhone; live reload continues while the dev server is running and reachable.'
        : 'Ends the console session. The app stays installed and running on the iPhone.';
}
