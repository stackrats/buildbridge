<div align="center">

# BuildBridge

### Build and sign iOS apps locally on a macOS virtual machine from Linux.

BuildBridge creates a persistent macOS build machine on your Linux host with Docker, QEMU, and KVM, run by Docker-OSX or by dockur/macos. Its desktop app handles setup, project sync, Xcode builds, signing, and IPA export. An optional control plane queues builds and shows their progress, logs, and artifacts.

[Features](#features) · [How it works](#how-it-works) · [Requirements](#requirements) · [Development](#development)

</div>

---

## Features

- Install macOS and Xcode once, then reuse the machine across projects.
- Sync an approved project folder and run unsigned test builds.
- Store signing certificates and provisioning profiles in the operating system's credential vault.
- Create signed archives and verified App Store Connect IPAs.
- Manage multiple local macOS build machines from the Linux desktop app.
- Pair with a control plane for queued builds, live status, logs, and artifacts.

## How it works

```text
Control plane ────── Reverb + HTTPS ─┐   Tauri desktop ─┐
Approved projects ───────────────────┼─ BuildBridge engine ◄─ buildbridge CLI
Credential vault ────────────────────┘   trusted Linux host
                                          ├─ Docker-OSX lifecycle ─┐
                                          └─ pinned SSH bridge ─────┴─ macOS / Xcode ── archive / IPA
```

The engine is the trusted side, running inside the desktop app or the `buildbridge` command line on your Linux host. It owns the machines, approved project folders, signing credentials, and pinned SSH connection to each guest. The control plane only coordinates pairing and builds over the protocol in the contract crate; it cannot run arbitrary shell commands or access signing secrets.

The desktop guides each machine through two workflows, and the command line drives the same steps:

1. **Setup:** Check the Linux host, create the machine, install macOS, configure SSH, and install Xcode.
2. **Build:** Approve and sync a project, test an unsigned build, provision signing credentials, and export the signed archive and IPA.

BuildBridge never collects an Apple Account password or two-factor code. The local macOS login password is asked for once, to install the SSH key into a guest whose fingerprint is already pinned, and is discarded after that one session; the same step can be done by typing commands in the guest Terminal instead. For more detail, see [Product and Architecture](docs/PRODUCT_AND_ARCHITECTURE.md).

## Requirements

For the desktop runner on Linux:

- x86_64 Linux with KVM (`/dev/kvm` readable and writable by your user)
- Docker Engine reachable by your user
- An X11 display for the one-time macOS installer console
- OpenSSH client tools
- Room under `~/.local/share` for each machine's macOS disk (a sparse 200 GB image; tens of GB used)

Optional, to run a Debug build on a real iPhone: the phone on USB, polkit (`pkexec`) to install one udev rule that releases iPhones from `usbmuxd`, and a `plugdev` group. Host-side iPhone sync is off while that rule is installed; the desktop can remove it again. This route is experimental.

Docker-OSX is an experimental, self-hosted route. Apple's licensing ties macOS virtualization to Apple hardware; review it before using BuildBridge for production builds.

## Command line

The same engine as the desktop, in a terminal, on the same machines:

```sh
cargo run -p buildbridge-cli -- status
buildbridge machine create "Team Mac" --from-template xcode-26-ready
buildbridge machine start team-mac
buildbridge project approve team-mac /path/to/app
buildbridge project sync team-mac
buildbridge build test team-mac
buildbridge signing attach team-mac dist-kit && buildbridge signing provision team-mac
buildbridge build archive team-mac
buildbridge device attach team-mac 3 9 && buildbridge device run team-mac <udid>
```

Progress goes to stderr as it happens and results to stdout; `--json` makes both machine
readable. A machine the desktop is working on is refused with the reason, and the other way
round. `buildbridge --help` lists every verb.

## Development

This is a monorepo:

```text
apps/desktop    Tauri and Vue desktop, a thin client of the engine
apps/cli        The buildbridge command line, the other client
crates/         Rust: protocol contract, runner transport, Docker-OSX provider, and the engine
```

The control plane a runner pairs with is a separate service, not part of this repository; the two share only the wire protocol defined in `crates/buildbridge-contract`.

Requirements are Rust, Docker, and the Node.js version managed by Vite+.

```bash
vp install
```

Run the apps:

```bash
pnpm dev                                # Tauri desktop
cargo run -p buildbridge-cli -- status  # the command line, on the same machines
```

The desktop interface can be developed in a plain browser: `vp dev` inside `apps/desktop` serves it against a mock backend with one prepared machine and one fresh machine, no Docker required.

Run the checks:

```bash
pnpm check          # format, lint, type check, cargo check
pnpm test           # Rust workspace tests
vp test --run       # desktop unit tests, inside apps/desktop
pnpm types:generate # regenerate the TypeScript contract from the Rust DTOs
pnpm build
```

## License

MIT.
