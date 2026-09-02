<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import Pusher from 'pusher-js';
import {
    ArrowRight,
    CheckCircle2,
    CircleDot,
    Cloud,
    Cpu,
    HardDrive,
    KeyRound,
    LoaderCircle,
    Play,
    Server,
    ShieldCheck,
    Unplug,
    Wifi,
    XCircle,
} from '@lucide/vue';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import MacBuilderPanel from './components/MacBuilderPanel.vue';

type RunnerStatus = {
    paired: boolean;
    serverUrl: string | null;
    runnerId: string | null;
    runnerName: string | null;
    platform: string;
    architecture: string;
    version: string;
};

type RunOnceResult = {
    state: 'idle' | 'completed' | 'busy';
    buildId: string | null;
    message: string;
};

type ActivityEntry = RunOnceResult & {
    at: Date;
};

type RealtimeConfiguration = {
    key: string;
    host: string;
    port: number;
    scheme: 'http' | 'https';
    channel: string;
};

type RealtimeAuthorization = {
    auth: string;
    channel_data?: string;
    shared_secret?: string;
};

type RealtimeState = 'disconnected' | 'connecting' | 'connected' | 'unavailable';

const status = ref<RunnerStatus | null>(null);
const loading = ref(true);
const pairing = ref(false);
const checking = ref(false);
const error = ref<string | null>(null);
const serverUrl = ref('http://127.0.0.1:8001');
const pairingCode = ref('');
const runnerName = ref('');
const activity = ref<ActivityEntry[]>([]);
const activeView = ref<'runner' | 'mac-builder'>('runner');
const realtimeState = ref<RealtimeState>('disconnected');
let realtimeClient: Pusher | null = null;
let heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
let workRequested = false;

const isPaired = computed(() => status.value?.paired === true);
const machineLabel = computed(() =>
    status.value === null
        ? 'Detecting machine…'
        : `${status.value.platform} / ${status.value.architecture}`,
);

onMounted(async () => {
    await refreshStatus();
    if (isPaired.value) await connectRealtime();
});

onUnmounted(() => {
    disconnectRealtime();
});

async function refreshStatus(): Promise<void> {
    loading.value = true;

    try {
        status.value = await invoke<RunnerStatus>('get_runner_status');
        if (status.value.serverUrl) serverUrl.value = status.value.serverUrl;
        if (status.value.runnerName) runnerName.value = status.value.runnerName;
        if (!runnerName.value) runnerName.value = defaultRunnerName(status.value);
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        loading.value = false;
    }
}

function defaultRunnerName(value: RunnerStatus): string {
    const role = value.platform === 'macos' ? 'mac-builder' : 'docker-host';
    return `${role}-${value.architecture}`;
}

async function pair(): Promise<void> {
    pairing.value = true;
    error.value = null;

    try {
        status.value = await invoke<RunnerStatus>('pair_runner', {
            input: {
                serverUrl: serverUrl.value,
                code: pairingCode.value,
                runnerName: runnerName.value,
            },
        });
        pairingCode.value = '';
        pushActivity({ state: 'idle', buildId: null, message: 'Runner paired securely.' });
        await connectRealtime();
    } catch (caught) {
        error.value = String(caught);
    } finally {
        pairing.value = false;
    }
}

async function unpair(): Promise<void> {
    error.value = null;

    try {
        disconnectRealtime();
        await invoke('unpair_runner');
        await refreshStatus();
        pushActivity({
            state: 'idle',
            buildId: null,
            message: 'Local runner credentials removed.',
        });
    } catch (caught) {
        error.value = String(caught);
    }
}

