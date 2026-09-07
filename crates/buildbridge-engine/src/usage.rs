//! What each machine costs right now: the cores and memory its container holds.
//!
//! Sampling happens only while someone is watching. The desktop asks for samples while its
//! window is visible and stops asking when it is hidden, so a laptop with buildbridge in the
//! tray spends nothing measuring itself; the command line takes one sample and leaves. Nothing
//! is persisted: a reading is worthless a minute later, and it carries none of the person's
//! content.
//!
//! The reading is Docker's own — one `docker stats --no-stream` for every running container,
//! matched to the machines that own them — so the numbers agree with what `docker stats`
//! would show in a terminal.

use std::time::Duration;

use super::*;

/// The event every sample goes out under. The payload is a [`UsageSample`].
pub const USAGE_EVENT: &str = "host-usage";

/// The pause between samples. `docker stats` itself takes about a second to read its counters
/// twice, so a reading lands every three seconds or so: fast enough to see a build begin,
/// slow enough that measuring is not itself the load.
const USAGE_INTERVAL: Duration = Duration::from_secs(2);

/// One machine's cost over the sampling window.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MachineUsage {
    pub machine_id: String,
    /// Cores consumed: `1.0` is one saturated core, so a machine with several cores can
    /// exceed it during a build.
    pub cpu_cores: f64,
    /// The working set, page cache excluded, as `docker stats` shows it.
    #[ts(type = "number")]
    pub memory_bytes: u64,
    /// The limit the container runs under: its own when it has one, otherwise the host's memory.
    #[ts(type = "number")]
    pub memory_limit_bytes: u64,
    /// Whether the limit is the container's own. An Android toolchain is created with a memory
    /// limit and is killed when it passes it; a macOS machine has none, and its share is only a
    /// share of the host.
    pub memory_limited: bool,
}

/// One tick: every running machine, measured together so the numbers can be summed.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsageSample {
    #[ts(type = "number")]
    pub at_unix_ms: u64,
    /// The host's cores, so a client can say "2.3 of 8" rather than "2.3".
    pub host_cores: u32,
    #[ts(type = "number")]
    pub host_memory_bytes: u64,
    pub machines: Vec<MachineUsage>,
}

/// Whether the desktop is watching, and whether the thread that serves it is alive. Both under
/// one lock so a watcher returning just as the thread winds down still gets a thread.
#[derive(Default)]
pub(crate) struct UsageSampler {
    wanted: bool,
    running: bool,
}

/// The machines' containers matched against every running container's reading.
pub(crate) fn usage_from_stats(
    containers: &HashMap<String, String>,
    stats: Vec<buildbridge_machines::ContainerStats>,
    host_memory_bytes: u64,
) -> Vec<MachineUsage> {
    stats
        .into_iter()
        .filter_map(|stat| {
            let machine_id = containers.get(&stat.name)?.clone();
            Some(MachineUsage {
                machine_id,
                cpu_cores: stat.cpu_cores,
                memory_bytes: stat.memory_bytes,
                memory_limit_bytes: stat.memory_limit_bytes,
                memory_limited: stat.memory_limit_bytes > 0
                    && host_memory_bytes > 0
                    && stat.memory_limit_bytes < host_memory_bytes,
            })
        })
        .collect()
}

/// One reading of every running machine. Blocks for about a second.
pub(crate) fn sample_usage(app: &Engine) -> Result<UsageSample, String> {
    let registry = machines::load_registry(app)?;
    let containers: HashMap<String, String> = registry
        .machines
        .iter()
        .filter_map(|machine| {
            MachinePaths::resolve(app, &machine.id)
                .ok()
                .map(|paths| (paths.container_name, machine.id.clone()))
        })
        .collect();
    let stats =
        buildbridge_machines::running_container_stats().map_err(|error| error.to_string())?;
    let host_memory_bytes = buildbridge_machines::host_memory_bytes();
    let mut machines = usage_from_stats(&containers, stats, host_memory_bytes);
    machines.sort_by(|a, b| a.machine_id.cmp(&b.machine_id));
    Ok(UsageSample {
        at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0),
        host_cores: buildbridge_machines::host_cores(),
        host_memory_bytes,
        machines,
    })
}

/// One sample, for a client that asks once.
pub async fn get_usage(app: &Engine) -> Result<UsageSample, String> {
    let engine = app.clone();
    tokio::task::spawn_blocking(move || sample_usage(&engine))
        .await
        .map_err(|error| error.to_string())?
}

/// Starts or stops the stream of [`USAGE_EVENT`] samples. Turning it on while it runs is a
/// no-op, so a client can say what it wants without counting; turning it off lets the thread
/// finish its current reading and end.
pub async fn set_usage_sampling(app: &Engine, enabled: bool) -> Result<(), String> {
    let mut sampler = app
        .state()
        .usage
        .lock()
        .map_err(|_| "The usage sampler is poisoned.".to_string())?;
    sampler.wanted = enabled;
    if enabled && !sampler.running {
        sampler.running = true;
        let engine = app.clone();
        std::thread::Builder::new()
            .name("buildbridge-usage".to_string())
            .spawn(move || sample_loop(engine))
            .map_err(|error| {
                sampler.running = false;
                format!("The usage sampler could not start: {error}")
            })?;
    }
    Ok(())
}

fn sample_loop(app: Engine) {
    loop {
        {
            let Ok(mut sampler) = app.state().usage.lock() else {
                return;
            };
            if !sampler.wanted {
                sampler.running = false;
                return;
            }
        }
        // Docker being away is already on the host chip; the next tick tries again.
        if let Ok(sample) = sample_usage(&app) {
            let _ = app.emit(USAGE_EVENT, sample);
        }
        std::thread::sleep(USAGE_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buildbridge_machines::ContainerStats;

    fn stat(
        name: &str,
        cpu_cores: f64,
        memory_bytes: u64,
        memory_limit_bytes: u64,
    ) -> ContainerStats {
        ContainerStats {
            name: name.to_string(),
            cpu_cores,
            memory_bytes,
            memory_limit_bytes,
        }
    }

    #[test]
    fn only_machines_containers_count_and_a_limit_below_the_host_is_the_containers_own() {
        let host = 32 * 1024 * 1024 * 1024;
        let containers = HashMap::from([
            (
                "buildbridge-macos-team-mac".to_string(),
                "team-mac".to_string(),
            ),
            ("buildbridge-android-pixel".to_string(), "pixel".to_string()),
        ]);
        let usage = usage_from_stats(
            &containers,
            vec![
                stat("buildbridge-macos-team-mac", 3.4, 9_000, host),
                stat("someone-elses-db", 0.2, 1_000, host),
                stat(
                    "buildbridge-android-pixel",
                    0.5,
                    2_000,
                    6 * 1024 * 1024 * 1024,
                ),
            ],
            host,
        );
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].machine_id, "team-mac");
        assert!(!usage[0].memory_limited);
        assert_eq!(usage[1].machine_id, "pixel");
        assert!(usage[1].memory_limited);
        assert_eq!(usage[1].memory_bytes, 2_000);
    }

    #[test]
    fn an_unknown_host_size_never_calls_a_limit_the_containers_own() {
        let containers =
            HashMap::from([("buildbridge-android-pixel".to_string(), "pixel".to_string())]);
        let usage = usage_from_stats(
            &containers,
            vec![stat("buildbridge-android-pixel", 0.0, 0, 6_000)],
            0,
        );
        assert!(!usage[0].memory_limited);
    }
}
