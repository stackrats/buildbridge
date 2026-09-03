// Per-viewer preferences kept in localStorage. Every read is guarded so the modules load in
// unit tests without a DOM and fall back to defaults when storage is unavailable.

export type Theme = 'system' | 'light' | 'dark';

const THEME_KEY = 'buildbridge.theme';
const SIDEBAR_WIDTH_KEY = 'buildbridge.sidebar-width';
const SELECTION_KEY = 'buildbridge.selection';
const LOG_WRAP_KEY = 'buildbridge.log-wrap';
const LOG_HEIGHT_KEY = 'buildbridge.log-height';

const local = typeof localStorage === 'undefined' ? null : localStorage;

function read(key: string): string | null {
    try {
        return local?.getItem(key) ?? null;
    } catch {
        return null;
    }
}

function write(key: string, value: string): void {
    try {
        local?.setItem(key, value);
    } catch {
        // Storage can be unavailable in private windows; preferences simply do not persist.
    }
}

export function loadTheme(): Theme {
    const raw = read(THEME_KEY);
    return raw === 'light' || raw === 'dark' ? raw : 'system';
}

export function saveTheme(theme: Theme): void {
    write(THEME_KEY, theme);
}

const media =
    typeof window === 'undefined' ? null : window.matchMedia('(prefers-color-scheme: dark)');
let current: Theme = 'system';

function sync(): void {
    const dark = current === 'dark' || (current === 'system' && (media?.matches ?? false));
    document.documentElement.classList.toggle('dark', dark);
}

/** Apply and remember a theme; "system" follows the operating system live. */
export function applyTheme(theme: Theme): void {
    current = theme;
    sync();
}

media?.addEventListener('change', sync);

export function loadSidebarWidth(): number {
    const value = Number(read(SIDEBAR_WIDTH_KEY));
    return Number.isFinite(value) && value >= 180 && value <= 420 ? value : 232;
}

export function saveSidebarWidth(width: number): void {
    write(SIDEBAR_WIDTH_KEY, String(Math.round(width)));
}

export function loadSelection(): string | null {
    return read(SELECTION_KEY);
}

export function saveSelection(selection: string): void {
    write(SELECTION_KEY, selection);
}

export function loadLogWrap(): boolean {
    return read(LOG_WRAP_KEY) === 'true';
}

export function saveLogWrap(wrap: boolean): void {
    write(LOG_WRAP_KEY, String(wrap));
}

export const LOG_HEIGHT_MIN = 120;
export const LOG_HEIGHT_MAX = 600;

export function loadLogHeight(): number {
    const value = Number(read(LOG_HEIGHT_KEY));
    return Number.isFinite(value) && value >= LOG_HEIGHT_MIN && value <= LOG_HEIGHT_MAX
        ? value
        : 224;
}

export function saveLogHeight(height: number): void {
    write(LOG_HEIGHT_KEY, String(Math.round(height)));
}
