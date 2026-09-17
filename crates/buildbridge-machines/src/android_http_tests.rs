use super::*;

#[test]
fn old_debug_records_default_to_no_http_override() {
    let record: AndroidBuildResult = serde_json::from_value(serde_json::json!({
        "applicationId": "example.app", "versionName": "1.0", "versionCode": "1",
        "toolchain": {"jdkVersion": "17", "buildToolsVersion": "35"},
        "outputTail": []
    }))
    .unwrap();
    assert!(!record.allow_http);
    assert!(record.live_reload_url.is_none());
    assert!(record.apk.is_none());
}

#[test]
fn reattached_debug_jobs_must_match_the_requested_http_mode() {
    for requested in [false, true] {
        assert!(validate_android_debug_http_mode(requested, Some(requested)).is_ok());
        assert!(validate_android_debug_http_mode(requested, Some(!requested)).is_err());
        assert!(validate_android_debug_http_mode(requested, None).is_err());
    }
}

#[test]
fn reattached_debug_jobs_must_match_the_requested_development_server() {
    assert!(validate_android_debug_live_reload(None, Some("")).is_ok());
    assert!(validate_android_debug_live_reload(None, None).is_err());
    let url = "http://localhost:5173/";
    assert!(validate_android_debug_live_reload(Some(url), Some(url)).is_ok());
    assert!(validate_android_debug_live_reload(Some(url), Some("")).is_err());
    assert!(validate_android_debug_live_reload(None, Some(url)).is_err());
    assert!(validate_android_debug_live_reload(Some(url), Some("http://localhost:3000/")).is_err());
}

struct HttpFixture(PathBuf);

impl HttpFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "buildbridge-http-debug-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn debug_http_recipe_keeps_job_completion_and_failure_status_and_cleans_init() {
    let fixture = HttpFixture::new();
    let gradlew = fixture.0.join("gradlew");
    crate::test_scripts::write_runnable(
        &gradlew,
        r#"#!/bin/sh
set -eu
/usr/bin/printf '%s\n' "$@" > arguments
/usr/bin/printf '%s' "$BUILDBRIDGE_ALLOW_HTTP" > http-mode
/usr/bin/printf '%s' "$BUILDBRIDGE_LIVE_RELOAD_URL" > live-url
previous=''
for argument do
    if test "$previous" = '--init-script'; then
        test -f "$argument"
        /bin/cat "$argument" > captured-init.gradle
        /usr/bin/printf '%s' "$argument" > init-path
    fi
    previous="$argument"
done
exit "$FIXTURE_GRADLE_STATUS"
"#,
    );
    for (allow_http, live_url, code) in [
        (true, "", 0),
        (true, "", 7),
        (false, "", 0),
        (true, "http://localhost:5173/", 0),
        (false, "https://dev.example/", 0),
        (false, "https://dev.example/", 7),
    ] {
        let job_name = format!("fixture-{allow_http}-{}-{code}", !live_url.is_empty());
        let body = android_gradle_command(
            &ProjectLayout::capacitor_default(),
            "./gradlew",
            ":app:assembleDebug",
            allow_http,
            !live_url.is_empty(),
        );
        let body = format!(
            "readonly BUILDBRIDGE_LIVE_RELOAD_URL BUILDBRIDGE_LIVE_RELOAD_MODULE\n/usr/bin/printf '__BUILDBRIDGE_LIVE_RELOAD_URL__\\t%s\\n' \"$BUILDBRIDGE_LIVE_RELOAD_URL\"\n{body}"
        );
        let script = android_job_script(&job_name, fixture.0.to_str().unwrap(), &body);
        if !live_url.is_empty() {
            assert!(
                !script.contains(live_url),
                "the URL must remain a separate argument"
            );
        }
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script, "sh", live_url, ":app"])
            .current_dir(&fixture.0)
            .env("FIXTURE_GRADLE_STATUS", code.to_string())
            .env("BUILDBRIDGE_ALLOW_HTTP", "false")
            .env("BUILDBRIDGE_LIVE_RELOAD_URL", "http://unused.example/")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                panic!("the HTTP override prevented the detached job from finishing");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(code), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .lines()
                .any(
                    |line| marker_fields(line, "__BUILDBRIDGE_LIVE_RELOAD_URL__")
                        == Some(vec![live_url])
                )
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join(format!("jobs/{job_name}/status")))
                .unwrap()
                .trim(),
            code.to_string()
        );
        let arguments = fs::read_to_string(fixture.0.join("arguments")).unwrap();
        let overlay = allow_http || !live_url.is_empty();
        assert_eq!(arguments.contains("--init-script"), overlay);
        assert!(!arguments.contains("Release"));
        if overlay {
            assert_eq!(
                fs::read_to_string(fixture.0.join("http-mode")).unwrap(),
                allow_http.to_string()
            );
            assert_eq!(
                fs::read_to_string(fixture.0.join("live-url")).unwrap(),
                live_url
            );
            assert_eq!(
                fs::read_to_string(fixture.0.join("captured-init.gradle")).unwrap(),
                format!("{ANDROID_HTTP_DEBUG_INIT}\n")
            );
            let init = fs::read_to_string(fixture.0.join("init-path")).unwrap();
            assert!(init.ends_with(".gradle"), "{init}");
            assert!(!Path::new(&init).exists());
            assert!(!Path::new(&init).parent().unwrap().exists(), "{init}");
        }
    }
}