async function connectRealtime(): Promise<void> {
    disconnectRealtime();
    if (!isPaired.value) return;

    realtimeState.value = 'connecting';

    try {
        const configuration = await invoke<RealtimeConfiguration>('get_realtime_configuration');
        const client = new Pusher(configuration.key, {
            cluster: 'mt1',
            wsHost: configuration.host,
            wsPort: configuration.port,
            wssPort: configuration.port,
            forceTLS: configuration.scheme === 'https',
            enabledTransports: ['ws'],
            disableStats: true,
            channelAuthorization: {
                customHandler: (params, callback) => {
                    void invoke<RealtimeAuthorization>('authorize_realtime', {
                        input: {
                            socketId: params.socketId,
                            channelName: params.channelName,
                        },
                    })
                        .then((authorization) => callback(null, authorization))
                        .catch((caught) => callback(new Error(String(caught)), null));
                },
            },
        });

        client.connection.bind('state_change', (state: { current: string }) => {
            realtimeState.value = normalizeRealtimeState(state.current);
        });
        client.connection.bind('connected', () => {
            error.value = null;
            void sendHeartbeat();
            void checkForWork();
        });
        client.connection.bind('error', (caught: unknown) => {
            error.value = `Realtime connection failed: ${String(caught)}`;
        });
        const channel = client.subscribe(configuration.channel);
        channel.bind('pusher:subscription_succeeded', () => {
            void checkForWork();
        });
        channel.bind('build.queued', () => {
            void checkForWork();
        });
        realtimeClient = client;
    } catch (caught) {
        realtimeState.value = 'unavailable';
        error.value = String(caught);
    }
}

function disconnectRealtime(): void {
    if (heartbeatTimer !== null) {
        clearTimeout(heartbeatTimer);
        heartbeatTimer = null;
    }

    realtimeClient?.disconnect();
    realtimeClient = null;
    realtimeState.value = 'disconnected';
}

function normalizeRealtimeState(state: string): RealtimeState {
    if (state === 'connected' || state === 'connecting' || state === 'unavailable') return state;

    return 'disconnected';
}

async function sendHeartbeat(): Promise<void> {
    if (!isPaired.value) return;

    if (heartbeatTimer !== null) {
        clearTimeout(heartbeatTimer);
        heartbeatTimer = null;
    }

    try {
        await invoke('heartbeat_runner');
    } catch (caught) {
        error.value = String(caught);
    } finally {
        if (isPaired.value) {
            heartbeatTimer = setTimeout(() => void sendHeartbeat(), 20_000);
        }
    }
}

async function checkForWork(): Promise<void> {
    if (!isPaired.value) return;
    if (checking.value) {
        workRequested = true;

        return;
    }

    checking.value = true;

    try {
        do {
            workRequested = false;
            const result = await invoke<RunOnceResult>('run_once');
            if (result.state !== 'busy') pushActivity(result);
            if (result.state === 'idle') break;
        } while (isPaired.value);

        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        checking.value = false;
        if (workRequested) void checkForWork();
    }
}

function pushActivity(entry: RunOnceResult): void {
    const previous = activity.value[0];
    if (entry.state === 'idle' && previous?.state === 'idle' && previous.message === entry.message)
        return;

    activity.value.unshift({ ...entry, at: new Date() });
    activity.value = activity.value.slice(0, 8);
}
</script>

