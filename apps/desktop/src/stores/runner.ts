// Control-plane pairing, the private Reverb channel, heartbeats, and the claim loop.
//
// Work is claimed when the socket connects, when the private channel subscription succeeds, or
// when a `build.queued` event arrives; a single drain loop claims until the control plane reports
// nothing queued. The heartbeat is health signalling, not polling — but its reply carries the
// number of builds waiting, so a queue event lost in transit is recovered within one heartbeat
// instead of stranding the build.

import Pusher from 'pusher-js';
import { reactive } from 'vue';

import { useBackend, type Unlisten } from '../lib/backend';
import { describeRealtimeFailure } from '../lib/realtime';
import { describeError } from '../lib/utils';
import type { DesktopStatus, PairInput } from '../types/backend';

import { normalizeRealtimeState, type RealtimeState } from '../model/runner';

export type { RealtimeState };

export interface ActivityEntry {
    id: number;
    at: number;
    tone: 'info' | 'success' | 'warning' | 'danger';
    message: string;
    buildId: string | null;
}

const HEARTBEAT_INTERVAL_MS = 20_000;
const ACTIVITY_LIMIT = 40;

const state = reactive({
    status: null as DesktopStatus | null,
    loading: true,
    pairing: false,
    unpairing: false,
    checking: false,
    error: null as string | null,
    realtime: 'disconnected' as RealtimeState,
    activity: [] as ActivityEntry[],
});

let client: Pusher | null = null;
let heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
let workRequested = false;
let activityId = 0;
let activityUnlisten: Unlisten | null = null;

function log(tone: ActivityEntry['tone'], message: string, buildId: string | null = null): void {
    activityId += 1;
    state.activity.unshift({ id: activityId, at: Date.now(), tone, message, buildId });
    if (state.activity.length > ACTIVITY_LIMIT) {
        state.activity.length = ACTIVITY_LIMIT;
    }
}

async function refreshStatus(): Promise<void> {
    state.loading = state.status === null;
    try {
        state.status = await useBackend().getRunnerStatus();
        state.error = null;
    } catch (error) {
        state.error = describeError(error);
    } finally {
        state.loading = false;
    }
}

function isPaired(): boolean {
    return state.status?.paired === true;
}

async function sendHeartbeat(): Promise<void> {
    if (!isPaired()) {
        return;
    }
    try {
        const { queuedBuilds } = await useBackend().heartbeatRunner();
        if (queuedBuilds > 0 && !state.checking) {
            log(
                'info',
                `${queuedBuilds} queued ${queuedBuilds === 1 ? 'build' : 'builds'} found by heartbeat.`,
            );
            void checkForWork();
        }
    } catch (error) {
        state.error = describeError(error);
    } finally {
        if (heartbeatTimer) {
            clearTimeout(heartbeatTimer);
        }
        heartbeatTimer = setTimeout(() => void sendHeartbeat(), HEARTBEAT_INTERVAL_MS);
    }
}

async function checkForWork(): Promise<void> {
    if (!isPaired()) {
        return;
    }
    if (state.checking) {
        workRequested = true;
        return;
    }
    state.checking = true;
    try {
        do {
            const result = await useBackend().runOnce();
            if (result.state === 'idle') {
                break;
            }
            if (result.state === 'completed') {
                log('success', result.message, result.buildId);
            }
            if (result.state === 'failed') {
                log('danger', result.message, result.buildId);
            }
            if (result.state === 'busy') {
                break;
            }
        } while (isPaired());
    } catch (error) {
        state.error = describeError(error);
        log('danger', describeError(error));
    } finally {
        state.checking = false;
        if (workRequested) {
            workRequested = false;
            void checkForWork();
        }
    }
}

async function connectRealtime(): Promise<void> {
    disconnectRealtime();
    if (!isPaired()) {
        return;
    }
    const backend = useBackend();
    let configuration;
    try {
        configuration = await backend.getRealtimeConfiguration();
    } catch (error) {
        state.realtime = 'unavailable';
        state.error = describeError(error);
        return;
    }

    state.realtime = 'connecting';
    const pusher = new Pusher(configuration.key, {
        cluster: 'mt1',
        wsHost: configuration.host,
        wsPort: configuration.port,
        wssPort: configuration.port,
        forceTLS: configuration.scheme === 'https',
        enabledTransports: ['ws'],
        disableStats: true,
        channelAuthorization: {
            transport: 'ajax',
            endpoint: '',
            customHandler: (params, callback) => {
                backend
                    .authorizeRealtime({
                        socketId: params.socketId,
                        channelName: params.channelName,
                    })
                    .then((authorization) => callback(null, authorization))
                    .catch((error: unknown) =>
                        callback(
                            error instanceof Error ? error : new Error(describeError(error)),
                            null,
                        ),
                    );
            },
        },
    });

    pusher.connection.bind('state_change', (change: { current: string }) => {
        state.realtime = normalizeRealtimeState(change.current);
    });
    pusher.connection.bind('connected', () => {
        state.error = null;
        void sendHeartbeat();
        void checkForWork();
    });
    pusher.connection.bind('error', (error: unknown) => {
        const reason = describeRealtimeFailure(error, configuration);
        if (state.error !== reason) {
            log('warning', reason);
        }
        state.error = reason;
    });
    const channel = pusher.subscribe(configuration.channel);
    channel.bind('pusher:subscription_succeeded', () => {
        log('info', 'Listening for queued builds on the private runner channel.');
        void checkForWork();
    });
    channel.bind('build.queued', () => {
        log('info', 'A build was queued.');
        void checkForWork();
    });
    client = pusher;
}

function disconnectRealtime(): void {
    if (heartbeatTimer) {
        clearTimeout(heartbeatTimer);
        heartbeatTimer = null;
    }
    client?.disconnect();
    client = null;
    state.realtime = 'disconnected';
}

export function useRunnerStore() {
    return {
        state,
        async initialize(): Promise<void> {
            activityUnlisten ??= await useBackend().onRunnerActivity((result) => {
                if (result.state === 'completed' || result.state === 'failed') {
                    log(
                        result.state === 'completed' ? 'success' : 'danger',
                        result.message,
                        result.buildId,
                    );
                }
            });
            await refreshStatus();
            if (isPaired()) {
                await connectRealtime();
            }
        },
        refresh: refreshStatus,
        checkForWork,
        async pair(input: PairInput): Promise<void> {
            state.pairing = true;
            state.error = null;
            try {
                state.status = await useBackend().pairRunner(input);
                log('success', `Paired as ${state.status.runnerName ?? input.runnerName}.`);
                await connectRealtime();
            } catch (error) {
                state.error = describeError(error);
                throw error;
            } finally {
                state.pairing = false;
            }
        },
        async unpair(): Promise<void> {
            state.unpairing = true;
            disconnectRealtime();
            try {
                await useBackend().unpairRunner();
                log('warning', 'Unpaired locally. Revoke the runner in the control plane as well.');
            } catch (error) {
                state.error = describeError(error);
            } finally {
                await refreshStatus();
                state.unpairing = false;
            }
        },
        dispose(): void {
            disconnectRealtime();
            activityUnlisten?.();
            activityUnlisten = null;
        },
        clearError(): void {
            state.error = null;
        },
    };
}
