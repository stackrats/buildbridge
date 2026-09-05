//! `buildbridge` — BuildBridge from the terminal. The same engine the desktop drives, in this
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
    about = "Build, sign and run iOS apps on a managed macOS machine, from the terminal"
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
    /// Machines: create, start, stop, inspect, discard, delete.
    #[command(subcommand)]
    Machine(MachineCommand),
    /// Templates: prepared machines saved once and cloned in seconds.
    #[command(subcommand)]
    Template(TemplateCommand),
    /// The guest's access: pin its identity and install the BuildBridge key.
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
    /// Signing kits and provisioning.
    #[command(subcommand)]
    Signing(SigningCommand),
    /// A phone plugged into this host: attach, pair, prepare, run.
    #[command(subcommand)]
    Device(DeviceCommand),
    /// Env sets stored on this host.
    #[command(subcommand)]
    Env(EnvCommand),
    /// The control plane this host is paired with.
    #[command(subcommand)]
    Runner(RunnerCommand),
}

#[derive(Subcommand)]
enum MachineCommand {
    List,
    Create {
        name: String,
        #[arg(long, default_value = "tahoe")]
        macos: String,
        #[arg(long, default_value_t = 8)]
        memory: u32,
        #[arg(long, default_value_t = 4)]
        cores: u32,
        #[arg(long)]
        port: Option<u16>,
        /// Clone a saved template instead of installing macOS.
        #[arg(long)]
        from_template: Option<String>,
        /// Which image runs macOS. docker-osx shows the screen in a window on this host's
        /// display; dockur-macos serves it as a web page on the port after the SSH port and
        /// needs /dev/net/tun and the machine's memory free. Either builds and clones the same.
        #[arg(long, value_enum, default_value_t = ProviderArg::DockerOsx)]
        provider: ProviderArg,
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
    /// Install the BuildBridge key; the macOS password is read from stdin.
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
    Approve {
        machine: String,
        path: String,
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
    /// The unsigned test build.
    Test {
        machine: String,
        #[arg(long, default_value = "device_sdk")]
        target: String,
    },
    /// The signed archive and IPA.
    Archive {
        machine: String,
        #[arg(long)]
        env: Option<String>,
    },
}

#[derive(Subcommand)]
enum SigningCommand {
    Kits,
    Attach { machine: String, kit: String },
    Provision { machine: String },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// Phones the guest sees, after asking it.
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
    /// Build, install and launch the Debug build; its console streams until Ctrl-C.
    Run {
        machine: String,
        udid: String,
        #[arg(long)]
        env: Option<String>,
    },
}

#[derive(Subcommand)]
enum EnvCommand {
    List,
    /// Change values in a stored set: KEY=VALUE pairs; every other variable keeps its value.
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
        if let Some(last) = payload.get("lastLine").and_then(Value::as_str) {
            self.line(format!("  {last}"));
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

/// The complete save payload for one stored set with some values changed: the engine removes
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
        .ok_or_else(|| format!("No env set is stored as {set_id}; `env list` names them."))?;
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
enum ProviderArg {
    DockerOsx,
    DockurMacos,
}

impl ProviderArg {
    fn value(self) -> &'static str {
        match self {
            Self::DockerOsx => "docker_osx",
            Self::DockurMacos => "dockur_macos",
        }
    }
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
            &["id", "name", "state", "busy", "project", "kit", "template"],
            rows,
        );
    }
}

fn print_machine(view: &Value) {
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
        ("kit", text(&view["signingKit"]["name"])),
        ("signing", text(&view["signingHealth"])),
        ("archive", text(&view["archive"]["ipa"]["path"])),
    ];
    for (label, value) in rows {
        println!("{label:<12} {value}");
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
            provider,
        }) => {
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
        Command::Machine(MachineCommand::Start { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::launch_mac_builder(engine, machine.clone()),
            )
            .await?,
            print_done,
        ),
        Command::Machine(MachineCommand::Stop { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::stop_mac_builder(engine, machine.clone()),
            )
            .await?,
            print_done,
        ),
        Command::Machine(MachineCommand::Show { machine }) => report(
            json,
            &e::get_mac_builder_status(engine, machine).await?,
            print_machine,
        ),
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
            &e::trust_mac_builder_guest(
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
        Command::Project(ProjectCommand::Approve { machine, path }) => report(
            json,
            &e::approve_apple_workspace(engine, machine, input(json!({ "path": path }))?).await?,
            |view| {
                println!(
                    "Approved {} ({})",
                    text(&view["appleWorkspace"]["name"]),
                    text(&view["appleWorkspace"]["bundleIdentifier"])
                )
            },
        ),
        Command::Project(ProjectCommand::Sync { machine }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::sync_apple_workspace(engine, machine.clone()),
            )
            .await?,
            |view| {
                println!(
                    "Synchronized snapshot {}",
                    text(&view["appleWorkspace"]["lastSnapshotSha256"])
                )
            },
        ),
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
        Command::Build(BuildCommand::Test { machine, target }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::run_apple_smoke_build(
                    engine,
                    machine.clone(),
                    input(json!({ "target": target }))?,
                ),
            )
            .await?,
            |result| {
                println!(
                    "Test build passed with Xcode {}",
                    text(&result["view"]["guest"]["diagnostics"]["xcodeVersion"])
                )
            },
        ),
        Command::Build(BuildCommand::Archive { machine, env }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::run_apple_signed_archive(engine, machine.clone(), env),
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
                            kit["attachedMachines"]
                                .as_array()
                                .map(|names| names.iter().map(text).collect::<Vec<_>>().join(", "))
                                .unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    println!("No signing kits. Store one in the desktop's Signing kits page.");
                } else {
                    table(&["id", "name", "team key", "identity", "machines"], rows);
                }
            })
        }
        Command::Signing(SigningCommand::Attach { machine, kit }) => report(
            json,
            &e::attach_signing_kit(engine, machine, input(json!({ "kitId": kit }))?).await?,
            |view| println!("Attached {}", text(&view["signingKit"]["name"])),
        ),
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
        Command::Device(DeviceCommand::List { machine }) => report(
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
        ),
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
        Command::Device(DeviceCommand::Run { machine, udid, env }) => report(
            json,
            &on_machine(
                engine,
                &machine,
                e::run_apple_device_build(
                    engine,
                    machine.clone(),
                    input(json!({ "udid": udid, "envSetId": env }))?,
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
        ),
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
                println!("No env sets.");
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
