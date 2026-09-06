import { describe, expect, it } from 'vite-plus/test';

import { tabBoundary } from './dialog';

describe('dialog keyboard boundaries', () => {
    const controls = ['close', 'name', 'advanced', 'cancel', 'save'];

    it('wraps from the last action to the first control', () => {
        expect(tabBoundary(controls, 'save', false)).toBe('close');
    });

    it('wraps backwards from the first control to the last action', () => {
        expect(tabBoundary(controls, 'close', true)).toBe('save');
    });

    it('leaves normal field-to-field tab order to the browser', () => {
        expect(tabBoundary(controls, 'name', false)).toBeNull();
        expect(tabBoundary(controls, 'name', true)).toBeNull();
    });

    it('enters from a focused heading or from a control that was removed', () => {
        expect(tabBoundary(controls, 'heading', false)).toBe('close');
        expect(tabBoundary(controls, 'removed-control', true)).toBe('save');
        expect(tabBoundary(controls, null, false)).toBe('close');
    });

    it('keeps a dialog with a single control on that control', () => {
        expect(tabBoundary(['close'], 'close', false)).toBe('close');
        expect(tabBoundary(['close'], 'close', true)).toBe('close');
    });

    it('reports no target when every control is unavailable', () => {
        expect(tabBoundary([], null, false)).toBeNull();
    });
});
