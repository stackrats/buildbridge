import { describe, expect, it } from 'vite-plus/test';

import {
    firstEnabledIndex,
    lastEnabledIndex,
    nextEnabledIndex,
    typeaheadIndex,
    type ListboxOption,
} from './listbox';

const options: ListboxOption[] = [
    { value: 'sequoia', label: 'macOS Sequoia' },
    { value: 'sonoma', label: 'macOS Sonoma', disabled: true },
    { value: 'ventura', label: 'macOS Ventura' },
    { value: 'tahoe', label: 'macOS Tahoe' },
];

describe('listbox movement', () => {
    it('steps over an option that cannot be chosen', () => {
        expect(nextEnabledIndex(options, 0, 1)).toBe(2);
        expect(nextEnabledIndex(options, 2, -1)).toBe(0);
    });

    it('stops at the ends instead of wrapping', () => {
        expect(nextEnabledIndex(options, 3, 1)).toBe(3);
        expect(nextEnabledIndex(options, 0, -1)).toBe(0);
    });

    it('finds the first and last choosable option', () => {
        expect(firstEnabledIndex(options)).toBe(0);
        expect(lastEnabledIndex(options)).toBe(3);
        expect(firstEnabledIndex([{ value: 'a', label: 'A', disabled: true }])).toBe(-1);
        expect(firstEnabledIndex([])).toBe(-1);
    });

    it('reports nothing to move to when every option is disabled', () => {
        const locked = options.map((option) => ({ ...option, disabled: true }));

        expect(nextEnabledIndex(locked, 1, 1)).toBe(-1);
    });
});

describe('listbox typeahead', () => {
    it('matches a label prefix, case-insensitively', () => {
        expect(typeaheadIndex(options, 'macos v', -1)).toBe(2);
        expect(typeaheadIndex(options, 'MACOS T', -1)).toBe(3);
    });

    it('searches after the current option so a repeated letter walks the list', () => {
        expect(typeaheadIndex(options, 'macos', -1)).toBe(0);
        expect(typeaheadIndex(options, 'macos', 0)).toBe(2);
        expect(typeaheadIndex(options, 'macos', 3)).toBe(0);
    });

    it('never lands on a disabled option or an empty query', () => {
        expect(typeaheadIndex(options, 'macos so', -1)).toBe(-1);
        expect(typeaheadIndex(options, '  ', -1)).toBe(-1);
    });
});
