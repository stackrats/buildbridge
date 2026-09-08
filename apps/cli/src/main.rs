//! `buildbridge` — buildbridge from the terminal. The same engine the desktop drives, in this
//! process, on the same directories, so both see the same machines; the engine's per-machine
//! lock keeps the two from running one machine at once. Progress goes to stderr as it happens,
//! results to stdout, and `--json` makes both machine-readable.

use std::future::Future;
use std::io::{BufRead, IsTerminal};
use std::sync::{Arc, Mutex};

use buildbridge_engine::{Engine, EngineDeps, EventSink};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The identifier the desktop stores everything under; the command line must match it.
const APP_IDENTIFIER: &str = "dev.buildbridge.desktop";

#[derive(Parser)]
#[command(
    name = "buildbridge",
    version,
    about = "Build and sign iOS and Android apps on managed machines, from the terminal"
)]
struct Cli {
    /// Print results and progress as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The host's readiness and every machine's state.
    Status,
    /// Machines: create, configure, start, stop, inspect, discard, delete.
    #[command(subcommand)]
    Machine(MachineCommand),
    /// Templates: prepared machines saved once and cloned in seconds.
    #[command(subcommand)]
    Template(TemplateCommand),
    /// The guest's access: pin its identity and install the buildbridge key.
    #[command(subcommand)]
    Guest(GuestCommand),
    /// Xcode inside the machine: import the archive and activate it.
    #[command(subcommand)]
    Xcode(XcodeCommand),
    /// The project: approve a folder and synchronize it into the machine.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Builds: the unsigned test build and the signed archive.
    #[command(subcommand)]
    Build(BuildCommand),
    /// Signing credentials and provisioning.
    #[command(subcommand)]
    Signing(SigningCommand),
    /// A phone plugged into this host: attach, pair, prepare, run.
    #[command(subcommand)]
    Device(DeviceCommand),
    /// Environments stored on this host.
    #[command(subcommand)]
    Env(EnvCommand),
    /// The control plane this host is paired with. Hidden while remote builds are off, which
    /// is where a host starts: every command here refuses until `settings set
    /// --remote-builds on`.
    #[command(subcommand, hide = true)]
    Runner(RunnerCommand),
    /// What each running machine costs right now: cores and memory, as Docker measures them.
    Usage,
    /// This host's preferences, shared with the desktop.
    #[command(subcommand)]
    Settings(SettingsCommand),
}

#[derive(Subcommand)]
enum SettingsCommand {
    Show,
    Set {
        /// The browser that opens pages outside buildbridge: a command name, a path, or on
        /// macOS an application bundle such as Firefox.app. Empty returns to the desktop's
        /// default browser.
        #[arg(long, value_name = "COMMAND")]
        browser: Option<String>,
        /// Whether this host offers remote builds: pairing with a buildbridge server, claiming
        /// queued work, and sharing a Mac. Off until the server it would pair with exists.
        #[arg(long, value_name = "ON|OFF", value_parser = parse_on_off)]
        remote_builds: Option<bool>,
    },
}

/// `on` and `off` as a person types them, with true/false accepted for scripts.
fn parse_on_off(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        other => Err(format!("Expected on or off, not {other}.")),
    }
}

