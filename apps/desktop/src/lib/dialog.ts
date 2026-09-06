import type { InjectionKey, Ref } from 'vue';

/** Popups belong in their dialog's top layer, where they remain visible and interactive. */
export const dialogKey: InjectionKey<Ref<HTMLDialogElement | null>> = Symbol('dialog');

/** Only intercept Tab at an edge; the browser handles the ordinary sequence between controls. */
export function tabBoundary<T>(
    controls: readonly T[],
    active: T | null,
    backward: boolean,
): T | null {
    if (controls.length === 0) {
        return null;
    }
    const first = controls[0]!;
    const last = controls[controls.length - 1]!;
    if (active === null || !controls.includes(active)) {
        return backward ? last : first;
    }
    if (backward && active === first) {
        return last;
    }
    if (!backward && active === last) {
        return first;
    }
    return null;
}

export function containDialogTab(dialog: HTMLDialogElement, event: KeyboardEvent): void {
    if (event.key !== 'Tab' || event.defaultPrevented) {
        return;
    }
    const candidates = dialog.querySelectorAll<HTMLElement>(
        'button, input, textarea, select, a[href], summary, [tabindex], [contenteditable="true"]',
    );
    const controls = [...candidates].filter(
        (element) =>
            element.tabIndex >= 0 &&
            !element.matches(':disabled') &&
            !element.closest('[inert]') &&
            element.closest('dialog') === dialog &&
            element.getClientRects().length > 0,
    );
    const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const target = tabBoundary(controls, active, event.shiftKey);
    if (target || controls.length === 0) {
        event.preventDefault();
        (target ?? dialog).focus();
    }
}
