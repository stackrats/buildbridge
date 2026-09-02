# BuildBridge

BuildBridge is a local-first, cross-platform build control plane. The web app coordinates runners and build activity, while the Tauri desktop app pairs a machine and manages its native build capabilities, including the Docker-OSX provider on supported Linux hosts.

The full product goal, platform strategy, security boundaries, golden-path workflow, and delivery phases are documented in [Product Goal and Architecture](docs/PRODUCT_AND_ARCHITECTURE.md).

## Workspace

```text
apps/
  desktop/              Tauri and Vue runner application
crates/
  buildbridge-contract/ Shared runner protocol types
  buildbridge-runner/   Native runner transport and execution
  buildbridge-docker-osx/ Docker-OSX host lifecycle provider
```

Vite+ owns shared JavaScript and Vue tooling at the repository root, and Cargo owns the Rust workspace.

## Setup

Requirements are Rust and the Node.js version managed by Vite+.

```bash
vp install
cd ../..
```

## Development

```bash
pnpm dev

# Web frontend only
pnpm dev:frontend

# Tauri desktop runner
pnpm dev:desktop
```

## Verification

```bash
pnpm check
pnpm test
pnpm build
```

Focused commands are also available as `build:web`, `build:desktop`, `test:web`, and `test:rust`.
