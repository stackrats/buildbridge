//! What each container costs right now, as `docker stats` reports it.
//!
//! One `--no-stream` call covers every running container: Docker reads the cgroup counters
//! twice, about a second apart, and reports the CPU delta as a percentage of one core, so the
//! call takes that second and nothing here has to remember a previous reading. The cost is paid
//! only while someone is watching; the engine samples on request and never in the background.

use super::*;

/// One running container's cost over Docker's sampling window.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerStats {
    pub name: String,
    /// Cores consumed: `1.0` is one saturated core, so a machine with several cores can exceed
    /// it. Cores rather than a percentage, because Docker's percentage is of one core and
    /// reads as "340%" on a busy build.
    pub cpu_cores: f64,
    /// The working set, page cache excluded, the number `docker stats` itself shows.
    pub memory_bytes: u64,
    /// The cgroup limit, or the host's memory when the container has none of its own.
    pub memory_limit_bytes: u64,
}

/// Every running container's cost, in one call that takes about a second.
pub fn running_container_stats() -> Result<Vec<ContainerStats>, ProviderError> {
    let output = docker_command()
        .args(["stats", "--no-stream", "--format", "{{json .}}"])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(ProviderError::DockerCommand {
            operation: "stats",
            message: clean_output(&output.stderr),
        });
    }
    Ok(parse_stats(&String::from_utf8_lossy(&output.stdout)))
}

/// The host's cores, the denominator that makes "2.3 cores" legible as "2.3 of 8".
pub fn host_cores() -> u32 {
    thread::available_parallelism()
        .map(|count| u32::try_from(count.get()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// The host's memory, in bytes; zero when the platform does not say.
pub fn host_memory_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| parse_meminfo_total(&text))
            .unwrap_or(0)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

/// `MemTotal:       32661300 kB` from `/proc/meminfo`.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib = line
        .trim_start_matches("MemTotal:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

/// One `{{json .}}` line per container; a line that is not a reading is skipped rather than
/// failing the whole sample.
pub(crate) fn parse_stats(text: &str) -> Vec<ContainerStats> {
    text.lines().filter_map(parse_stats_line).collect()
}

fn parse_stats_line(line: &str) -> Option<ContainerStats> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let name = value["Name"].as_str()?.to_string();
    let cpu_cores = parse_percent(value["CPUPerc"].as_str()?)? / 100.0;
    let (memory_bytes, memory_limit_bytes) = parse_memory_usage(value["MemUsage"].as_str()?)?;
    Some(ContainerStats {
        name,
        cpu_cores,
        memory_bytes,
        memory_limit_bytes,
    })
}

fn parse_percent(text: &str) -> Option<f64> {
    text.trim().strip_suffix('%')?.trim().parse().ok()
}

/// `1.38GiB / 30.27GiB`: what is held, then the limit.
fn parse_memory_usage(text: &str) -> Option<(u64, u64)> {
    let (used, limit) = text.split_once('/')?;
    Some((parse_size(used)?, parse_size(limit)?))
}

/// Docker's own size notation: binary units for memory (`274.3MiB`), decimal for I/O
/// (`2.93MB`), and a bare `0B`.
pub(crate) fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits = text
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(text.len());
    let number: f64 = text[..digits].parse().ok()?;
    let multiplier: f64 = match text[digits..].trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        "kib" => 1024.0,
        "mib" => 1024f64.powi(2),
        "gib" => 1024f64.powi(3),
        "tib" => 1024f64.powi(4),
        "pib" => 1024f64.powi(5),
        _ => return None,
    };
    Some((number * multiplier).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_dockers_units() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("512B"), Some(512));
        assert_eq!(parse_size("1.5kB"), Some(1_500));
        assert_eq!(parse_size("2.93MB"), Some(2_930_000));
        assert_eq!(parse_size("274.3MiB"), Some(287_624_397));
        assert_eq!(parse_size(" 1.38GiB "), Some(1_481_763_717));
        assert_eq!(parse_size("30.27GiB"), Some(32_502_165_012));
        assert_eq!(parse_size("1.2XB"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn a_stats_line_becomes_cores_and_bytes() {
        let text = concat!(
            r#"{"BlockIO":"223MB / 2.49MB","CPUPerc":"340.50%","Container":"7521","ID":"7521","MemPerc":"4.56%","MemUsage":"1.38GiB / 30.27GiB","Name":"buildbridge-macos-team-mac","NetIO":"2.93MB / 1.98MB","PIDs":"105"}"#,
            "\n",
            r#"{"CPUPerc":"0.00%","MemUsage":"0B / 0B","Name":"buildbridge-android-stopped"}"#,
            "\n",
            "not a reading\n",
            r#"{"CPUPerc":"--","MemUsage":"-- / --","Name":"gone"}"#,
            "\n",
        );
        let stats = parse_stats(text);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].name, "buildbridge-macos-team-mac");
        assert!(
            (stats[0].cpu_cores - 3.405).abs() < 1e-9,
            "{}",
            stats[0].cpu_cores
        );
        assert_eq!(stats[0].memory_bytes, 1_481_763_717);
        assert_eq!(stats[0].memory_limit_bytes, 32_502_165_012);
        assert_eq!(stats[1].cpu_cores, 0.0);
        assert_eq!(stats[1].memory_limit_bytes, 0);
    }

    #[test]
    fn meminfo_total_is_read_in_kibibytes() {
        let text = "MemTotal:       32661300 kB\nMemFree:        12345 kB\n";
        assert_eq!(parse_meminfo_total(text), Some(32_661_300 * 1024));
        assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
    }
}
