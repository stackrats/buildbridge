import { describe, expect, it } from 'vite-plus/test';

import type { EnvSetSummary } from '../types/backend';
import {
    describeEnvSetSize,
    envDraftIssues,
    envDraftRows,
    envDraftVariables,
    envKeyIssue,
    envValueIssue,
} from './envs';

describe('env keys', () => {
    it('accepts what a dotenv loader and a shell both accept', () => {
        for (const key of ['VITE_API_URL', '_private', 'A1', 'lower_case']) {
            expect(envKeyIssue(key)).toBeNull();
        }
    });

    it('names the problem with a bad key', () => {
        expect(envKeyIssue('')).toBe('needs a name');
        for (const key of ['1ABC', 'MY-KEY', 'MY KEY', 'a.b']) {
            expect(envKeyIssue(key)).toContain('letters, digits and underscores');
        }
    });
});

describe('env values', () => {
    it('allows ordinary values, including either kind of quote alone', () => {
        expect(envValueIssue('https://api.example.com')).toBeNull();
        expect(envValueIssue("it's fine")).toBeNull();
        expect(envValueIssue('say "hi"')).toBeNull();
    });

    it('refuses what the two guest renderings would disagree on', () => {
        expect(envValueIssue('both \' and "')).toContain('both');
        expect(envValueIssue('two\nlines')).toContain('line breaks');
        expect(envValueIssue('x'.repeat(4097))).toContain('4096');
    });
});

describe('draft issues', () => {
    it('is empty for a saveable draft, keeping blank stored secrets', () => {
        expect(
            envDraftIssues([
                { key: 'VITE_API_URL', value: 'https://api.example', secret: false, stored: true },
                { key: 'VITE_SENTRY_DSN', value: '', secret: true, stored: true },
                { key: 'VITE_FLAG', value: 'on', secret: false, stored: false },
            ]),
        ).toEqual([]);
    });

    it('needs a value for a plain variable even when stored, and for a new secret', () => {
        expect(
            envDraftIssues([
                { key: 'VITE_API_URL', value: '', secret: false, stored: true },
                { key: 'TOKEN', value: '', secret: true, stored: false },
            ]),
        ).toEqual(['VITE_API_URL needs a value.', 'TOKEN needs a value.']);
    });

    it('lists every problem in row order, numbering unnamed rows within their section', () => {
        expect(
            envDraftIssues([
                { key: '', value: 'x', secret: false, stored: false },
                { key: 'NEW', value: '', secret: false, stored: false },
                { key: 'A', value: '1', secret: false, stored: false },
                { key: 'A', value: '2', secret: true, stored: false },
                { key: '', value: 'y', secret: true, stored: false },
            ]),
        ).toEqual([
            'Variable 1 needs a name.',
            'NEW needs a value.',
            'A is listed twice.',
            'Secret 2 needs a name.',
        ]);
    });
});

describe('editor rows', () => {
    const set: EnvSetSummary = {
        id: 'production',
        name: 'production',
        variables: [{ key: 'VITE_API_URL', value: 'https://api.example' }],
        secretKeys: ['VITE_SENTRY_DSN'],
        createdAtEpochSeconds: 0,
        attachedMachines: [],
    };

    it('seeds plain variables with their values and secrets blank', () => {
        expect(envDraftRows(set)).toEqual({
            variables: [
                { key: 'VITE_API_URL', value: 'https://api.example', secret: false, stored: true },
            ],
            secrets: [{ key: 'VITE_SENTRY_DSN', value: '', secret: true, stored: true }],
        });
        expect(envDraftRows(null)).toEqual({ variables: [], secrets: [] });
    });

    it('sends null only for a stored secret left blank', () => {
        expect(
            envDraftVariables([
                {
                    key: ' VITE_API_URL ',
                    value: 'https://api.example',
                    secret: false,
                    stored: true,
                },
                { key: 'VITE_SENTRY_DSN', value: '', secret: true, stored: true },
                { key: 'TOKEN', value: 'fresh', secret: true, stored: true },
            ]),
        ).toEqual([
            { key: 'VITE_API_URL', value: 'https://api.example', secret: false },
            { key: 'VITE_SENTRY_DSN', value: null, secret: true },
            { key: 'TOKEN', value: 'fresh', secret: true },
        ]);
    });

    it('describes what a set holds', () => {
        expect(describeEnvSetSize(set)).toBe('1 variable, 1 secret');
        expect(describeEnvSetSize({ variables: [], secretKeys: ['A', 'B'] })).toBe('2 secrets');
        expect(describeEnvSetSize({ variables: [], secretKeys: [] })).toBe('empty');
    });
});