<template>
    <div class="app-shell">
        <header class="topbar">
            <div class="brand">
                <span class="brand-mark"><ArrowRight :size="20" /></span>
                <span>
                    <strong>BuildBridge</strong>
                    <small>Desktop runner</small>
                </span>
            </div>

            <nav class="view-tabs" aria-label="BuildBridge sections">
                <button
                    type="button"
                    :class="{ active: activeView === 'runner' }"
                    @click="activeView = 'runner'"
                >
                    <Wifi :size="14" /> Runner
                </button>
                <button
                    type="button"
                    :class="{ active: activeView === 'mac-builder' }"
                    @click="activeView = 'mac-builder'"
                >
                    <HardDrive :size="14" /> macOS builder
                </button>
            </nav>

            <div class="machine-chip">
                <Cpu :size="14" />
                <span>{{ machineLabel }}</span>
                <i :class="isPaired ? 'online' : ''" />
            </div>
        </header>

        <main v-if="!loading && activeView === 'runner'" class="content">
            <section class="hero">
                <div>
                    <p class="eyebrow">Runner connection</p>
                    <h1>{{ isPaired ? 'Ready for builds' : 'Connect this machine' }}</h1>
                    <p class="hero-copy">
                        {{
                            isPaired
                                ? 'This runner waits on a private WebSocket channel, executes typed work locally, and sends results back to your control plane.'
                                : 'Pair once with the code shown in the web control plane. Your runner token stays in the operating system credential vault.'
                        }}
                    </p>
                </div>

                <div class="connection-orb" :class="{ connected: realtimeState === 'connected' }">
                    <Wifi v-if="realtimeState === 'connected'" :size="26" />
                    <Unplug v-else :size="26" />
                </div>
            </section>

            <div v-if="error" class="error-banner">
                <XCircle :size="17" />
                <span>{{ error }}</span>
            </div>

            <div class="workspace">
                <section v-if="!isPaired" class="panel pairing-panel">
                    <div class="panel-heading">
                        <span class="icon-box"><KeyRound :size="18" /></span>
                        <div>
                            <h2>Pair runner</h2>
                            <p>Use a single-use code from the BuildBridge web app.</p>
                        </div>
                    </div>

                    <form class="form-grid" @submit.prevent="pair">
                        <label>
                            <span>Control-plane URL</span>
                            <input
                                v-model.trim="serverUrl"
                                type="url"
                                required
                                placeholder="https://builds.example.com"
                            />
                        </label>

                        <label>
                            <span>Runner name</span>
                            <input
                                v-model.trim="runnerName"
                                required
                                maxlength="80"
                                placeholder="mac-builder-1"
                            />
                        </label>

                        <label class="code-field">
                            <span>Pairing code</span>
                            <input
                                v-model.trim="pairingCode"
                                required
                                maxlength="20"
                                autocomplete="one-time-code"
                                placeholder="ABCD-EFGH"
                            />
                        </label>

                        <button class="primary-button" type="submit" :disabled="pairing">
                            <LoaderCircle v-if="pairing" :size="16" class="spin" />
                            <ShieldCheck v-else :size="16" />
                            Pair securely
                        </button>
                    </form>
                </section>

                <section v-else class="panel runner-panel">
                    <div class="runner-summary">
                        <span class="runner-icon"><Server :size="23" /></span>
                        <div>
                            <div class="title-row">
                                <h2>{{ status?.runnerName }}</h2>
                                <span class="online-badge"><i /> {{ realtimeState }}</span>
                            </div>
                            <p>{{ status?.serverUrl }}</p>
                        </div>
                    </div>

                    <dl class="metadata">
                        <div>
                            <dt>Runner ID</dt>
                            <dd>{{ status?.runnerId?.slice(0, 13) }}…</dd>
                        </div>
                        <div>
                            <dt>Version</dt>
                            <dd>v{{ status?.version }}</dd>
                        </div>
                        <div>
                            <dt>Transport</dt>
                            <dd>WebSocket</dd>
                        </div>
                        <div>
                            <dt>Credential</dt>
                            <dd>OS vault</dd>
                        </div>
                    </dl>

                    <div class="runner-actions">
                        <button
                            class="primary-button"
                            type="button"
                            :disabled="checking"
                            @click="checkForWork"
                        >
                            <LoaderCircle v-if="checking" :size="16" class="spin" />
                            <Play v-else :size="15" />
                            {{ checking ? 'Checking…' : 'Check now' }}
                        </button>
                        <button class="quiet-button" type="button" @click="unpair">
                            <Unplug :size="14" /> Unpair locally
                        </button>
                    </div>
                </section>

                <section class="panel activity-panel">
                    <div class="panel-heading compact">
                        <span class="icon-box blue"><Cloud :size="18" /></span>
                        <div>
                            <h2>Runner activity</h2>
                            <p>Claims and results from this session.</p>
                        </div>
                        <span v-if="checking" class="checking"
                            ><LoaderCircle :size="12" class="spin" /> claiming</span
                        >
                    </div>

                    <div v-if="activity.length" class="activity-list">
                        <article v-for="entry in activity" :key="entry.at.getTime()">
                            <span class="activity-icon" :class="entry.state">
                                <CheckCircle2 v-if="entry.state === 'completed'" :size="15" />
                                <CircleDot v-else :size="15" />
                            </span>
                            <div>
                                <p>{{ entry.message }}</p>
                                <small>
                                    {{
                                        entry.at.toLocaleTimeString([], {
                                            hour: '2-digit',
                                            minute: '2-digit',
                                            second: '2-digit',
                                        })
                                    }}
                                    <template v-if="entry.buildId">
                                        · build {{ entry.buildId.slice(0, 8) }}</template
                                    >
                                </small>
                            </div>
                        </article>
                    </div>

                    <div v-else class="empty-activity">
                        <CircleDot :size="19" />
                        <p>Activity appears here once the runner connects.</p>
                    </div>
                </section>
            </div>
        </main>

        <MacBuilderPanel v-else-if="!loading" />

        <main v-else class="loading-state">
            <LoaderCircle :size="22" class="spin" />
            <span>Loading runner configuration…</span>
        </main>

        <footer>
            <span
                ><i :class="isPaired ? 'online' : ''" />
                {{ isPaired ? `Realtime ${realtimeState}` : 'Not paired' }}</span
            >
            <span>{{
                activeView === 'runner'
                    ? 'Typed jobs only · protocol v1'
                    : 'Managed Docker-OSX provider'
            }}</span>
        </footer>
    </div>
