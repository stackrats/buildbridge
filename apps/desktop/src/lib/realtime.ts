// Turning a realtime failure into something a person can act on.
//
// pusher-js reports connection failures as nested plain objects, not Errors, so anything that
// stringifies them naively prints "[object Object]" and the actual reason — usually a four-digit
// close code carrying the whole diagnosis — never reaches the screen.

export interface RealtimeEndpoint {
    scheme: string;
    host: string;
    port: number;
}

export interface RealtimeErrorDetail {
    code: number | null;
    message: string | null;
}

function readString(value: unknown): string | null {
    return typeof value === 'string' && value.trim() !== '' ? value.trim() : null;
}

/**
 * Digs the close code and message out of whatever shape arrived. Pusher nests the useful part
 * under `error.data`, and Reverb sometimes delivers `data` as a JSON string rather than an object.
 */
export function realtimeErrorDetail(error: unknown): RealtimeErrorDetail {
    let node: unknown = error;

    for (let depth = 0; depth < 5 && node !== null && typeof node === 'object'; depth += 1) {
        const record = node as Record<string, unknown>;
        const code = typeof record.code === 'number' ? record.code : null;
        const message = readString(record.message);
        if (code !== null || message !== null) {
            return { code, message };
        }

        let next = record.data ?? record.error;
        if (typeof next === 'string') {
            try {
                next = JSON.parse(next);
            } catch {
                return { code: null, message: readString(next) };
            }
        }
        node = next ?? null;
    }

    return { code: null, message: readString(error) };
}

/**
 * A sentence naming the endpoint, the reason, and what to do about it. 4001 is worth singling
 * out: it means a realtime server answered but does not know this application key, which on a
 * development machine almost always means another project's server holds the port.
 */
export function describeRealtimeFailure(error: unknown, endpoint: RealtimeEndpoint): string {
    const { code, message } = realtimeErrorDetail(error);
    const address = `${endpoint.host}:${endpoint.port}`;

    if (code === 4001 || code === 4000) {
        return `The realtime server at ${address} does not recognise this application key. Another server may be using that port; check that the control plane's Reverb is the one listening there.`;
    }
    if (code === 4004) {
        return `The realtime server at ${address} is over its connection quota.`;
    }
    if (code !== null) {
        return `Realtime connection to ${address} failed with code ${code}${message ? `: ${message}` : '.'}`;
    }
    if (message !== null) {
        return `Realtime connection to ${address} failed: ${message}`;
    }

    return `Could not reach the realtime server at ${address}. Start the control plane's Reverb server, then check for work.`;
}
