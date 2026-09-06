import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { hideTip, showTip, TOOLTIP_ID, tooltip } from './tooltip';

function anchor(description?: string): HTMLElement {
    const attributes = new Map<string, string>();
    if (description) {
        attributes.set('aria-describedby', description);
    }
    return {
        getAttribute: (name: string) => attributes.get(name) ?? null,
        setAttribute: (name: string, value: string) => attributes.set(name, value),
        removeAttribute: (name: string) => attributes.delete(name),
    } as unknown as HTMLElement;
}

describe('tooltip descriptions', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => {
        hideTip();
        vi.useRealTimers();
    });

    it('keeps the control help text associated when a tooltip opens and closes', () => {
        const control = anchor('field-help button-reason');

        showTip(control, 'More information');
        vi.runAllTimers();
        expect(control.getAttribute('aria-describedby')).toBe(
            `field-help button-reason ${TOOLTIP_ID}`,
        );

        hideTip(control);
        expect(control.getAttribute('aria-describedby')).toBe('field-help button-reason');
        expect(tooltip.open).toBe(false);
    });

    it('does not add duplicate references after repeated focus or hover', () => {
        const control = anchor();
        showTip(control, 'More information');
        vi.runAllTimers();
        showTip(control, 'Updated information');
        vi.runAllTimers();

        expect(control.getAttribute('aria-describedby')).toBe(TOOLTIP_ID);
        hideTip(control);
        expect(control.getAttribute('aria-describedby')).toBeNull();
    });

    it('leaving before the delay does not remove existing field help', () => {
        const control = anchor('field-help');
        showTip(control, 'More information');
        hideTip(control);
        vi.runAllTimers();

        expect(control.getAttribute('aria-describedby')).toBe('field-help');
        expect(tooltip.open).toBe(false);
    });
});
