<div align="center">

# BuildBridge

### Build and sign iOS apps locally on a macOS virtual machine from Linux.

BuildBridge creates a persistent macOS build machine on your Linux host with Docker, QEMU, and KVM. Its desktop app handles setup, project sync, Xcode builds, signing, and IPA export. An optional web control plane queues builds and shows their progress, logs, and artifacts.

[Features](#features) · [How it works](#how-it-works) · [Requirements](#requirements) · [Development](#development)

</div>

---

## Features

- Install macOS and Xcode once, then reuse the machine across projects.
- Sync an approved project folder and run unsigned test builds.
- Store signing certificates and provisioning profiles in the operating system's credential vault.
- Create signed archives and verified App Store Connect IPAs.
- Manage multiple local macOS build machines from the Linux desktop app.
- Pair with the web control plane for queued builds, live status, logs, and artifacts.

## How it works

```text
Web control plane ── Reverb + HTTPS ─┐
Approved projects ───────────────────┼─ Tauri desktop runner
Credential vault ────────────────────┘  trusted Linux host
                                        ├─ Docker-OSX lifecycle ─┐
                                        └─ pinned SSH bridge ─────┴─ macOS / Xcode ── archive / IPA
```

The desktop app is the trusted side. It owns the machines, approved project folders, signing credentials, and pinned SSH connection to each guest. The web app only coordinates pairing and builds; it cannot run arbitrary shell commands or access signing secrets.

The desktop app guides each machine through two workflows:

1. **Setup:** Check the Linux host, create the machine, install macOS, configure SSH, and install Xcode.
2. **Build:** Approve and sync a project, test an unsigned build, provision signing credentials, and export the signed archive and IPA.

BuildBridge never collects an Apple Account password or two-factor code. The local macOS login password is asked for once, to install the SSH key into a guest whose fingerprint is already pinned, and is discarded after that one session; the same step can be done by typing commands in the guest Terminal instead. For more detail, see [Product and Architecture](docs/PRODUCT_AND_ARCHITECTURE.md).

## Requirements

For the desktop runner on Linux:

- x86_64 Linux with KVM (`/dev/kvm` readable and writable by your user)
- Docker Engine reachable by your user
- An X11 display for the one-time macOS installer console
- OpenSSH client tools

Docker-OSX is an experimental, self-hosted route. Apple's licensing ties macOS virtualization to Apple hardware; review it before using BuildBridge for production builds.

## Development

This is a monorepo:

```text
apps/desktop    Tauri and Vue desktop runner
crates/         Shared Rust: protocol contract, runner transport, Docker-OSX provider
```

Requirements are Rust, Docker, and the Node.js version managed by Vite+.

```bash
vp install
```

Run the apps:

```bash
pnpm dev:desktop    # Tauri desktop runner
```

The desktop interface can be developed in a plain browser: `vp dev` inside `apps/desktop` serves it against a mock backend with one prepared machine and one fresh machine, no Docker required.

Run the checks:

```bash
pnpm check          # format, lint, type check, cargo check
pnpm test           # Rust tests
vp test --run       # desktop unit tests, inside apps/desktop
pnpm build
```

## License

MIT.
