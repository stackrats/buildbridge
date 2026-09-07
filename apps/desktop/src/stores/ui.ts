// Navigation and window-level view state. Plain reactive singletons keep the app free of a
// store dependency while still giving every component one source of truth.

import { computed, reactive } from 'vue';

import { useMachineOrder } from './machine-order';

import {
    loadLogHeight,
    loadSelection,
    loadSidebarWidth,
    saveLogHeight,
    saveSelection,
    saveSidebarWidth,
} from '../lib/prefs';

export type LogSource = 'activity' | 'build' | 'archive' | 'device' | 'console';

export type Route =
    | { kind: 'home' }
    | { kind: 'runner' }
    | { kind: 'signing' }
    | { kind: 'envs' }
    | { kind: 'templates' }
    | { kind: 'native_mac' }
    | { kind: 'machine'; id: string };

function devParams(): URLSearchParams | null {
    return import.meta.env.DEV && typeof location !== 'undefined'
        ? new URLSearchParams(location.search)
        : null;
}

/** In development a `?view=machine:default&step=archive` query selects the initial screen. */
function initialSelection(): string | null {
    const view = devParams()?.get('view');
    return view || loadSelection();
}

function initialSteps(): Record<string, string> {
    const params = devParams();
    const view = params?.get('view');
    const step = params?.get('step');
    if (view?.startsWith('machine:') && step) {
        return { [view.slice('machine:'.length)]: step };
    }
    return {};
}

/** `?log=1` (or the older `?tab=logs`) opens the log drawer, so the preview can show it. */
function initialLogOpen(): Record<string, boolean> {
    const params = devParams();
    const view = params?.get('view');
    if (view?.startsWith('machine:') && (params?.has('log') || params?.get('tab') === 'logs')) {
        return { [view.slice('machine:'.length)]: true };
    }
    return {};
}

/** `?newMachine=1` opens the create dialog, so the browser preview can show it. */
function initialNewMachineOpen(): boolean {
    return devParams()?.has('newMachine') ?? false;
}

/** `?settings=1` opens the settings dialog in the browser preview. */
function initialSettingsOpen(): boolean {
    return devParams()?.has('settings') ?? false;
}

const state = reactive({
    route: parseSelection(initialSelection()) as Route,
    sidebarWidth: loadSidebarWidth(),
    /** The step each machine page is showing; the focus step when unset. */
    machineSteps: initialSteps(),
    /**
     * Which sections of each machine page a person opened or closed — the timeline's phases and
     * the sections after it — so a page comes back as it was left. Unset means the page's own
     * default for that section.
     */
    machineSections: {} as Record<string, Record<string, boolean>>,
    logOpen: initialLogOpen(),
    logSource: {} as Record<string, LogSource>,
    logHeight: loadLogHeight(),
    newMachineOpen: initialNewMachineOpen(),
    settingsOpen: initialSettingsOpen(),
    /** The template the new machine dialog opens with chosen, from the templates page. */
    newMachineTemplateId: null as string | null,
});

function parseSelection(value: string | null): Route {
    if (!value) {
        return { kind: 'home' };
    }
    if (
        value === 'runner' ||
        value === 'signing' ||
        value === 'envs' ||
        value === 'templates' ||
        value === 'native_mac' ||
        value === 'home'
    ) {
        return { kind: value };
    }
    if (value.startsWith('machine:')) {
        return { kind: 'machine', id: value.slice('machine:'.length) };
    }
    return { kind: 'home' };
}

function serializeRoute(route: Route): string {
    return route.kind === 'machine' ? `machine:${route.id}` : route.kind;
}

export function useUi() {
    return {
        state,
        route: computed(() => state.route),
        navigate(route: Route): void {
            state.route = route;
            saveSelection(serializeRoute(route));
            if (route.kind === 'machine') {
                useMachineOrder().markUsed(route.id);
            }
        },
        /**
         * Show a machine, optionally on one step. Without one the page comes back as it was
         * left: the step a person had open, or the focus step when they had chosen none.
         */
        openMachine(id: string, stepId?: string): void {
            if (stepId) {
                state.machineSteps[id] = stepId;
            }
            state.route = { kind: 'machine', id };
            saveSelection(serializeRoute(state.route));
            useMachineOrder().markUsed(id);
        },
        /** A step id, '' when the person closed it, or null when they have not chosen. */
        selectedStep(id: string): string | null {
            return state.machineSteps[id] ?? null;
        },
        selectStep(id: string, stepId: string | null): void {
            state.machineSteps[id] = stepId ?? '';
        },
        /** A person's choice for one section of a machine page, or null for the page's default. */
        sectionOpen(id: string, section: string): boolean | null {
            return state.machineSections[id]?.[section] ?? null;
        },
        setSectionOpen(id: string, section: string, open: boolean): void {
            state.machineSections[id] = { ...state.machineSections[id], [section]: open };
        },
        isLogOpen(id: string): boolean {
            return state.logOpen[id] ?? false;
        },
        openLog(id: string, source?: LogSource): void {
            state.logOpen[id] = true;
            if (source) {
                state.logSource[id] = source;
            }
        },
        /** Points the drawer at a source without opening it. */
        selectLog(id: string, source: LogSource): void {
            state.logSource[id] = source;
        },
        closeLog(id: string): void {
            state.logOpen[id] = false;
        },
        logSource(id: string): LogSource {
            return state.logSource[id] ?? 'activity';
        },
        setLogSource(id: string, source: LogSource): void {
            state.logSource[id] = source;
        },
        setLogHeight(height: number): void {
            state.logHeight = height;
            saveLogHeight(height);
        },
        setSidebarWidth(width: number): void {
            state.sidebarWidth = width;
            saveSidebarWidth(width);
        },
    };
}
