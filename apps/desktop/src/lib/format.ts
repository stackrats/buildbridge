// Formatting helpers shared by every pane. Pure functions, unit-tested alongside the models.

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function formatBytes(bytes: number | null | undefined): string {
    if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) {
        return '—';
    }
    let value = bytes;
    let unit = 0;
    while (value >= 1000 && unit < BYTE_UNITS.length - 1) {
        value /= 1000;
        unit += 1;
    }
    const digits = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
    return `${value.toFixed(digits)} ${BYTE_UNITS[unit]}`;
}

export function formatElapsed(totalSeconds: number | null | undefined): string {
    if (totalSeconds === null || totalSeconds === undefined || !Number.isFinite(totalSeconds)) {
        return '—';
    }
    const seconds = Math.max(0, Math.floor(totalSeconds));
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const rest = seconds % 60;
    if (hours > 0) {
        return `${hours}h ${minutes}m`;
    }
    if (minutes > 0) {
        return `${minutes}m ${rest}s`;
    }
    return `${rest}s`;
}

/** Elapsed seconds since an ISO-8601 timestamp, tolerating Docker's nine-digit fractions. */
export function secondsSince(iso: string | null | undefined, now = Date.now()): number | null {
    if (!iso) {
        return null;
    }
    const normalized = iso.replace(/(\.\d{3})\d+/, '$1');
    const started = new Date(normalized).getTime();
    if (!Number.isFinite(started)) {
        return null;
    }
    return Math.max(0, Math.floor((now - started) / 1000));
}

export function relativeTime(iso: string | null | undefined, now = Date.now()): string {
    if (!iso) {
        return 'never';
    }
    const seconds = Math.round((new Date(iso).getTime() - now) / 1000);
    if (!Number.isFinite(seconds)) {
        return 'unknown';
    }
    const formatter = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
    if (Math.abs(seconds) < 60) {
        return formatter.format(seconds, 'second');
    }
    if (Math.abs(seconds) < 3600) {
        return formatter.format(Math.round(seconds / 60), 'minute');
    }
    if (Math.abs(seconds) < 86_400) {
        return formatter.format(Math.round(seconds / 3600), 'hour');
    }
    return formatter.format(Math.round(seconds / 86_400), 'day');
}

export function formatDate(iso: string | null | undefined): string {
    if (!iso) {
        return '—';
    }
    const date = new Date(iso);
    if (!Number.isFinite(date.getTime())) {
        return iso;
    }
    return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function formatTime(epochMilliseconds: number): string {
    return new Date(epochMilliseconds).toLocaleTimeString(undefined, {
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
    });
}

export function shortHash(value: string | null | undefined, length = 12): string {
    if (!value) {
        return '—';
    }
    return value.length <= length ? value : `${value.slice(0, length)}…`;
}

export function isExpired(iso: string | null | undefined, now = Date.now()): boolean {
    if (!iso) {
        return false;
    }
    const time = new Date(iso).getTime();
    return Number.isFinite(time) && time <= now;
}

export function percent(completed: number, total: number): number | null {
    if (!Number.isFinite(completed) || !Number.isFinite(total) || total <= 0) {
        return null;
    }
    return Math.max(0, Math.min(1, completed / total));
}

export function fileName(path: string | null | undefined): string {
    if (!path) {
        return '';
    }
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] ?? path;
}
