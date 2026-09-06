import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import {
    LOG_HEIGHT_MAX,
    LOG_HEIGHT_MIN,
    loadLogHeight,
    loadLogWrap,
    loadSelection,
    loadSidebarWidth,
    loadTheme,
    saveLogHeight,
    saveLogWrap,
    saveSelection,
    saveSidebarWidth,
    saveTheme,
} from './prefs';

let storage: Map<string, string>;

beforeEach(() => {
    storage = new Map();
    vi.stubGlobal('localStorage', {
        getItem: (name: string) => storage.get(name) ?? null,
        setItem: (name: string, value: string) => storage.set(name, value),
    });
});

afterEach(() => {
    vi.unstubAllGlobals();
});

describe('viewer preferences', () => {
    it('follows the system theme unless a light or dark choice was stored', () => {
        expect(loadTheme()).toBe('system');
        for (const raw of ['', 'blue', 'DARK', 'null']) {
            storage.set('buildbridge.theme', raw);
            expect(loadTheme(), raw).toBe('system');
        }
        saveTheme('dark');
        expect(loadTheme()).toBe('dark');
        saveTheme('light');
        expect(loadTheme()).toBe('light');
    });

    it('keeps the sidebar width and log height within their bounds, rounding what it saves', () => {
        expect(loadSidebarWidth()).toBe(232);
        expect(loadLogHeight()).toBe(224);
        for (const raw of ['179', '421', 'wide', '', 'Infinity']) {
            storage.set('buildbridge.sidebar-width', raw);
            expect(loadSidebarWidth(), raw).toBe(232);
        }
        saveSidebarWidth(300.4);
        expect(storage.get('buildbridge.sidebar-width')).toBe('300');
        expect(loadSidebarWidth()).toBe(300);
        for (const raw of [String(LOG_HEIGHT_MIN - 1), String(LOG_HEIGHT_MAX + 1), 'tall']) {
            storage.set('buildbridge.log-height', raw);
            expect(loadLogHeight(), raw).toBe(224);
        }
        saveLogHeight(LOG_HEIGHT_MAX);
        expect(loadLogHeight()).toBe(LOG_HEIGHT_MAX);
        saveLogHeight(LOG_HEIGHT_MIN + 0.4);
        expect(loadLogHeight()).toBe(LOG_HEIGHT_MIN);
    });

    it('reads the log wrap flag strictly and the selection verbatim', () => {
        expect(loadLogWrap()).toBe(false);
        for (const raw of ['TRUE', '1', 'yes']) {
            storage.set('buildbridge.log-wrap', raw);
            expect(loadLogWrap(), raw).toBe(false);
        }
        saveLogWrap(true);
        expect(loadLogWrap()).toBe(true);
        expect(loadSelection()).toBeNull();
        saveSelection('machine:pixel-builder');
        expect(loadSelection()).toBe('machine:pixel-builder');
    });

    it.each(['missing', 'denied', 'getter'])(
        'answers with defaults and saves quietly when localStorage is %s',
        (failure) => {
            const fail = () => {
                throw new Error('Storage unavailable');
            };
            if (failure === 'missing') vi.stubGlobal('localStorage', undefined);
            if (failure === 'denied')
                vi.stubGlobal('localStorage', { getItem: fail, setItem: fail });
            if (failure === 'getter') {
                Object.defineProperty(globalThis, 'localStorage', {
                    configurable: true,
                    get: fail,
                });
            }
            expect(() => {
                saveTheme('dark');
                saveSidebarWidth(300);
                saveLogHeight(300);
                saveLogWrap(true);
                saveSelection('envs');
            }).not.toThrow();
            expect(loadTheme()).toBe('system');
            expect(loadSidebarWidth()).toBe(232);
            expect(loadLogHeight()).toBe(224);
            expect(loadLogWrap()).toBe(false);
            expect(loadSelection()).toBeNull();
        },
    );
});
