//! Writing a script that a test is about to run.
//!
//! A test binary runs its tests on many threads and several of them spawn child processes. A
//! fork between writing an executable here and exec'ing it later leaves the forked child
//! holding the write descriptor until it execs its own program, and the kernel refuses to run
//! a file that any process still has open for writing. Nothing buildbridge ships execs a file
//! it has just written, so the wait belongs with the fixtures rather than with the code they
//! exercise.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static FIXTURE_DIRECTORIES: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory under the system temp for one fixture. The name carries the
/// process id and a counter rather than a clock: a test binary creates fixtures on many
/// threads at once, and a macOS clock ticks in microseconds, so two fixtures born in the same
/// instant collided on the name and one of them died in `create_dir`.
pub(crate) fn fixture_dir(prefix: &str) -> PathBuf {
    let ordinal = FIXTURE_DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "buildbridge-{prefix}-{}-{ordinal}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

/// The argument a guarded script answers by doing nothing at all.
const PROBE: &str = "--buildbridge-probe";

/// How long to wait for the last writer to let go. The window is one fork away from its exec,
/// so this is orders of magnitude more than it takes; a fixture that is still busy after it
/// has something else wrong with it.
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

/// The script with a guard for the interpreter its shebang names, so that asking whether the
/// file will run cannot touch what the test then asserts the script did. An interpreter this
/// does not know is a mistake to fix here rather than a fixture that silently loses its guard.
fn guarded(script: &str) -> String {
    if let Some(rest) = script.strip_prefix("#!/bin/sh\n") {
        format!("#!/bin/sh\nif [ \"$1\" = {PROBE} ]; then exit 0; fi\n{rest}")
    } else if let Some(rest) = script.strip_prefix("#!/usr/bin/python3\n") {
        format!(
            "#!/usr/bin/python3\nimport sys\nif sys.argv[1:2] == [\"{PROBE}\"]: raise SystemExit(0)\n{rest}"
        )
    } else {
        panic!("a fixture script must start with a shebang this module knows how to guard");
    }
}

/// Retry only the kernel's transient refusal to execute a file another test's fork still
/// has open for writing. Every other error and every actual process exit returns immediately.
fn retry_executable_busy<T>(mut attempt: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let deadline = Instant::now() + GIVE_UP_AFTER;
    loop {
        match attempt() {
            Err(error)
                if error.kind() == ErrorKind::ExecutableFileBusy && Instant::now() < deadline => {}
            result => return result,
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Executes an existing helper without rewriting its contents or skipping its shebang.
pub(crate) fn runnable_output(command: &mut Command) -> std::io::Result<Output> {
    retry_executable_busy(|| command.output())
}

/// Writes an executable script and returns once the kernel will actually run it.
pub(crate) fn write_runnable(path: &Path, script: &str) {
    fs::write(path, guarded(script)).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    retry_executable_busy(|| {
        Command::new(path)
            .arg(PROBE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    })
    .unwrap_or_else(|error| panic!("the fixture {} could not be run: {error}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guard_goes_after_the_shebang_and_leaves_the_script_alone() {
        let shell = guarded("#!/bin/sh\nexit 3\n");
        assert!(shell.starts_with("#!/bin/sh\nif [ \"$1\" = --buildbridge-probe ]"));
        assert!(shell.ends_with("\nexit 3\n"));

        let python = guarded("#!/usr/bin/python3\nraise SystemExit(3)\n");
        assert!(python.contains("if sys.argv[1:2] == [\"--buildbridge-probe\"]"));
        assert!(python.ends_with("\nraise SystemExit(3)\n"));
    }

    #[test]
    #[should_panic(expected = "shebang this module knows how to guard")]
    fn an_unknown_interpreter_is_refused_rather_than_left_unguarded() {
        guarded("#!/usr/bin/perl\nexit 0;\n");
    }

    #[test]
    fn a_written_script_runs_and_answers_the_probe_without_doing_its_work() {
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-test-scripts-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("fixture");
        // The probe must not leave the marker the script writes for every other argument.
        write_runnable(&script, "#!/bin/sh\nprintf ran > \"$0.marker\"\n");
        assert!(!script.with_file_name("fixture.marker").exists());

        assert!(Command::new(&script).status().unwrap().success());
        assert_eq!(
            fs::read_to_string(script.with_file_name("fixture.marker")).unwrap(),
            "ran"
        );
        fs::remove_dir_all(&directory).unwrap();
    }
}