</template>

<style>
:root {
    font-family:
        Inter,
        'SF Pro Text',
        ui-sans-serif,
        system-ui,
        -apple-system,
        sans-serif;
    color: #e7eef7;
    background: #07111f;
    font-synthesis: none;
    text-rendering: optimizeLegibility;
    -webkit-font-smoothing: antialiased;
}

* {
    box-sizing: border-box;
}

body {
    margin: 0;
    min-width: 320px;
    min-height: 100vh;
    background:
        radial-gradient(circle at 10% 0%, rgb(20 184 166 / 0.11), transparent 26rem),
        radial-gradient(circle at 100% 5%, rgb(59 130 246 / 0.09), transparent 28rem), #07111f;
}

button,
input {
    font: inherit;
}

button {
    cursor: pointer;
}

button:disabled {
    cursor: not-allowed;
    opacity: 0.58;
}

.app-shell {
    min-height: 100vh;
    display: grid;
    grid-template-rows: auto 1fr auto;
}

.topbar,
footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 24px;
    border-color: rgb(255 255 255 / 0.07);
}

.topbar {
    border-bottom: 1px solid rgb(255 255 255 / 0.07);
}

.view-tabs {
    display: flex;
    gap: 3px;
    padding: 3px;
    border: 1px solid rgb(255 255 255 / 0.07);
    border-radius: 10px;
    background: rgb(2 8 20 / 0.42);
}

.view-tabs button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 10px;
    color: #64748b;
    font-size: 10px;
    font-weight: 700;
    border: 0;
    border-radius: 7px;
    background: transparent;
}

.view-tabs button.active {
    color: #ccfbf1;
    background: rgb(45 212 191 / 0.1);
    box-shadow: inset 0 0 0 1px rgb(94 234 212 / 0.1);
}

.brand,
.brand > span:last-child,
.machine-chip,
.panel-heading,
.runner-summary,
.title-row,
.runner-actions,
.checking,
footer span {
    display: flex;
    align-items: center;
}

.brand {
    gap: 11px;
}

.brand-mark,
.icon-box,
.runner-icon {
    display: grid;
    place-items: center;
}

.brand-mark {
    width: 36px;
    height: 36px;
    color: #99f6e4;
    border: 1px solid rgb(94 234 212 / 0.18);
    border-radius: 11px;
    background: rgb(45 212 191 / 0.09);
}

.brand > span:last-child {
    align-items: flex-start;
    flex-direction: column;
}

.brand strong {
    color: white;
    font-size: 15px;
}

.brand small {
    margin-top: 1px;
    color: #64748b;
    font-size: 11px;
}

