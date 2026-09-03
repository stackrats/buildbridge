import { defineConfig } from 'vite-plus';

export default defineConfig({
    staged: {
        '*': 'vp check --fix',
    },
    lint: {
        ignorePatterns: [
            '.agents/**',
            '.claude/**',
            '.codex/**',
            'apps/desktop/src-tauri/**',
            'target/**',
        ],
        options: {
            denyWarnings: true,
            typeAware: true,
        },
    },
    fmt: {
        printWidth: 100,
        tabWidth: 4,
        singleQuote: true,
        semi: true,
        ignorePatterns: [
            '.agents/**',
            '.claude/**',
            '.codex/**',
            '**/*.md',
            '**/*.toml',
            '.mcp.json',
            'apps/desktop/src-tauri/gen/**',
            'opencode.json',
        ],
        sortTailwindcss: {
            entryPoint: 'apps/desktop/src/style.css',
        },
    },
});
