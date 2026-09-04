# BuildBridge workspace

This is a polyglot monorepo. The Tauri application lives in `apps/desktop`, the `buildbridge` command line in `apps/cli`, and shared native code lives in `crates` — including `crates/buildbridge-engine`, which holds everything BuildBridge does and which both the desktop and the command line drive. The root `vite.config.ts` owns shared Vite+ lint and formatting policy, while each application owns its build configuration.
