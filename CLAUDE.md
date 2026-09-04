# BuildBridge workspace

This is a Rust and Vite+ monorepo. `crates/buildbridge-engine` holds everything BuildBridge does; the Tauri application in `apps/desktop` and the `buildbridge` command line in `apps/cli` are thin clients over it, sharing the same configuration and data directories. `crates/buildbridge-contract` defines the versioned wire protocol between runners and the control plane, and `crates/buildbridge-runner` is the runner's client for it. The control plane a runner pairs with is a separate service and not part of this repository. The root `vite.config.ts` owns shared Vite+ lint and formatting policy, while each application owns its build configuration.

`docs/PRODUCT_AND_ARCHITECTURE.md` is the design record and the issue ledger; update it when a durable decision is made or an issue is found. TypeScript types for the backend are generated from the Rust crates with `pnpm types:generate`; never edit `apps/desktop/src/types/generated` by hand.

## Conventions

- Follow the conventions already in the file you are editing; check sibling files for structure and naming before creating a new one.
- Do not change dependencies in any manifest without approval.
- Child processes take a fixed argument vector; user data is never interpolated into a shell string. Secrets travel through the environment or length-framed stdin, never through arguments or logs.
- Only create documentation files when asked. `docs/brand` stays untracked.
- Run `vp check`, the desktop typecheck and tests, and `cargo clippy --workspace` before finishing a change to the code they cover.