#[derive(Subcommand)]
enum MachineCommand {
    List,
    Create {
        name: String,
        /// The macOS release to install: tahoe, sequoia, sonoma or ventura. Omitted, it is the
        /// provider's recommendation, tahoe on docker-osx and sequoia on dockur-macos.
        #[arg(long)]
        macos: Option<String>,
        #[arg(long, default_value_t = 8)]
        memory: u32,
        #[arg(long, default_value_t = 4)]
        cores: u32,
        #[arg(long)]
        port: Option<u16>,
        /// Clone a saved template instead of installing macOS.
        #[arg(long)]
        from_template: Option<String>,
        /// What the machine builds for: ios (a macOS machine) or android (a toolchain
        /// container). Picks that platform's recommended provider unless --provider names one.
        #[arg(long, value_enum)]
        platform: Option<PlatformArg>,
        /// Which provider, within the platform. docker-osx runs macOS and shows its screen in a
        /// window on this host's display; dockur-macos runs macOS and serves the screen as a web
        /// page on the port after the SSH port, needing /dev/net/tun and the machine's memory
        /// free; the two build and clone the same. android-toolchain is a JDK container with the
        /// Android SDK, Gradle and Node prepared inside it: no macOS, no KVM, no ports, and
        /// memory and cores are limits on its builds.
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
    },
    /// Change a machine's profile, moving only what is named here. The name changes at any
    /// time; memory, cores and the SSH port need the machine stopped, and saving one of them
    /// removes its container so the next start creates it again with the new profile — the
    /// macOS disk and a toolchain's home are on this host and stay. The installer can only be
    /// chosen while the disk is still empty.
    Configure {
        machine: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        memory: Option<u32>,
        #[arg(long)]
        cores: Option<u32>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        macos: Option<String>,
    },
    Start {
        machine: String,
    },
    Stop {
        machine: String,
    },
    Show {
        machine: String,
    },
    /// Print the address of the machine's screen, for providers that serve it as a web page.
    Screen {
        machine: String,
        /// Open it in this host's default browser as well.
        #[arg(long)]
        open: bool,
    },
    /// Remove the container and its macOS disk; the machine profile stays.
    Discard {
        machine: String,
        #[command(flatten)]
        confirm: Confirm,
    },
    Delete {
        machine: String,
        #[command(flatten)]
        confirm: Confirm,
    },
    /// Stop whatever this machine is doing.
    Cancel {
        machine: String,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    List,
    /// Shut the machine down and save its disk as a template.
    Save {
        machine: String,
        name: String,
    },
    Delete {
        template: String,
        #[command(flatten)]
        confirm: Confirm,
    },
}

#[derive(Subcommand)]
enum GuestCommand {
    /// Pin the guest's SSH identity, by the fingerprint the machine reports.
    Pin {
        machine: String,
        fingerprint: String,
    },
    /// Install the buildbridge key; the macOS password is read from stdin.
    Authorize {
        machine: String,
        username: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Adopt a clone's identity and key from its template.
    Adopt { machine: String },
}

#[derive(Subcommand)]
enum XcodeCommand {
    Import {
        machine: String,
        path: String,
    },
    /// Activate Xcode; with --password-stdin over SSH, else in the guest Terminal.
    Activate {
        machine: String,
        #[arg(long)]
        password_stdin: bool,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Approve a project folder: its iOS project for a macOS machine, its Android project for
    /// an Android one. Capacitor, Cordova, React Native, Expo, Flutter and plain Xcode or
    /// Gradle projects are recognised from their files.
    Approve {
        machine: String,
        path: String,
        /// The Xcode scheme to build when the project offers several (macOS machines).
        #[arg(long)]
        scheme: Option<String>,
        /// The Gradle application module to build when the project has several, such as
        /// `:app` (Android machines).
        #[arg(long)]
        module: Option<String>,
    },
    Sync {
        machine: String,
    },
    /// Copy the Podfile.lock the guest resolved into the project, lifting the archive's block.
    AdoptLock {
        machine: String,
    },
}

#[derive(Subcommand)]
enum BuildCommand {
    /// The unsigned test build on a macOS machine, or the debug build on an Android one.
    Test {
        machine: String,
        /// macOS machines only: device_sdk or simulator.
        #[arg(long, default_value = "device_sdk")]
        target: String,
        /// Allow HTTP API requests in this Android debug APK only.
        #[arg(long)]
        allow_http: bool,
        /// Set the version (marketing version or version name), in the project and this build.
        #[arg(long)]
        version: Option<String>,
        /// Set the build number (or version code), in the project and this build.
        #[arg(long)]
        build: Option<String>,
    },
    /// The signed archive and IPA, on a macOS machine.
    Archive {
        machine: String,
        #[arg(long)]
        env: Option<String>,
        /// Set the marketing version, in the project and in this archive.
        #[arg(long)]
        version: Option<String>,
        /// Set the build number, in the project and in this archive.
        #[arg(long)]
        build: Option<String>,
    },
    /// The selected signed Android release artifacts (both by default).
    Release {
        machine: String,
        #[arg(long)]
        env: Option<String>,
        #[arg(long, default_value = "both", value_parser = ["both", "aab", "apk"])]
        outputs: String,
        /// Set the version name, in the project and in this release.
        #[arg(long)]
        version: Option<String>,
        /// Set the version code, in the project and in this release.
        #[arg(long)]
        build: Option<String>,
    },
    /// Ask the store which build numbers it already holds for this app.
    Check {
        machine: String,
        /// The version to ask about; the project's own by default.
        #[arg(long)]
        version: Option<String>,
    },
}

#[derive(Subcommand)]
enum SigningCommand {
    Kits,
    Attach {
        machine: String,
        kit: String,
    },
    Provision {
        machine: String,
    },
    /// Create an Android upload key for stored signing credentials; the keystore password is read from stdin and is
    /// yours to keep, because neither Google Play nor buildbridge can recover it.
    Keystore {
        kit: String,
        /// The alias of the key inside the keystore.
        #[arg(long, default_value = "upload")]
        alias: String,
        /// The name in the certificate; the credentials' name when omitted.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        password_stdin: bool,
    },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// Phones the guest sees, after asking it; on an Android machine, what host ADB sees.
    List {
        machine: String,
    },
    /// Hand a plugged-in phone to the machine by host bus and port.
    Attach {
        machine: String,
        bus: u32,
        port: String,
    },
    Detach {
        machine: String,
    },
    Pair {
        machine: String,
        udid: String,
    },
    /// Register the phone at Apple and provision a development identity for it.
    Prepare {
        machine: String,
        udid: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        confirm: Confirm,
    },
    /// Build, install and launch the Debug build; its console streams until Ctrl-C. On an
    /// Android machine, install and launch the retained debug APK on the ADB device with this
    /// serial; its log streams until Ctrl-C.
    Run {
        machine: String,
        /// The phone: its UDID on a macOS machine, its ADB serial on an Android one.
        udid: String,
        /// macOS machines only; an Android debug APK carries the environment it was built with.
        #[arg(long)]
        env: Option<String>,
        /// macOS machines only: set the marketing version in the project and this build.
        #[arg(long)]
        version: Option<String>,
        /// macOS machines only: set the build number in the project and this build.
        #[arg(long)]
        build: Option<String>,
    },
}

#[derive(Subcommand)]
enum EnvCommand {
    List,
    /// Change values in a stored environment: KEY=VALUE pairs; every other variable keeps its value.
    Set {
        set: String,
        #[arg(required = true, value_name = "KEY=VALUE")]
        variables: Vec<String>,
    },
    Attach {
        machine: String,
        set: String,
    },
}

#[derive(Subcommand)]
enum RunnerCommand {
    Status,
    /// Claim and run one waiting build from the control plane.
    Check,
}

#[derive(Args)]
struct Confirm {
    /// Confirm the destructive step without a prompt.
    #[arg(long)]
    yes: bool,
}

impl Confirm {
    fn require(&self, what: &str) -> Result<(), String> {
        if self.yes {
            Ok(())
        } else {
            Err(format!("{what} needs --yes."))
        }
    }
}

/// Progress on stderr: one line per phase change, log lines indented, the same payloads the
/// desktop's window receives.
struct Printer {
    json: bool,
    last: Mutex<String>,
}

impl Printer {
    fn line(&self, text: String) {
        if let Ok(mut last) = self.last.lock() {
            if *last == text {
                return;
            }
            *last = text.clone();
        }
        eprintln!("{text}");
    }
}

impl EventSink for Printer {
    fn emit(&self, event: &str, payload: Value) {
        if event == buildbridge_engine::MACHINE_CHANGED_EVENT {
            return;
        }
        if self.json {
            eprintln!("{}", json!({ "event": event, "payload": payload }));
            return;
        }
        if let Some(lines) = payload.get("logLines").and_then(Value::as_array) {
            for line in lines.iter().filter_map(Value::as_str) {
                eprintln!("  {line}");
            }
        }
        if let Some(phase) = payload.get("phase").and_then(Value::as_str) {
            let percent = payload
                .get("percent")
                .and_then(Value::as_u64)
                .map(|value| format!(" {value}%"))
                .unwrap_or_default();
            let detail = payload
                .get("detail")
                .and_then(Value::as_str)
                .map(|value| format!(" · {value}"))
                .unwrap_or_default();
            self.line(format!("{}{percent}{detail}", phase.replace('_', " ")));
        }
        if let Some(line) = payload.get("logLine").and_then(Value::as_str) {
            self.line(format!("  {line}"));
        }
    }
}

fn engine(json: bool) -> Result<Engine, String> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| "no configuration directory for this user".to_string())?
        .join(APP_IDENTIFIER);
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| "no data directory for this user".to_string())?
        .join(APP_IDENTIFIER);
    Ok(Engine::new(EngineDeps {
        config_dir,
        data_dir,
        events: Arc::new(Printer {
            json,
            last: Mutex::new(String::new()),
        }),
    }))
}

