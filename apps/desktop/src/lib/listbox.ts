// Keyboard movement for a listbox, kept out of the component so it can be tested directly.

import type { MachinePlatform } from '../types/backend';

export interface ListboxOption {
    value: string;
    label: string;
    /** Consequences or guidance shown below the option name without truncation. */
    description?: string;
    /** Platforms represented by this choice, shown with the shared accessible icons. */
    platforms?: MachinePlatform[];
    disabled?: boolean;
}

function selectable(options: ListboxOption[], index: number): boolean {
    const option = options[index];
    return option !== undefined && option.disabled !== true;
}

/**
 * The next selectable index in a direction. Movement stops at the ends rather than wrapping, and
 * an unselectable option is stepped over rather than landed on. Returns the starting index when
 * there is nowhere to go, so a held arrow key is a no-op at the end of a list.
 */
export function nextEnabledIndex(options: ListboxOption[], from: number, step: number): number {
    for (let index = from + step; index >= 0 && index < options.length; index += step) {
        if (selectable(options, index)) {
            return index;
        }
    }

    return selectable(options, from) ? from : -1;
}

export function firstEnabledIndex(options: ListboxOption[]): number {
    return nextEnabledIndex(options, -1, 1);
}

export function lastEnabledIndex(options: ListboxOption[]): number {
    return nextEnabledIndex(options, options.length, -1);
}

/**
 * The option a typed prefix points at, searching after the current one first so repeatedly
 * typing the same letter walks through the options that share it.
 */
export function typeaheadIndex(options: ListboxOption[], query: string, from: number): number {
    const needle = query.trim().toLowerCase();
    if (needle === '') {
        return -1;
    }

    const order = [
        ...options.slice(from + 1).map((_, offset) => from + 1 + offset),
        ...options.slice(0, from + 1).map((_, index) => index),
    ];

    return (
        order.find(
            (index) =>
                selectable(options, index) &&
                options[index]!.label.toLowerCase().startsWith(needle),
        ) ?? -1
    );
}