#[cfg(unix)]
#[test]
fn a_debug_job_refuses_an_environment_file_that_overwrites_its_development_server() {
    let fixture = HttpFixture::new();
    let body = "readonly BUILDBRIDGE_LIVE_RELOAD_URL BUILDBRIDGE_LIVE_RELOAD_MODULE\nexport BUILDBRIDGE_LIVE_RELOAD_URL=http://wrong.example/\n/usr/bin/printf 'wrong server reached Gradle\\n'";
    let script = android_job_script("environment-collision", fixture.0.to_str().unwrap(), body);
    let result = Command::new("/bin/sh")
        .args(["-c", &script, "sh", "http://localhost:5173/", ":app"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("wrong server reached Gradle"));
}

/// BSD mktemp replaces only trailing X's, so a template such as `name.XXXXXX.gradle` is
/// created literally and the second build on a Mac fails with "File exists".
#[test]
fn debug_http_init_template_is_portable_to_bsd_mktemp() {
    let body = android_gradle_command(
        &ProjectLayout::capacitor_default(),
        "./gradlew",
        ":app:assembleDebug",
        true,
        false,
    );
    let template = body
        .lines()
        .find_map(|line| line.split_once("/usr/bin/mktemp "))
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .expect("the HTTP debug recipe creates its init script with mktemp");
    let template = template.rsplit(' ').next().unwrap();
    assert!(template.ends_with("XXXXXX"), "{template}");
    assert!(body.contains(r#"--init-script "$init_dir/http.gradle""#));
}

#[test]
#[ignore = "requires BUILDBRIDGE_TEST_GRADLE_LIB pointing at a Gradle distribution's lib directory"]
fn debug_http_overlay_preserves_source_and_tls_settings() {
    let fixture = HttpFixture::new();
    let gradle_lib = std::env::var_os("BUILDBRIDGE_TEST_GRADLE_LIB")
        .expect("set BUILDBRIDGE_TEST_GRADLE_LIB to a local Gradle distribution's lib directory");
    let source = fixture.0.join("android_http.gradle");
    let tests = fixture.0.join("android_http_tests.groovy");
    fs::write(&source, ANDROID_HTTP_DEBUG_INIT).unwrap();
    fs::write(&tests, include_str!("android_http_tests.groovy")).unwrap();
    for (mode, allow_http, live_url) in [
        ("http", "true", ""),
        ("http-live", "true", "http://localhost:5173/"),
        ("https-live", "false", "https://dev.example/"),
    ] {
        let directory = fixture.0.join(mode);
        fs::create_dir(&directory).unwrap();
        let result = Command::new("java")
            .arg("-cp")
            .arg(PathBuf::from(&gradle_lib).join("*"))
            .arg("groovy.ui.GroovyMain")
            .arg(&tests)
            .arg(&source)
            .arg(&directory)
            .env("BUILDBRIDGE_ALLOW_HTTP", allow_http)
            .env("BUILDBRIDGE_LIVE_RELOAD_URL", live_url)
            .output()
            .expect("the Android HTTP overlay fixture requires a local JDK");
        assert!(
            result.status.success(),
            "{mode}: {}\n{}",
            clean_output(&result.stdout),
            clean_output(&result.stderr)
        );
    }
}
