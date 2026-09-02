# BuildBridge workspace

This is a polyglot monorepo. The Tauri application lives in `apps/desktop`, and shared native code lives in `crates`. The root `vite.config.ts` owns shared Vite+ lint and formatting policy, while each application owns its build configuration.