.machine-chip {
    gap: 7px;
    padding: 7px 10px;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 11px;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 999px;
    background: rgb(255 255 255 / 0.03);
}

.machine-chip i,
footer i,
.online-badge i {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: #64748b;
}

.machine-chip i.online,
footer i.online,
.online-badge i {
    background: #34d399;
    box-shadow: 0 0 10px rgb(52 211 153 / 0.7);
}

.content {
    width: min(100%, 1100px);
    margin: 0 auto;
    padding: 38px 28px 34px;
}

.hero {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 32px;
    margin-bottom: 24px;
}

.eyebrow {
    margin: 0 0 8px;
    color: #5eead4;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.18em;
    text-transform: uppercase;
}

h1,
h2,
p {
    margin-top: 0;
}

h1 {
    margin-bottom: 10px;
    color: white;
    font-size: clamp(28px, 4vw, 42px);
    line-height: 1.08;
    letter-spacing: -0.035em;
}

.hero-copy {
    max-width: 650px;
    margin-bottom: 0;
    color: #94a3b8;
    font-size: 14px;
    line-height: 1.7;
}

.connection-orb {
    display: grid;
    width: 70px;
    height: 70px;
    flex: 0 0 auto;
    place-items: center;
    color: #64748b;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 22px;
    background: rgb(255 255 255 / 0.025);
}

.connection-orb.connected {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.2);
    background: rgb(45 212 191 / 0.08);
    box-shadow: 0 0 45px rgb(45 212 191 / 0.08);
}

.workspace {
    display: grid;
    grid-template-columns: minmax(0, 1.16fr) minmax(300px, 0.84fr);
    gap: 16px;
}

.panel {
    padding: 22px;
    border: 1px solid rgb(255 255 255 / 0.075);
    border-radius: 17px;
    background: rgb(12 25 43 / 0.72);
    box-shadow: 0 18px 45px rgb(0 0 0 / 0.13);
    backdrop-filter: blur(14px);
}

.pairing-panel,
.runner-panel {
    min-height: 390px;
}

.panel-heading {
    gap: 11px;
}

.panel-heading.compact {
    align-items: flex-start;
}

.panel-heading > div {
    min-width: 0;
}

.icon-box {
    width: 38px;
    height: 38px;
    flex: 0 0 auto;
    color: #99f6e4;
    border-radius: 11px;
    background: rgb(45 212 191 / 0.09);
}

.icon-box.blue {
    color: #7dd3fc;
    background: rgb(56 189 248 / 0.08);
}

.panel h2 {
    margin-bottom: 3px;
    color: white;
    font-size: 15px;
}

.panel-heading p,
.runner-summary p {
    margin-bottom: 0;
    overflow: hidden;
    color: #64748b;
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.form-grid {
    display: grid;
    gap: 14px;
    margin-top: 26px;
}

label {
    display: grid;
    gap: 7px;
}

label span,
dt {
    color: #94a3b8;
    font-size: 11px;
    font-weight: 600;
}

input {
    width: 100%;
    padding: 10px 12px;
    color: #e2e8f0;
    outline: none;
    border: 1px solid rgb(255 255 255 / 0.09);
    border-radius: 9px;
    background: rgb(2 8 20 / 0.55);
    transition:
        border-color 150ms ease,
        box-shadow 150ms ease;
}

input:focus {
    border-color: rgb(94 234 212 / 0.45);
    box-shadow: 0 0 0 3px rgb(45 212 191 / 0.08);
}

.code-field input {
    font-family: ui-monospace, monospace;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
}

.primary-button,
.quiet-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    padding: 10px 14px;
    font-size: 12px;
    font-weight: 700;
    border-radius: 9px;
    transition:
        background 150ms ease,
        border-color 150ms ease;
}

.primary-button {
    color: #042f2e;
    border: 1px solid #5eead4;
    background: #5eead4;
}

.primary-button:hover:not(:disabled) {
    background: #99f6e4;
}

