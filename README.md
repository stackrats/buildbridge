<div align="center">

<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/buildbridge-logo-dark.svg">
    <img src="docs/images/buildbridge-logo-light.svg" alt="BuildBridge" width="240" height="120">
  </picture>
</h1>

### Build and sign iOS and Android apps locally, from Linux or a Mac.

BuildBridge creates persistent build machines on your Linux host with Docker: a macOS virtual machine under QEMU and KVM, run by Docker-OSX or by dockur/macos, for iOS; and an Android toolchain container, with no virtual machine in it, for Android. On a Mac it can instead use the installed Xcode and your own signing identity directly for iOS, and Docker Desktop for Android. Its desktop app handles setup, project sync, the builds, signing, export and publishing: a verified App Store Connect IPA, or a signed app bundle and APK. An optional self-hosted dashboard queues builds, shows their progress, logs and artifacts, and lets you lend a trusted Mac to someone else.

[Features](#features) · [How it works](#how-it-works) · [Requirements](#requirements) · [Development](#development)

</div>

---

## Features

- Install macOS and Xcode once, then reuse the machine across projects; an Android toolchain prepares itself on its first build; a Mac's own Xcode needs no setup at all.
- Sync an approved Capacitor project folder and run unsigned test builds or debug builds; the debug APK is kept on the host, ready to install.
- Run the debug build on a real iPhone through the machine, or on a real Android phone or emulator through ADB on the host, and inspect it from Safari or Chrome.
- Store Apple certificates and profiles, and Android upload keys, in the operating system's credential vault; create either without a Mac or a keystore to hand; review, copy or export what is stored later.
- Create signed archives and verified App Store Connect IPAs, and signed app bundles and APKs for Google Play, choosing AAB, APK or both.
- Publish from the desktop: upload an IPA with Transporter from a managed macOS machine, save a Google Play internal-testing draft with a service account held in the vault, or follow the guided handoff for TestFlight, the App Store and direct APK distribution.
- Manage several machines of either platform from the desktop app or the command line.
- Pair with a self-hosted dashboard for queued builds, live status, logs and artifacts, and lend a trusted Mac to another person on the same server.

## How it works

```text
Dashboard ─────────── Reverb + HTTPS ─┐   Tauri desktop ─┐
Approved projects ───────────────────┼─ BuildBridge engine ◄─ buildbridge CLI
Credential vault ────────────────────┘   trusted host
                                          ├─ macOS machine lifecycle ─┐
                                          ├─ pinned SSH bridge ────────┴─ macOS / Xcode ── archive / IPA
                                          ├─ docker exec ─────────────── Android toolchain ── bundle / APK
                                          └─ native Xcode ────────────── This Mac ── archive / IPA
```

The engine is the trusted side, running inside the desktop app or the `buildbridge` command line on your host. It owns the machines, approved project folders, signing credentials, and pinned SSH connection to each guest. The dashboard only coordinates pairing, builds and sharing over the protocol in the contract crate; it cannot run arbitrary shell commands or access signing secrets.

The desktop guides each machine through two workflows, and the command line drives the same steps:

1. **Setup:** Check the host and create the machine. For a managed macOS machine, install macOS, configure SSH, and download Xcode from Apple in a window of the app, signed in with your own Apple ID; an Android toolchain has nothing else to set up; **This Mac** reports the Xcode and signing identity it found.
2. **Build:** Approve and sync a project, run the unsigned test build or the debug build, attach signing credentials, and export the signed archive and IPA or the signed app bundle and APK. **Preview** runs the debug build on a real device, and **Publish** hands the retained file to its store.

BuildBridge never collects an Apple Account password or two-factor code. The local macOS login password is asked for once, to install the SSH key into a guest whose fingerprint is already pinned, and is discarded after that one session; the same step can be done by typing commands in the guest Terminal instead. For more detail, see [Product and Architecture](docs/PRODUCT_AND_ARCHITECTURE.md).

## Requirements

For the desktop on Linux:

- Docker Engine reachable by your user
- For a macOS machine: x86_64 Linux with KVM (`/dev/kvm` readable and writable by your user), an X11 display for the one-time macOS installer console (Docker-OSX) or `/dev/net/tun` (dockur/macos), and OpenSSH client tools
- Room under `~/.local/share` for each machine: a sparse 200 GB macOS disk (tens of GB used), or a few GB of Android SDK and Gradle caches

An Android toolchain needs only Docker: no KVM, no display, no ports. To run a debug APK on a real Android device, ADB must be installed on the host.

For the desktop on a Mac: Xcode with the iOS platform, and an Apple Distribution identity and App Store profile already in the login keychain for signed exports; Docker Desktop for Android machines, which run as `linux/amd64` and so need working amd64 emulation on Apple silicon. Native Mac execution is implemented and covered by automated tests, but has not yet been accepted on physical Apple hardware.

Optional, to run a Debug build on a real iPhone from a managed macOS machine: the phone on USB, polkit (`pkexec`) to install one udev rule that releases iPhones from `usbmuxd`, and a `plugdev` group. Host-side iPhone sync is off while that rule is installed; the desktop can remove it again. This route is experimental.

Docker-OSX is an experimental, self-hosted route. Apple's licensing ties macOS virtualization to Apple hardware; review it before using BuildBridge for production builds.

## Command line

The same engine as the desktop, in a terminal, on the same machines:

```sh
cargo run -p buildbridge-cli -- status
buildbridge machine create "Team Mac" --from-template xcode-26-ready
buildbridge machine start team-mac
buildbridge machine screen team-mac --open     # dockur/macos serves the screen as a web page
buildbridge project approve team-mac /path/to/app
buildbridge project sync team-mac
buildbridge build test team-mac --target device_sdk
buildbridge signing attach team-mac dist-kit && buildbridge signing provision team-mac
buildbridge build archive team-mac
buildbridge device attach team-mac 3 9 && buildbridge device run team-mac <udid>

buildbridge machine create "Android builder" --platform android
buildbridge machine start android-builder
buildbridge project approve android-builder /path/to/app && buildbridge project sync android-builder
buildbridge build test android-builder --allow-http   # HTTP APIs in this debug APK only
buildbridge signing keystore dist-kit --password-stdin < keystore-password.txt
buildbridge signing attach android-builder dist-kit
buildbridge build release android-builder --outputs both   # or aab, or apk
```

Progress goes to stderr as it happens and results to stdout; `--json` makes both machine
readable. A machine the desktop is working on is refused with the reason, and the other way
round. `buildbridge --help` lists every verb.

## Development

This is a monorepo:

```text
apps/desktop    Tauri and Vue desktop, a thin client of the engine
apps/cli        The buildbridge command line, the other client
crates/         Rust: protocol contract, runner transport, the machines (Docker-OSX, dockur/macos, the Android toolchain, and the native Mac), and the engine
```

The dashboard a runner pairs with is a separate service, not part of this repository; the two share only the wire protocol defined in `crates/buildbridge-contract`.

Requirements are Rust, Docker, and the Node.js version managed by Vite+.

```bash
vp install
```

Run the apps:

```bash
vp run dev                              # Tauri desktop
cargo run -p buildbridge-cli -- status  # the command line, on the same machines
```

The desktop interface can be developed in a plain browser: `vp dev` inside `apps/desktop` serves it against a mock backend with one prepared macOS machine, one fresh machine and one Android builder, no Docker required.

Run the checks:

```bash
vp check                 # format and lint (vp fmt, vp lint on their own); --fix applies
vp run -r typecheck      # desktop type check
vp test --run            # desktop unit tests, inside apps/desktop
cargo test --workspace   # Rust workspace tests
cargo clippy --workspace
vp run types:generate    # regenerate the TypeScript contract from the Rust DTOs
cargo test -p buildbridge-engine --test provider_boot -- --ignored   # boots a throwaway machine per provider; run before changing an image digest
vp run -r build          # build the desktop front end
```

GitHub Actions runs the same checks on every push and pull request, with a macOS job compiling the workspace for the native Mac paths. Pushing a tag named after the workspace version, `v0.1.0`, builds the Linux and macOS bundles and the command line and attaches them to a draft release.

## License

MIT.