/// The inputs the engine takes are the desktop's JSON shapes; built here the same way.
fn input<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// The stored profile with only the named fields moved. `configure_machine` is handed a whole
/// profile and replaces the stored one with it, so everything the command leaves out has to
/// come back exactly as it is stored, and naming nothing is a mistake rather than a no-op.
fn configured_profile(
    stored: &Value,
    name: Option<String>,
    memory: Option<u32>,
    cores: Option<u32>,
    port: Option<u16>,
    macos: Option<String>,
) -> Result<Value, String> {
    let changes: Vec<(&str, Value)> = [
        name.map(|value| ("name", Value::from(value))),
        memory.map(|value| ("memoryGib", Value::from(value))),
        cores.map(|value| ("cpuCores", Value::from(value))),
        port.map(|value| ("sshPort", Value::from(value))),
        macos.map(|value| ("macosRelease", Value::from(value))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if changes.is_empty() {
        return Err(
            "Name what to change: --name, --memory, --cores, --port or --macos.".to_string(),
        );
    }
    let mut profile = stored.clone();
    for (key, value) in changes {
        profile[key] = value;
    }

    Ok(profile)
}

/// The version a build was asked to carry: either flag alone keeps the other half as the
/// project declares it; neither leaves the project's version alone.
fn version_input(
    version: Option<String>,
    build: Option<String>,
) -> Result<Option<buildbridge_engine::ProjectVersionInput>, String> {
    input(version_json(version, build))
}

/// The same request as JSON, for inputs that carry it as one field.
fn version_json(version: Option<String>, build: Option<String>) -> Value {
    if version.is_none() && build.is_none() {
        return Value::Null;
    }
    json!({ "version": version, "build": build })
}

/// `3.2.0 (15)` from a view's project version, or nothing when the project declares none.
fn version_text(value: &Value) -> String {
    match value["version"].as_str() {
        Some(version) => format!("{version} ({})", text(&value["build"])),
        None => String::new(),
    }
}

/// Runs one machine operation; Ctrl-C asks the engine to stop it and waits for the answer,
/// so processes in the guest are killed and the machine's lock is released.
async fn on_machine<T, F>(engine: &Engine, machine: &str, work: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    tokio::pin!(work);
    let mut cancelled = false;
    loop {
        tokio::select! {
            result = &mut work => return result,
            signal = tokio::signal::ctrl_c(), if !cancelled => {
                let _ = signal;
                cancelled = true;
                eprintln!("Stopping…");
                let _ = buildbridge_engine::cancel_machine_operation(engine, machine.to_string()).await;
            }
        }
    }
}

/// The complete save payload for one stored environment with some values changed: the engine removes
/// any key it is not handed, so every stored variable is listed, a missing value keeping the one
/// in the vault and secrets staying secret.
async fn env_set_update(
    engine: &Engine,
    set_id: &str,
    changes: &[(String, String)],
) -> Result<Value, String> {
    let sets = serde_json::to_value(buildbridge_engine::list_env_sets(engine).await?)
        .map_err(|error| error.to_string())?;
    let stored = sets
        .as_array()
        .into_iter()
        .flatten()
        .find(|set| set["id"] == set_id)
        .ok_or_else(|| format!("No environment is stored as {set_id}; `env list` names them."))?;
    let changed = |key: &str| {
        changes
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    };
    let mut variables = stored["variables"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|variable| {
            let key = text(&variable["key"]);
            json!({ "key": key, "value": changed(&key), "secret": false })
        })
        .chain(
            stored["secretKeys"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|key| {
                    let key = text(key);
                    json!({ "key": key, "value": changed(&key), "secret": true })
                }),
        )
        .collect::<Vec<_>>();
    for (key, value) in changes {
        if !variables
            .iter()
            .any(|variable| variable["key"] == key.as_str())
        {
            variables.push(json!({ "key": key, "value": value, "secret": false }));
        }
    }
    Ok(json!({ "setId": set_id, "name": stored["name"], "variables": variables }))
}

fn password_from_stdin(enabled: bool) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!(
            "Password (not echoed if piped; use `--password-stdin < file` to keep it out of history):"
        );
    }
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let password = line.trim_end_matches(['\n', '\r']).to_string();
    if password.is_empty() {
        return Err("no password was read from stdin".to_string());
    }
    Ok(Some(password))
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "yes" } else { "no" }.to_string(),
        other => other.to_string(),
    }
}

/// A phone's network beside this computer's, as the listing read it; blank when it was not
/// asked (an emulator, or a device that is not ready).
fn device_network_text(network: &Value) -> String {
    if network.is_null() {
        return "—".to_string();
    }
    let address = network["address"].as_str().unwrap_or("no Wi-Fi");
    if network["onHostNetwork"] == true {
        format!("{address} · this computer's")
    } else {
        format!("{address} · not this computer's")
    }
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row.get(index).map_or(0, |cell| cell.chars().count()))
                .max()
                .unwrap_or(0)
                .max(header.chars().count())
        })
        .collect::<Vec<_>>();
    let render = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(index, cell)| format!("{cell:<width$}", width = widths[index]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!(
        "{}",
        render(headers.iter().map(|h| h.to_string()).collect())
    );
    for row in rows {
        println!("{}", render(row));
    }
}

/// Prints a result: JSON when asked, else the renderer's summary.
fn report<T: serde::Serialize>(
    json: bool,
    result: &T,
    human: impl FnOnce(&Value),
) -> Result<(), String> {
    let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
        );
    } else {
        human(&value);
    }
    Ok(())
}

fn machine_rows(list: &Value) -> Vec<Vec<String>> {
    list["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|machine| {
            vec![
                text(&machine["id"]),
                text(&machine["config"]["name"]),
                text(&machine["state"]),
                text(&machine["busyOperation"]).replace('_', " "),
                text(&machine["workspaceName"]),
                text(&machine["signingKitName"]),
                text(&machine["templateName"]),
            ]
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PlatformArg {
    Ios,
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ProviderArg {
    DockerOsx,
    DockurMacos,
    AndroidToolchain,
}

impl ProviderArg {
    fn value(self) -> &'static str {
        match self {
            Self::DockerOsx => "docker_osx",
            Self::DockurMacos => "dockur_macos",
            Self::AndroidToolchain => "android_toolchain",
        }
    }

    fn platform(self) -> PlatformArg {
        match self {
            Self::DockerOsx | Self::DockurMacos => PlatformArg::Ios,
            Self::AndroidToolchain => PlatformArg::Android,
        }
    }

    /// The release a machine installs when none is named, the same one the desktop's profile
    /// form moves to when this provider is chosen: dockur/macos's own authors do not recommend
    /// Tahoe on it yet, and the first Tahoe install there hung in its second stage. The
    /// toolchain container has no macOS in it; the value is carried but never read.
    fn recommended_release(self) -> &'static str {
        match self {
            Self::DockerOsx => "tahoe",
            Self::DockurMacos | Self::AndroidToolchain => "sequoia",
        }
    }

    /// The provider a machine gets from the two flags: the named one, else the platform's
    /// recommended one, else the original. The two must agree when both are given.
    fn resolve(
        platform: Option<PlatformArg>,
        provider: Option<ProviderArg>,
    ) -> Result<Self, String> {
        match (platform, provider) {
            (Some(platform), Some(provider)) if provider.platform() != platform => Err(format!(
                "--provider {} builds {:?}, not {:?}; drop one of the two flags",
                provider.value(),
                provider.platform(),
                platform
            )),
            (_, Some(provider)) => Ok(provider),
            (Some(PlatformArg::Android), None) => Ok(Self::AndroidToolchain),
            (Some(PlatformArg::Ios), None) | (None, None) => Ok(Self::DockerOsx),
        }
    }
}

/// One line naming what was detected: the kind, the package manager, the Xcode container and
/// scheme, the Gradle root and module, with the other schemes or modules the project offers.
fn describe_layout(layout: &serde_json::Value) -> String {
    // The engine names the kinds; the command line does not keep a second list of them.
    let kind = serde_json::from_value::<buildbridge_engine::ProjectKind>(layout["kind"].clone())
        .map(|kind| kind.label().to_string())
        .unwrap_or_default();
    let mut parts = vec![kind];
    if let Some(manager) = layout["packageManager"].as_str() {
        parts.push(manager.to_string());
    }
    if let Some(ios) = layout["ios"].as_object() {
        let scheme = ios["scheme"].as_str().unwrap_or_default();
        let others: Vec<&str> = ios["schemes"]
            .as_array()
            .map(|schemes| {
                schemes
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|name| *name != scheme)
                    .collect()
            })
            .unwrap_or_default();
        parts.push(format!(
            "{} · scheme {scheme}{}",
            ios["container"].as_str().unwrap_or_default(),
            if others.is_empty() {
                String::new()
            } else {
                format!(" (also {}; choose with --scheme)", others.join(", "))
            }
        ));
    }
    if let Some(android) = layout["android"].as_object() {
        let module = android["modulePath"].as_str().unwrap_or_default();
        let others: Vec<&str> = android["modules"]
            .as_array()
            .map(|modules| {
                modules
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|name| *name != module)
                    .collect()
            })
            .unwrap_or_default();
        let root = android["root"].as_str().unwrap_or_default();
        parts.push(format!(
            "{} · module {module}{}",
            if root.is_empty() {
                "Gradle at the root"
            } else {
                root
            },
            if others.is_empty() {
                String::new()
            } else {
                format!(" (also {}; choose with --module)", others.join(", "))
            }
        ));
    }
    parts.join(" · ")
}