.quiet-button {
    color: #94a3b8;
    border: 1px solid rgb(255 255 255 / 0.08);
    background: rgb(255 255 255 / 0.025);
}

.quiet-button:hover {
    color: #e2e8f0;
    border-color: rgb(255 255 255 / 0.14);
}

.error-banner {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    margin-bottom: 16px;
    padding: 11px 13px;
    color: #fecdd3;
    font-size: 12px;
    line-height: 1.5;
    border: 1px solid rgb(251 113 133 / 0.18);
    border-radius: 10px;
    background: rgb(244 63 94 / 0.08);
}

.runner-summary {
    gap: 13px;
}

.runner-icon {
    width: 46px;
    height: 46px;
    flex: 0 0 auto;
    color: #cbd5e1;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 13px;
    background: rgb(255 255 255 / 0.035);
}

.runner-summary > div {
    min-width: 0;
}

.title-row {
    gap: 9px;
}

.runner-summary h2 {
    margin: 0;
    font-size: 17px;
}

.online-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 7px;
    color: #a7f3d0;
    font-size: 9px;
    font-weight: 700;
    border: 1px solid rgb(52 211 153 / 0.15);
    border-radius: 99px;
    background: rgb(52 211 153 / 0.07);
}

.metadata {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1px;
    overflow: hidden;
    margin: 26px 0;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 11px;
    background: rgb(255 255 255 / 0.06);
}

.metadata div {
    padding: 13px;
    background: #0a1728;
}

dd {
    margin: 5px 0 0;
    color: #cbd5e1;
    font-family: ui-monospace, monospace;
    font-size: 11px;
}

.runner-actions {
    gap: 9px;
}

.runner-actions .primary-button {
    flex: 1;
}

.checking {
    gap: 5px;
    margin-left: auto;
    color: #7dd3fc;
    font-family: ui-monospace, monospace;
    font-size: 9px;
}

.activity-list {
    margin-top: 21px;
}

.activity-list article {
    display: flex;
    gap: 10px;
    padding: 12px 0;
    border-top: 1px solid rgb(255 255 255 / 0.055);
}

.activity-icon {
    display: grid;
    width: 27px;
    height: 27px;
    flex: 0 0 auto;
    place-items: center;
    color: #94a3b8;
    border-radius: 9px;
    background: rgb(255 255 255 / 0.035);
}

.activity-icon.completed {
    color: #6ee7b7;
    background: rgb(52 211 153 / 0.08);
}

.activity-list p {
    margin-bottom: 3px;
    color: #cbd5e1;
    font-size: 11px;
    line-height: 1.45;
}

.activity-list small {
    color: #475569;
    font-family: ui-monospace, monospace;
    font-size: 9px;
}

.empty-activity,
.loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    color: #64748b;
}

.empty-activity {
    min-height: 220px;
    flex-direction: column;
    gap: 10px;
    text-align: center;
}

.empty-activity p {
    max-width: 210px;
    margin: 0;
    font-size: 11px;
    line-height: 1.6;
}

.loading-state {
    gap: 9px;
    font-size: 12px;
}

footer {
    padding-top: 11px;
    padding-bottom: 11px;
    color: #475569;
    font-family: ui-monospace, monospace;
    font-size: 9px;
    border-top: 1px solid rgb(255 255 255 / 0.06);
}

footer span {
    gap: 6px;
}

.spin {
    animation: spin 850ms linear infinite;
}

@keyframes spin {
    to {
        transform: rotate(360deg);
    }
}

@media (max-width: 760px) {
    .topbar {
        gap: 10px;
        padding-right: 16px;
        padding-left: 16px;
    }

    .brand small,
    .view-tabs button {
        font-size: 0;
    }

    .view-tabs button {
        gap: 0;
        padding: 8px;
    }

    .content {
        padding: 24px 18px;
    }

    .workspace {
        grid-template-columns: 1fr;
    }

    .hero {
        align-items: flex-start;
    }

    .connection-orb {
        width: 54px;
        height: 54px;
        border-radius: 16px;
    }

    .machine-chip span {
        display: none;
    }
}
</style>
