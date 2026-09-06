#![cfg(unix)]

use super::*;

struct JobFixture {
    directory: PathBuf,
    tools: PathBuf,
    jobs: PathBuf,
    workspace: PathBuf,
}

impl JobFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-android-job-{}-{nonce}",
            std::process::id()
        ));
        let tools = directory.join("tools");
        let jobs = tools.join("jobs");
        let workspace = directory.join("workspace");
        fs::create_dir_all(&jobs).unwrap();
        fs::create_dir(&workspace).unwrap();
        Self {
            directory,
            tools,
            jobs,
            workspace,
        }
    }

    fn job(&self, name: &str, status: Option<i32>) -> PathBuf {
        let job = self.jobs.join(name);
        fs::create_dir(&job).unwrap();
        fs::write(job.join("meta"), "keep-original-request").unwrap();
        fs::write(job.join("pid"), std::process::id().to_string()).unwrap();
        fs::write(job.join("output.log"), "keep-original-log\n").unwrap();
        if let Some(status) = status {
            fs::write(job.join("status"), status.to_string()).unwrap();
        }
        job
    }

    fn prepare(&self, reattach_debug: bool) -> Output {
        Command::new("/bin/sh")
            .args(["-c", ANDROID_JOB_PREPARATION, "sh"])
            .arg(&self.jobs)
            .arg(if reattach_debug { "1" } else { "0" })
            .output()
            .unwrap()
    }

    fn run_debug_job(&self, body: &str) -> Output {
        let script = crate::device_run::guest_job_script(
            "android-debug-build",
            self.tools.to_str().unwrap(),
            "''",
            body,
        );
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if started.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the Android job fixture did not finish");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for JobFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn assert_job_preserved(job: &Path, status: Option<i32>) {
    assert_eq!(
        fs::read_to_string(job.join("meta")).unwrap(),
        "keep-original-request"
    );
    assert_eq!(
        fs::read_to_string(job.join("output.log")).unwrap(),
        "keep-original-log\n"
    );
    assert_eq!(
        fs::read_to_string(job.join("pid")).unwrap(),
        std::process::id().to_string()
    );
    if let Some(status) = status {
        assert_eq!(
            fs::read_to_string(job.join("status")).unwrap(),
            status.to_string()
        );
    } else {
        assert!(!job.join("status").exists());
    }
}

#[test]
fn completed_debug_log_cannot_replay_after_the_snapshot_replaced_its_apk() {
    let fixture = JobFixture::new();
    let apk = fixture
        .workspace
        .join("android/app/build/outputs/apk/debug/app-debug.apk");
    fs::create_dir_all(apk.parent().unwrap()).unwrap();
    fs::write(&apk, b"old-apk").unwrap();
    let job = fixture.job("android-debug-build", Some(0));
    fs::write(
        job.join("output.log"),
        "__BUILDBRIDGE_ALLOW_HTTP__\tfalse\nBUILD SUCCESSFUL old snapshot\n",
    )
    .unwrap();

    // A newly synchronized snapshot has the same workspace path, but no earlier APK.
    fs::remove_dir_all(&fixture.workspace).unwrap();
    fs::create_dir(&fixture.workspace).unwrap();
    fs::write(fixture.workspace.join("source"), b"new snapshot").unwrap();
    let body = format!(
        "/bin/mkdir -p {}\n/usr/bin/printf '%s' 'fresh-apk' > {}\n/usr/bin/printf '__BUILDBRIDGE_ALLOW_HTTP__\\tfalse\\nBUILD SUCCESSFUL fresh snapshot\\n'",
        shell_single_quote(apk.parent().unwrap().to_str().unwrap()),
        shell_single_quote(apk.to_str().unwrap())
    );
    let destination = fixture.directory.join("transferred.apk");

    // Exercise the actual detachable wrapper to reproduce the misleading success:
    // the old log is replayed without running the new recipe, then APK transfer fails.
    let stale = fixture.run_debug_job(&body);
    assert!(stale.status.success(), "{stale:?}");
    assert!(clean_output(&stale.stdout).contains("BUILD SUCCESSFUL old snapshot"));
    assert!(clean_output(&stale.stdout).contains("__BUILDBRIDGE_REATTACHED__:yes"));
    assert_eq!(
        fs::copy(&apk, &destination).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );

    // A new debug request discards only that completed job before running its recipe.
    let prepared = fixture.prepare(true);
    assert!(prepared.status.success(), "{prepared:?}");
    assert!(!job.exists());
    let fresh = fixture.run_debug_job(&body);
    assert!(fresh.status.success(), "{fresh:?}");
    let output = clean_output(&fresh.stdout);
    assert!(output.contains("BUILD SUCCESSFUL fresh snapshot"));
    assert!(!output.contains("old snapshot"));
    assert!(!output.contains("__BUILDBRIDGE_REATTACHED__:yes"));
    assert_eq!(fs::copy(&apk, &destination).unwrap(), 9);
    assert_eq!(fs::read(destination).unwrap(), b"fresh-apk");
    assert_eq!(
        fs::read(fixture.workspace.join("source")).unwrap(),
        b"new snapshot"
    );
}

#[test]
fn active_debug_job_allows_reattach_but_refuses_sync_and_release_without_changes() {
    let fixture = JobFixture::new();
    let debug = fixture.job("android-debug-build", None);
    let release = fixture.job("android-release", Some(0));
    fs::write(fixture.workspace.join("source"), b"active snapshot").unwrap();

    let refused = fixture.prepare(false);
    assert!(!refused.status.success());
    assert_job_preserved(&debug, None);
    assert_job_preserved(&release, Some(0));
    assert_eq!(
        fs::read(fixture.workspace.join("source")).unwrap(),
        b"active snapshot"
    );

    let reattach = fixture.prepare(true);
    assert!(reattach.status.success(), "{reattach:?}");
    assert_job_preserved(&debug, None);
    assert!(!release.exists());
}

#[test]
fn active_release_refuses_every_new_recipe_before_any_completed_job_is_deleted() {
    for reattach_debug in [false, true] {
        let fixture = JobFixture::new();
        let debug = fixture.job("android-debug-build", Some(0));
        let release = fixture.job("android-release", None);
        let refused = fixture.prepare(reattach_debug);
        assert!(!refused.status.success());
        assert_job_preserved(&debug, Some(0));
        assert_job_preserved(&release, None);
    }
}

#[test]
fn completed_jobs_are_removed_for_sync_and_build_without_touching_other_state() {
    for reattach_debug in [false, true] {
        for status in [0, 7] {
            let fixture = JobFixture::new();
            let debug = fixture.job("android-debug-build", Some(status));
            let release = fixture.job("android-release", Some(status));
            let unrelated = fixture.job("unrelated-job", Some(0));
            fs::write(fixture.workspace.join("source"), b"keep source").unwrap();
            fs::write(fixture.tools.join("tool-cache"), b"keep cache").unwrap();
            let prepared = fixture.prepare(reattach_debug);
            assert!(prepared.status.success(), "{prepared:?}");
            assert!(!debug.exists());
            assert!(!release.exists());
            assert_job_preserved(&unrelated, Some(0));
            assert_eq!(
                fs::read(fixture.workspace.join("source")).unwrap(),
                b"keep source"
            );
            assert_eq!(
                fs::read(fixture.tools.join("tool-cache")).unwrap(),
                b"keep cache"
            );
            assert!(fixture.prepare(reattach_debug).status.success());
        }
    }
}