/// Whether a registered machine is an Android toolchain, from the machine list rather than a
/// full probe of the machine.
async fn machine_is_android(engine: &Engine, machine: &str) -> Result<bool, String> {
    let list = serde_json::to_value(buildbridge_engine::list_machines(engine).await?)
        .map_err(|error| error.to_string())?;
    let found = list["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["id"].as_str() == Some(machine))
        .ok_or_else(|| "This machine is no longer registered.".to_string())?;
    Ok(found["platform"].as_str() == Some("android"))
}

fn print_machine_list(list: &Value) {
    let host = &list["host"];
    if host["ready"].as_bool() == Some(true) {
        println!("host: ready");
    } else {
        println!("host: not ready");
        for issue in host["issues"].as_array().into_iter().flatten() {
            println!("  {}", text(issue));
        }
    }
    let rows = machine_rows(list);
    if rows.is_empty() {
        println!("No machines yet. `buildbridge machine create <name>` makes one.");
    } else {
        table(
            &[
                "id",
                "name",
                "state",
                "busy",
                "project",
                "credentials",
                "template",
            ],
            rows,
        );
    }
}

fn print_machine(view: &Value) {
    if view["profile"]["provider"].as_str() == Some("android_toolchain") {
        let android = &view["android"];
        let last_build = &android["workspace"]["lastBuild"];
        let rows = vec![
            ("machine", text(&view["machineId"])),
            ("name", text(&view["profile"]["name"])),
            ("state", text(&view["runtime"]["state"])),
            ("busy", text(&view["busyOperation"]).replace('_', " ")),
            ("provider", "android toolchain".to_string()),
            ("project", text(&android["workspace"]["name"])),
            ("app id", text(&android["workspace"]["applicationId"])),
            ("version", version_text(&view["projectVersion"])),
            (
                "last build",
                match last_build["versionName"].as_str() {
                    Some(version) => format!("{version} ({})", text(&last_build["versionCode"])),
                    None => String::new(),
                },
            ),
            ("jdk", text(&last_build["toolchain"]["jdkVersion"])),
            ("credentials", text(&view["signingKit"]["name"])),
            ("signing", text(&view["signingHealth"])),
            ("bundle", text(&android["release"]["aab"]["path"])),
            ("apk", text(&android["release"]["apk"]["path"])),
        ];
        for (label, value) in rows {
            println!("{label:<12} {value}");
        }
        return;
    }
    let diagnostics = &view["guest"]["diagnostics"];
    let rows = vec![
        ("machine", text(&view["machineId"])),
        ("name", text(&view["profile"]["name"])),
        ("state", text(&view["runtime"]["state"])),
        ("busy", text(&view["busyOperation"]).replace('_', " ")),
        ("template", text(&view["template"]["name"])),
        (
            "provider",
            text(&view["profile"]["provider"]).replace('_', " "),
        ),
        (
            "ssh",
            format!("127.0.0.1:{}", text(&view["profile"]["sshPort"])),
        ),
        ("screen", text(&view["displayUrl"])),
        ("identity", text(&view["guest"]["ssh"]["trust"])),
        ("fingerprint", text(&view["guest"]["ssh"]["fingerprint"])),
        ("user", text(&view["guest"]["username"])),
        ("macOS", text(&diagnostics["macosVersion"])),
        ("Xcode", text(&diagnostics["xcodeVersion"])),
        ("project", text(&view["appleWorkspace"]["name"])),
        ("bundle", text(&view["appleWorkspace"]["bundleIdentifier"])),
        ("version", version_text(&view["projectVersion"])),
        ("credentials", text(&view["signingKit"]["name"])),
        ("signing", text(&view["signingHealth"])),
        ("archive", text(&view["archive"]["ipa"]["path"])),
    ];
    for (label, value) in rows {
        println!("{label:<12} {value}");
    }
}

fn print_settings(settings: &Value) {
    println!(
        "browser: {}",
        settings["browser"]
            .as_str()
            .unwrap_or("the desktop's default")
    );
    let remote_builds = settings["remoteBuilds"].as_bool().unwrap_or(false);
    println!(
        "remote builds: {}",
        if remote_builds { "on" } else { "off" }
    );
    if remote_builds {
        println!("  buildbridge runner status · buildbridge runner check");
    }
}

/// Bytes as a person reads memory: binary units, whole numbers above ten.
fn memory_text(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        let value = bytes / GIB;
        if value >= 10.0 {
            format!("{value:.0} GiB")
        } else {
            format!("{value:.1} GiB")
        }
    } else {
        format!("{:.0} MiB", bytes / MIB)
    }
}

