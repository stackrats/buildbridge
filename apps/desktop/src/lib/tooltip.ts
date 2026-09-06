// One tooltip for the whole window, driven by a `v-tip` directive.
//
// The native `title` attribute is not an option here: the Linux webview does not render it,
// it cannot be styled to match anything, and it never appears for a disabled control — which
// is exactly when the reason for a control matters most. A single shared bubble also means
// only one tooltip can ever be on screen, and only one element is added to the document.

import { reactive, type Directive } from 'vue';

export const TOOLTIP_ID = 'buildbridge-tooltip';

const OPEN_DELAY_MS = 350;

export const tooltip = reactive({
    open: false,
    text: '',
    anchor: null as HTMLElement | null,
});

let timer: ReturnType<typeof setTimeout> | null = null;

/** Text is read at hover time so a label that changes with state is always current. */
const texts = new WeakMap<HTMLElement, string>();

export function showTip(element: HTMLElement, text: string): void {
    if (!text.trim()) {
        return;
    }
    if (timer) {
        clearTimeout(timer);
    }
    timer = setTimeout(() => {
        timer = null;
        tooltip.anchor = element;
        tooltip.text = text;
        tooltip.open = true;
        const descriptions = new Set(
            (element.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean),
        );
        descriptions.add(TOOLTIP_ID);
        element.setAttribute('aria-describedby', [...descriptions].join(' '));
    }, OPEN_DELAY_MS);
}

export function hideTip(element?: HTMLElement): void {
    if (timer) {
        clearTimeout(timer);
        timer = null;
    }
    const owner = element ?? tooltip.anchor;
    if (owner) {
        const descriptions = (owner.getAttribute('aria-describedby') ?? '')
            .split(/\s+/)
            .filter((id) => id && id !== TOOLTIP_ID);
        if (descriptions.length) {
            owner.setAttribute('aria-describedby', descriptions.join(' '));
        } else {
            owner.removeAttribute('aria-describedby');
        }
    }
    tooltip.open = false;
    tooltip.anchor = null;
}

function enter(event: Event): void {
    const element = event.currentTarget as HTMLElement;
    const text = texts.get(element);
    if (text) {
        showTip(element, text);
    }
}

function leave(event: Event): void {
    hideTip(event.currentTarget as HTMLElement);
}

function dismiss(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
        hideTip(event.currentTarget as HTMLElement);
    }
}

function listen(element: HTMLElement): void {
    element.addEventListener('mouseenter', enter);
    element.addEventListener('mouseleave', leave);
    element.addEventListener('focusin', enter);
    element.addEventListener('focusout', leave);
    element.addEventListener('keydown', dismiss);
}

function unlisten(element: HTMLElement): void {
    element.removeEventListener('mouseenter', enter);
    element.removeEventListener('mouseleave', leave);
    element.removeEventListener('focusin', enter);
    element.removeEventListener('focusout', leave);
    element.removeEventListener('keydown', dismiss);
}

/** `v-tip="'What this control does'"`; a null or empty value simply does nothing. */
export const vTip: Directive<HTMLElement, string | null | undefined> = {
    mounted(element, binding) {
        if (binding.value) {
            texts.set(element, binding.value);
            listen(element);
        }
    },
    updated(element, binding) {
        const had = texts.has(element);
        if (binding.value) {
            texts.set(element, binding.value);
            if (!had) {
                listen(element);
            }
            if (tooltip.open && tooltip.anchor === element) {
                tooltip.text = binding.value;
            }
        } else if (had) {
            texts.delete(element);
            unlisten(element);
            if (tooltip.anchor === element) {
                hideTip(element);
            }
        }
    },
    beforeUnmount(element) {
        if (tooltip.anchor === element) {
            hideTip(element);
        }
        texts.delete(element);
        unlisten(element);
    },
};
