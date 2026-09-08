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
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

/// Writes an executable script and returns once the kernel will actually run it.
pub(crate) fn write_runnable(path: &Path, script: &str) {
    fs::write(path, guarded(script)).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    let deadline = Instant::now() + GIVE_UP_AFTER;
    loop {
        let attempt = Command::new(path)
            .arg(PROBE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match attempt {
            // The exit status is the script's business: this only asks whether it ran.
            Ok(_) => return,
            Err(error)
                if error.kind() == ErrorKind::ExecutableFileBusy && Instant::now() < deadline => {}
            Err(error) => panic!("the fixture {} could not be run: {error}", path.display()),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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