fn print_usage(sample: &Value) {
    let host_cores = sample["hostCores"].as_u64().unwrap_or(0);
    let host_memory = sample["hostMemoryBytes"].as_u64().unwrap_or(0);
    let rows = sample["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|machine| {
            vec![
                text(&machine["machineId"]),
                format!("{:.2}", machine["cpuCores"].as_f64().unwrap_or(0.0)),
                memory_text(machine["memoryBytes"].as_u64().unwrap_or(0)),
                if machine["memoryLimited"] == true {
                    memory_text(machine["memoryLimitBytes"].as_u64().unwrap_or(0))
                } else {
                    "—".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        println!("No machine is running.");
    } else {
        table(&["machine", "cores", "memory", "limit"], rows);
    }
    let mut host = Vec::new();
    if host_cores > 0 {
        host.push(format!("{host_cores} cores"));
    }
    if host_memory > 0 {
        host.push(memory_text(host_memory));
    }
    if !host.is_empty() {
        println!("host: {}", host.join(" · "));
    }
}

fn print_done(view: &Value) {
    println!(
        "{} is {}",
        text(&view["profile"]["name"]),
        text(&view["runtime"]["state"])
    );
}

async fn run(cli: Cli) -> Result<(), String> {
    use buildbridge_engine as e;
    let json = cli.json;
    let engine = engine(json)?;
    let engine = &engine;
    match cli.command {
        Command::Status | Command::Machine(MachineCommand::List) => {
            report(json, &e::list_machines(engine).await?, print_machine_list)
        }
        Command::Machine(MachineCommand::Create {
            name,
            macos,
            memory,
            cores,
            port,
            from_template,
            platform,
            provider,
        }) => {
            let provider = ProviderArg::resolve(platform, provider)?;
            let macos = macos.unwrap_or_else(|| provider.recommended_release().to_string());
            let used = serde_json::to_value(e::list_machines(engine).await?)
                .map_err(|error| error.to_string())?;
            let port = port.unwrap_or_else(|| {
                let taken = used["machines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|machine| machine["config"]["sshPort"].as_u64())
                    .collect::<Vec<_>>();
                let mut candidate: u64 = 50922;
                while taken.contains(&candidate) {
                    candidate += 1;
                }
                candidate as u16
            });
            let profile = input(json!({
                "name": name,
                "macosRelease": macos,
                "memoryGib": memory,
                "cpuCores": cores,
                "sshPort": port,
                "provider": provider.value(),
            }))?;
            report(
                json,
                &e::create_machine(engine, profile, from_template).await?,
                |list| {
                    print_machine_list(list);
                    println!("Created. `buildbridge machine start <id>` starts it.");
                },
            )
        }
        Command::Machine(MachineCommand::Configure {
            machine,
            name,
            memory,
            cores,
            port,
            macos,
        }) => {
            let stored = serde_json::to_value(e::get_machine(engine, machine.clone()).await?)
                .map_err(|error| error.to_string())?;
            // Whether the machine has a container to lose: a hardware change removes it, and
            // the line below says so rather than leaving a machine that was there and is now
            // missing unexplained.
            let had_container = !matches!(
                stored["runtime"]["state"].as_str(),
                Some("missing") | Some("unavailable") | None
            );
            let profile = configured_profile(&stored["profile"], name, memory, cores, port, macos)?;
            report(
                json,
                &e::configure_machine(engine, machine, input(profile)?).await?,
                |view| {
                    print_machine(view);
                    println!(
                        "{:<12} {} GiB · {} cores",
                        "hardware",
                        text(&view["profile"]["memoryGib"]),
                        text(&view["profile"]["cpuCores"])
                    );
                    if had_container && view["runtime"]["state"].as_str() == Some("missing") {
                        println!(
                            "The container was removed; starting the machine creates it again with this profile."
                        );
                    }
                },
            )
        }
        Command::Machine(MachineCommand::Start { machine }) => report(
            json,
            &on_machine(engine, &machine, e::launch_machine(engine, machine.clone())).await?,
            print_done,
        ),
        Command::Machine(MachineCommand::Stop { machine }) => report(
            json,
            &on_machine(engine, &machine, e::stop_machine(engine, machine.clone())).await?,
            print_done,
        ),
        Command::Machine(MachineCommand::Show { machine }) => {
            report(json, &e::get_machine(engine, machine).await?, print_machine)
        }
        Command::Machine(MachineCommand::Screen { machine, open }) => {
            let view = serde_json::to_value(e::get_machine(engine, machine).await?)
                .map_err(|error| error.to_string())?;
            let Some(url) = view["displayUrl"].as_str().map(str::to_string) else {
                return Err(
                    "This machine's provider shows its screen in a window on this host's display, not at an address."
                        .to_string(),
                );
            };
            if view["runtime"]["state"].as_str() != Some("running") {
                return Err(
                    "Start the machine first; its screen is served while it runs.".to_string(),
                );
            }
            if json {
                println!("{}", serde_json::json!({ "displayUrl": url }));
            } else {
                println!("{url}");
            }
            if open {
                e::open_url(engine, url).await?;
            }
            Ok(())
        }
        Command::Machine(MachineCommand::Discard { machine, confirm }) => {
            confirm.require("Discarding the container and its macOS disk")?;
            report(
                json,
                &e::discard_machine_container(
                    engine,
                    machine,
                    input(json!({ "confirmed": true }))?,
                )
                .await?,
                print_done,
            )
        }
        Command::Machine(MachineCommand::Delete { machine, confirm }) => {
            confirm.require("Deleting a machine")?;
            report(
                json,
                &e::delete_machine(engine, machine, input(json!({ "confirmed": true }))?).await?,
                print_machine_list,
            )
        }
        Command::Machine(MachineCommand::Cancel { machine }) => {
            e::cancel_machine_operation(engine, machine).await?;
            println!("Stop requested.");
            Ok(())
        }
        Command::Template(TemplateCommand::List) => report(
            json,
            &e::list_machine_templates(engine).await?,
            |templates| {
                let rows = templates
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|template| {
                        vec![
                            text(&template["id"]),
                            text(&template["name"]),
                            text(&template["macosVersion"]),
                            text(&template["xcodeVersion"]),
                            format!(
                                "{:.1} GiB",
                                template["sizeBytes"].as_f64().unwrap_or(0.0) / 1_073_741_824.0
                            ),
                            template["machineNames"]
                                .as_array()
                                .map(|names| names.iter().map(text).collect::<Vec<_>>().join(", "))
                                .unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    println!(
                        "No templates. `buildbridge template save <machine> <name>` makes one."
                    );
                } else {
                    table(&["id", "name", "macOS", "Xcode", "size", "clones"], rows);
                }
            },
        ),
        Command::Template(TemplateCommand::Save { machine, name }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::save_machine_template(
                    engine,
                    machine.clone(),
                    input(json!({ "name": name, "confirmed": true }))?,
                ),
            )
            .await?,
            |template| {
                println!(
                    "Saved {} ({:.1} GiB). The machine is stopped; start it again when you need it.",
                    text(&template["name"]),
                    template["sizeBytes"].as_f64().unwrap_or(0.0) / 1_073_741_824.0
                );
            },
        ),
        Command::Template(TemplateCommand::Delete { template, confirm }) => {
            confirm.require("Deleting a template")?;
            report(
                json,
                &e::delete_machine_template(engine, template, input(json!({ "confirmed": true }))?)
                    .await?,
                |_| println!("Deleted."),
            )
        }
        Command::Guest(GuestCommand::Pin {
            machine,
            fingerprint,
        }) => report(
            json,
            &e::trust_mac_guest(
                engine,
                machine,
                input(json!({ "fingerprint": fingerprint }))?,
            )
            .await?,
            |view| {
                println!(
                    "Pinned {}",
                    text(&view["guest"]["ssh"]["pinnedFingerprint"])
                )
            },
        ),
        Command::Guest(GuestCommand::Authorize {
            machine,
            username,
            password_stdin,
        }) => {
            let password = password_from_stdin(password_stdin)?.ok_or_else(|| {
                "pass --password-stdin and pipe the macOS password in".to_string()
            })?;
            report(
                json,
                &e::authorize_mac_guest_key(
                    engine,
                    machine,
                    input(json!({ "username": username, "password": password }))?,
                )
                .await?,
                |view| {
                    println!(
                        "Key installed; signed in as {}",
                        text(&view["guest"]["username"])
                    )
                },
            )
        }
        Command::Guest(GuestCommand::Adopt { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::adopt_template_guest(engine, machine.clone()),
            )
            .await?,
            |view| println!("Adopted; signed in as {}", text(&view["guest"]["username"])),
        ),
        Command::Xcode(XcodeCommand::Import { machine, path }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::import_mac_xcode_package(
                    engine,
                    machine.clone(),
                    input(json!({ "path": path }))?,
                ),
            )
            .await?,
            |_| println!("Imported. `buildbridge xcode activate <machine>` is next."),
        ),
        Command::Xcode(XcodeCommand::Activate {
            machine,
            password_stdin,
        }) => {
            let password = password_from_stdin(password_stdin)?;
            report(
                json,
                &on_machine(
                    engine,
                    &machine,
                    e::activate_mac_xcode(
                        engine,
                        machine.clone(),
                        input(json!({ "password": password }))?,
                    ),
                )
                .await?,
                |view| {
                    println!(
                        "Xcode {} active",
                        text(&view["guest"]["diagnostics"]["xcodeVersion"])
                    )
                },
            )
        }
        Command::Project(ProjectCommand::Approve {
            machine,
            path,
            scheme,
            module,
        }) => {
            if machine_is_android(engine, &machine).await? {
                report(
                    json,
                    &e::approve_android_workspace(
                        engine,
                        machine,
                        input(json!({ "path": path, "module": module }))?,
                    )
                    .await?,
                    |view| {
                        let workspace = &view["android"]["workspace"];
                        println!(
                            "Approved {} ({})",
                            text(&workspace["name"]),
                            text(&workspace["applicationId"])
                        );
                        println!("  {}", describe_layout(&workspace["layout"]));
                    },
                )
            } else {
                report(
                    json,
                    &e::approve_apple_workspace(
                        engine,
                        machine,
                        input(json!({ "path": path, "scheme": scheme }))?,
                    )
                    .await?,
                    |view| {
                        let workspace = &view["appleWorkspace"];
                        println!(
                            "Approved {} ({})",
                            text(&workspace["name"]),
                            text(&workspace["bundleIdentifier"])
                        );
                        println!("  {}", describe_layout(&workspace["layout"]));
                    },
                )
            }
        }
        Command::Project(ProjectCommand::Sync { machine }) => {
            if machine_is_android(engine, &machine).await? {
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::sync_android_workspace(engine, machine.clone()),
                    )
                    .await?,
                    |result| {
                        println!(
                            "Synchronized snapshot {}",
                            text(&result["sync"]["snapshotSha256"])
                        )
                    },
                )
            } else {
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::sync_apple_workspace(engine, machine.clone()),
                    )
                    .await?,
                    |result| {
                        println!(
                            "Synchronized snapshot {}",
                            text(&result["sync"]["snapshotSha256"])
                        )
                    },
                )
            }
        }
        Command::Project(ProjectCommand::AdoptLock { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::adopt_guest_podfile_lock(engine, machine.clone()),
            )
            .await?,
            |result| {
                let changes = &result["changes"];
                if changes["identical"].as_bool() == Some(true) {
                    println!(
                        "The project already held the guest's Podfile.lock; the block is lifted."
                    );
                } else {
                    let pods = changes["pods"].as_array().map_or(0, Vec::len);
                    println!(
                        "Adopted the guest's Podfile.lock into the project: {pods} pod(s) repinned. Commit it in the project."
                    );
                    for pod in changes["pods"].as_array().into_iter().flatten() {
                        println!(
                            "  {} {} → {}",
                            text(&pod["name"]),
                            text(&pod["before"]),
                            text(&pod["after"])
                        );
                    }
                }
            },
        ),
        Command::Build(BuildCommand::Test {
            machine,
            target,
            allow_http,
            version,
            build,
        }) => {
            if machine_is_android(engine, &machine).await? {
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::run_android_debug_build(
                            engine,
                            machine.clone(),
                            allow_http,
                            version_input(version, build)?,
                        ),
                    )
                    .await?,
                    |result| {
                        let build = &result["build"];
                        println!(
                            "Debug build passed: {} {} ({}) with {}",
                            text(&build["applicationId"]),
                            text(&build["versionName"]),
                            text(&build["versionCode"]),
                            text(&build["toolchain"]["jdkVersion"])
                        );
                        if build["allowHttp"].as_bool() == Some(true) {
                            println!("HTTP API access enabled for this debug APK.");
                        }
                    },
                )
            } else {
                if allow_http {
                    return Err("--allow-http is only available for Android debug builds.".into());
                }
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::run_apple_smoke_build(
                            engine,
                            machine.clone(),
                            input(json!({
                                "target": target,
                                "version": version_json(version, build),
                            }))?,
                        ),
                    )
                    .await?,
                    |result| {
                        println!(
                            "Test build passed with Xcode {}",
                            text(&result["view"]["guest"]["diagnostics"]["xcodeVersion"])
                        )
                    },
                )
            }
        }
        Command::Build(BuildCommand::Release {
            machine,
            env,
            outputs,
            version,
            build,
        }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::run_android_signed_release(
                    engine,
                    machine.clone(),
                    env,
                    Some(input(json!(outputs))?),
                    version_input(version, build)?,
                ),
            )
            .await?,
            |result| {
                let release = &result["release"];
                println!(
                    "{} {} ({}) · key {} · certificate {}",
                    text(&release["applicationId"]),
                    text(&release["versionName"]),
                    text(&release["versionCode"]),
                    text(&release["keyAlias"]),
                    text(&release["certificateSha256"])
                );
                for (field, label) in [("aab", "AAB"), ("apk", "APK")] {
                    if !release[field].is_null() {
                        println!(
                            "{label}: {} ({:.2} MB)",
                            text(&release[field]["path"]),
                            release[field]["bytes"].as_f64().unwrap_or(0.0) / 1_000_000.0
                        );
                    }
                }
            },
        ),
        Command::Build(BuildCommand::Archive {
            machine,
            env,
            version,
            build,
        }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::run_apple_signed_archive(
                    engine,
                    machine.clone(),
                    env,
                    version_input(version, build)?,
                ),
            )
            .await?,
            |result| {
                let archive = &result["archive"];
                println!(
                    "{} ({}) · IPA {:.2} MB · archive {:.1} MB",
                    text(&archive["marketingVersion"]),
                    text(&archive["buildNumber"]),
                    archive["ipa"]["bytes"].as_f64().unwrap_or(0.0) / 1_000_000.0,
                    archive["archive"]["bytes"].as_f64().unwrap_or(0.0) / 1_000_000.0
                );
                println!("IPA: {}", text(&archive["ipa"]["path"]));
                println!("Archive: {}", text(&archive["archive"]["path"]));
            },
        ),
        Command::Build(BuildCommand::Check { machine, version }) => report(
            json,
            &e::check_store_builds(
                engine,
                machine.clone(),
                Some(input(json!({ "version": version }))?),
            )
            .await?,
            |check| {
                let store = match check["store"].as_str() {
                    Some("google_play") => "Google Play",
                    _ => "App Store Connect",
                };
                if check["mustExceed"].is_object() {
                    println!(
                        "{store} holds {} ({}); the next upload needs a higher build number, {} or more.",
                        text(&check["mustExceed"]["version"]),
                        text(&check["mustExceed"]["build"]),
                        text(&check["nextBuild"])
                    );
                } else {
                    println!(
                        "{store} holds nothing that binds version {}; any build number is accepted.",
                        text(&check["version"])
                    );
                }
            },
        ),
        Command::Signing(SigningCommand::Kits) => {
            report(json, &e::list_signing_kits(engine).await?, |kits| {
                let rows = kits
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|kit| {
                        vec![
                            text(&kit["id"]),
                            text(&kit["name"]),
                            text(&kit["appStoreConnectKeyId"]),
                            text(&kit["signingCertificateName"]),
                            text(&kit["androidKeystoreName"]),
                            kit["attachedMachines"]
                                .as_array()
                                .map(|names| names.iter().map(text).collect::<Vec<_>>().join(", "))
                                .unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    println!("No signing credentials. Store some on the desktop's Signing page.");
                } else {
                    table(
                        &["id", "name", "team key", "identity", "keystore", "machines"],
                        rows,
                    );
                }
            })
        }
        Command::Signing(SigningCommand::Attach { machine, kit }) => report(
            json,
            &e::attach_signing_kit(engine, machine, input(json!({ "kitId": kit }))?).await?,
            |view| println!("Attached {}", text(&view["signingKit"]["name"])),
        ),
        Command::Signing(SigningCommand::Keystore {
            kit,
            alias,
            name,
            password_stdin,
        }) => {
            let password = password_from_stdin(password_stdin)?.ok_or_else(|| {
                "Pass --password-stdin and the keystore password on stdin.".to_string()
            })?;
            report(
                json,
                &e::create_android_keystore(
                    engine,
                    kit,
                    input(json!({
                        "password": password,
                        "keyAlias": alias,
                        "certificateName": name.unwrap_or_default(),
                        "confirmed": true,
                    }))?,
                )
                .await?,
                |result| {
                    println!(
                        "Created {} (alias {}, certificate {})",
                        text(&result["keystore"]["path"]),
                        text(&result["keystore"]["keyAlias"]),
                        text(&result["keystore"]["certificateSha256"])
                    );
                    println!("Keep the password: nothing can recover an upload key without it.");
                },
            )
        }
        Command::Signing(SigningCommand::Provision { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::provision_mac_signing(engine, machine.clone()),
            )
            .await?,
            |view| {
                println!(
                    "Provisioned {}",
                    text(&view["signing"]["distributionIdentity"]["identityName"])
                )
            },
        ),
        Command::Device(DeviceCommand::List { machine }) => {
            if machine_is_android(engine, &machine).await? {
                report(
                    json,
                    &e::list_android_devices(engine, machine).await?,
                    |devices| {
                        let listed = devices["devices"].as_array().into_iter().flatten();
                        let rows = listed
                            .clone()
                            .map(|device| {
                                vec![
                                    text(&device["serial"]),
                                    text(&device["state"]),
                                    text(&device["model"]),
                                    device_network_text(&device["network"]),
                                ]
                            })
                            .collect::<Vec<_>>();
                        if devices["available"].as_bool() != Some(true) {
                            println!("{}", text(&devices["issue"]));
                        } else if rows.is_empty() {
                            println!(
                                "ADB sees no device. Plug a phone in with USB debugging on, or start an emulator on this host."
                            );
                        } else {
                            table(&["serial", "state", "model", "network"], rows);
                            if listed
                                .clone()
                                .any(|device| device["network"]["onHostNetwork"] == false)
                            {
                                let networks = devices["hostNetworks"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                println!(
                                    "A phone off this computer's network gets no response from an API served here. Join the Wi-Fi network this computer is on{}, then list again.",
                                    if networks.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" ({networks})")
                                    }
                                );
                            }
                        }
                    },
                )
            } else {
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::list_guest_devices(engine, machine.clone()),
                    )
                    .await?,
                    |view| {
                        let rows = view["guest"]["devices"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|device| {
                                vec![
                                    text(&device["name"]),
                                    text(&device["udid"]),
                                    text(&device["osVersion"]),
                                    text(&device["pairingState"]),
                                    text(&device["developerMode"]),
                                ]
                            })
                            .collect::<Vec<_>>();
                        if rows.is_empty() {
                            println!(
                                "The guest sees no phone. `buildbridge device attach` hands one over."
                            );
                        } else {
                            table(&["name", "udid", "iOS", "pairing", "developer mode"], rows);
                        }
                    },
                )
            }
        }
        Command::Device(DeviceCommand::Attach { machine, bus, port }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::attach_usb_device(
                    engine,
                    machine.clone(),
                    input(json!({ "bus": bus, "port": port }))?,
                ),
            )
            .await?,
            |view| {
                println!(
                    "Attached; the guest sees {} phone(s)",
                    view["guest"]["devices"].as_array().map_or(0, Vec::len)
                )
            },
        ),
        Command::Device(DeviceCommand::Detach { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::detach_usb_device(engine, machine.clone()),
            )
            .await?,
            |_| {
                println!(
                    "Detached. Unplug the phone and plug it in again before attaching it again."
                )
            },
        ),
        Command::Device(DeviceCommand::Pair { machine, udid }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::pair_guest_device(engine, machine.clone(), input(json!({ "udid": udid }))?),
            )
            .await?,
            |_| println!("Paired."),
        ),
        Command::Device(DeviceCommand::Prepare {
            machine,
            udid,
            name,
            confirm,
        }) => {
            confirm.require(
                "Registering a phone with the team counts toward the yearly allowance and",
            )?;
            report(
                json,
                &on_machine(
                    engine,
                    &machine,
                    e::prepare_apple_device_signing(
                        engine,
                        machine.clone(),
                        input(json!({ "udid": udid, "deviceName": name, "confirmed": true }))?,
                    ),
                )
                .await?,
                |result| {
                    println!(
                        "Prepared: certificate created {}, device already registered {}, profile created {}",
                        text(&result["certificateCreated"]),
                        text(&result["deviceAlreadyRegistered"]),
                        text(&result["profileCreated"])
                    )
                },
            )
        }
        Command::Device(DeviceCommand::Run {
            machine,
            udid,
            env,
            version,
            build,
        }) => {
            if machine_is_android(engine, &machine).await? {
                if env.is_some() {
                    return Err(
                        "--env applies to macOS machines; an Android debug APK carries the environment it was built with."
                            .into(),
                    );
                }
                if version.is_some() || build.is_some() {
                    return Err(
                        "--version and --build apply to macOS machines; an Android debug APK carries the version it was built with, so set it on `build test`."
                            .into(),
                    );
                }
                let view = serde_json::to_value(e::get_machine(engine, machine.clone()).await?)
                    .map_err(|error| error.to_string())?;
                let Some(sha256) = view["android"]["workspace"]["lastBuild"]["apk"]["sha256"]
                    .as_str()
                    .map(str::to_string)
                else {
                    return Err(
                        "No debug APK is retained. Run `buildbridge build test` on this machine first."
                            .into(),
                    );
                };
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::run_android_device(
                            engine,
                            machine.clone(),
                            input(json!({
                                "kind": "debug",
                                "serial": udid,
                                "expectedSha256": sha256,
                            }))?,
                        ),
                    )
                    .await?,
                    |result| {
                        println!(
                            "Ran {} on {}; log ended: {}",
                            text(&result["run"]["applicationId"]),
                            text(&result["run"]["serial"]),
                            text(&result["run"]["consoleEnd"])
                        )
                    },
                )
            } else {
                report(
                    json,
                    &on_machine(
                        engine,
                        &machine,
                        e::run_apple_device_build(
                            engine,
                            machine.clone(),
                            input(json!({
                                "udid": udid,
                                "envSetId": env,
                                "version": version_json(version, build),
                            }))?,
                        ),
                    )
                    .await?,
                    |result| {
                        println!(
                            "Ran {} on {}; console ended: {}",
                            text(&result["run"]["bundleIdentifier"]),
                            text(&result["run"]["device"]["name"]),
                            text(&result["run"]["consoleEnd"])
                        )
                    },
                )
            }
        }
        Command::Env(EnvCommand::List) => report(json, &e::list_env_sets(engine).await?, |sets| {
            let rows = sets
                .as_array()
                .into_iter()
                .flatten()
                .map(|set| {
                    vec![
                        text(&set["id"]),
                        text(&set["name"]),
                        set["variables"].as_array().map_or(0, Vec::len).to_string(),
                        set["secretKeys"].as_array().map_or(0, Vec::len).to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            if rows.is_empty() {
                println!("No environments.");
            } else {
                table(&["id", "name", "variables", "secrets"], rows);
            }
        }),
        Command::Env(EnvCommand::Set { set, variables }) => {
            let changes = variables
                .iter()
                .map(|pair| {
                    pair.split_once('=')
                        .map(|(key, value)| (key.trim().to_string(), value.to_string()))
                        .ok_or_else(|| format!("{pair} is not KEY=VALUE."))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let payload = env_set_update(engine, &set, &changes).await?;
            report(
                json,
                &e::save_env_set(engine, input(payload)?).await?,
                |_| {
                    for (key, value) in &changes {
                        println!("{key}={value}");
                    }
                },
            )
        }
        Command::Env(EnvCommand::Attach { machine, set }) => report(
            json,
            &e::attach_env_set(engine, machine, input(json!({ "setId": set }))?).await?,
            |view| println!("Attached {}", text(&view["envSet"]["name"])),
        ),
        Command::Usage => report(json, &e::get_usage(engine).await?, print_usage),
        Command::Settings(SettingsCommand::Show) => {
            report(json, &e::get_host_settings(engine).await?, print_settings)
        }
        Command::Settings(SettingsCommand::Set {
            browser,
            remote_builds,
        }) => {
            let mut settings = e::get_host_settings(engine).await?;
            if let Some(browser) = browser {
                settings.browser = Some(browser);
            }
            if let Some(remote_builds) = remote_builds {
                settings.remote_builds = remote_builds;
            }
            report(
                json,
                &e::save_host_settings(engine, settings).await?,
                print_settings,
            )
        }
        // The engine refuses these on its own; asking here as well means `runner status`, which
        // only reads local files, answers the same way as the commands that reach a server.
        Command::Runner(_) if !e::get_host_settings(engine).await?.remote_builds => Err(
            "Remote builds are off on this host. Turn them on with: buildbridge settings set --remote-builds on"
                .to_string(),
        ),
        Command::Runner(RunnerCommand::Status) => {
            report(json, &e::get_runner_status(engine).await?, |status| {
                println!(
                    "paired: {} · runner {} · {}",
                    text(&status["paired"]),
                    text(&status["runnerName"]),
                    text(&status["serverUrl"])
                )
            })
        }
        Command::Runner(RunnerCommand::Check) => {
            report(json, &e::run_once(engine).await?, |result| {
                println!("{}: {}", text(&result["state"]), text(&result["message"]))
            })
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(error) = run(cli).await {
        if json {
            eprintln!("{}", json!({ "error": error }));
        } else {
            eprintln!("error: {error}");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuring_a_machine_moves_only_what_is_named() {
        let stored = json!({
            "name": "Android builder",
            "provider": "android_toolchain",
            "macosRelease": "sequoia",
            "memoryGib": 8,
            "cpuCores": 4,
            "sshPort": 50922,
        });

        let profile = configured_profile(&stored, None, Some(12), None, None, None).unwrap();
        assert_eq!(profile["memoryGib"], json!(12));
        // The engine replaces the stored profile with this one, so a field the command did
        // not name must come back as it was rather than as a default.
        assert_eq!(profile["cpuCores"], json!(4));
        assert_eq!(profile["name"], json!("Android builder"));
        assert_eq!(profile["sshPort"], json!(50922));
        assert_eq!(profile["provider"], json!("android_toolchain"));
        assert_eq!(profile["macosRelease"], json!("sequoia"));

        let renamed = configured_profile(
            &stored,
            Some("Pixel builder".to_string()),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(renamed["name"], json!("Pixel builder"));
        assert_eq!(renamed["memoryGib"], json!(8));

        // Naming nothing would silently rewrite the profile with itself.
        assert!(configured_profile(&stored, None, None, None, None, None).is_err());
    }

    #[test]
    fn the_macos_release_follows_the_provider_unless_named() {
        let release = |arguments: &[&str]| {
            let cli = Cli::try_parse_from(arguments).unwrap();
            let Command::Machine(MachineCommand::Create {
                macos,
                platform,
                provider,
                ..
            }) = cli.command
            else {
                panic!("not a create command");
            };
            let provider = ProviderArg::resolve(platform, provider).unwrap();
            macos.unwrap_or_else(|| provider.recommended_release().to_string())
        };
        assert_eq!(
            release(&["buildbridge", "machine", "create", "Mac"]),
            "tahoe"
        );
        assert_eq!(
            release(&[
                "buildbridge",
                "machine",
                "create",
                "Mac",
                "--provider",
                "dockur-macos"
            ]),
            "sequoia"
        );
        assert_eq!(
            release(&[
                "buildbridge",
                "machine",
                "create",
                "Mac",
                "--provider",
                "dockur-macos",
                "--macos",
                "tahoe"
            ]),
            "tahoe"
        );
        assert_eq!(
            release(&[
                "buildbridge",
                "machine",
                "create",
                "Droid",
                "--platform",
                "android"
            ]),
            "sequoia"
        );
    }

    #[test]
    fn http_override_requires_an_explicit_test_build_flag() {
        for (arguments, expected) in [
            (vec!["buildbridge", "build", "test", "android"], false),
            (
                vec!["buildbridge", "build", "test", "android", "--allow-http"],
                true,
            ),
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(matches!(
                cli.command,
                Command::Build(BuildCommand::Test { allow_http, .. }) if allow_http == expected
            ));
        }
        assert!(
            Cli::try_parse_from(["buildbridge", "build", "release", "android", "--allow-http"])
                .is_err()
        );
    }
}
