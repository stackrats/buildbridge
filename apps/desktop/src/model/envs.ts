// What an env set edit still needs before it can be stored, and how the editor's rows map to
// and from the vault.
//
// A set holds plain variables, which read back with their values, and secrets, which come back
// only to the editor, masked. Editing a stored set seeds a secret with a blank value until that
// fetch lands: blank on a stored secret means "keep it", blank anywhere else is a mistake worth
// naming before saving. The Rust side enforces the same rules; keeping them here lets the dialog
// say so first.

import type { EnvSetSummary, EnvVariableInput } from '../types/backend';

export interface EnvVariableDraft {
    key: string;
    value: string;
    /** Masked while typing and left out of the summary once stored. */
    secret: boolean;
    /** The key already exists in the vault. For a secret, a blank value then keeps what is stored. */
    stored: boolean;
}

const KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;
const MAX_VALUE_LENGTH = 4096;

/** A variable name as every dotenv loader and POSIX shell agree on it. */
export function envKeyIssue(key: string): string | null {
    const trimmed = key.trim();
    if (trimmed === '') {
        return 'needs a name';
    }
    if (trimmed.length > 120 || !KEY_PATTERN.test(trimmed)) {
        return 'can only use letters, digits and underscores, and cannot start with a digit';
    }

    return null;
}

/**
 * Why a value cannot be stored. The set is written twice in the guest — for Vite and for the
 * build shell — and both have to read it back the same way, which rules out line breaks and a
 * value that mixes both kinds of quote.
 */
export function envValueIssue(value: string): string | null {
    if (value.length > MAX_VALUE_LENGTH) {
        return 'is longer than 4096 characters';
    }
    if (/[\r\n]/.test(value)) {
        return 'cannot contain line breaks';
    }
    if (value.includes('"') && value.includes("'")) {
        return 'cannot contain both single and double quotes';
    }

    return null;
}

/**
 * Every reason the draft cannot be saved yet, in row order, each naming its variable. An unnamed
 * row is numbered within its own section, which is how the dialog shows it.
 */
export function envDraftIssues(rows: EnvVariableDraft[]): string[] {
    const issues: string[] = [];
    const seen = new Set<string>();
    const position = { variable: 0, secret: 0 };

    for (const row of rows) {
        const kind = row.secret ? 'secret' : 'variable';
        position[kind] += 1;
        const key = row.key.trim();
        const label = key === '' ? `${row.secret ? 'Secret' : 'Variable'} ${position[kind]}` : key;
        const keyIssue = envKeyIssue(key);
        if (keyIssue) {
            issues.push(`${label} ${keyIssue}.`);
            continue;
        }
        if (seen.has(key)) {
            issues.push(`${key} is listed twice.`);
            continue;
        }
        seen.add(key);
        if (row.value === '' && !(row.secret && row.stored)) {
            issues.push(`${key} needs a value.`);
            continue;
        }
        const valueIssue = envValueIssue(row.value);
        if (valueIssue) {
            issues.push(`The value of ${key} ${valueIssue}.`);
        }
    }

    return issues;
}

/** The editor's rows for a stored set: plain variables with their values, secrets blank. */
export function envDraftRows(set: EnvSetSummary | null): {
    variables: EnvVariableDraft[];
    secrets: EnvVariableDraft[];
} {
    return {
        variables: (set?.variables ?? []).map(({ key, value }) => ({
            key,
            value,
            secret: false,
            stored: true,
        })),
        secrets: (set?.secretKeys ?? []).map((key) => ({
            key,
            value: '',
            secret: true,
            stored: true,
        })),
    };
}

/** The rows as the vault takes them: a stored secret left blank sends null, which keeps it. */
export function envDraftVariables(rows: EnvVariableDraft[]): EnvVariableInput[] {
    return rows.map((row) => ({
        key: row.key.trim(),
        value: row.secret && row.stored && row.value === '' ? null : row.value,
        secret: row.secret,
    }));
}

/** What a set holds, for a label: "2 variables, 1 secret", or "empty". */
export function describeEnvSetSize(set: Pick<EnvSetSummary, 'variables' | 'secretKeys'>): string {
    const parts = [
        [set.variables.length, 'variable'],
        [set.secretKeys.length, 'secret'],
    ] as const;

    const described = parts
        .filter(([count]) => count > 0)
        .map(([count, noun]) => `${count} ${noun}${count === 1 ? '' : 's'}`);

    return described.length ? described.join(', ') : 'empty';
}
