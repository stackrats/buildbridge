//! Execute the actual re-sign helper on the host with Apple's keychain calls replaced.
//! This checks the process/secret boundary, not macOS ABI or real code signing.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn c_function(source: &str, name: &str) -> String {
    let name_at = source.find(&format!("{name}(")).unwrap();
    let start = source[..name_at].rfind('\n').unwrap_or(0);
    let body = name_at + source[name_at..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source[body..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=body + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("helper function {name} has no closing brace");
}

#[test]
fn device_resigning_preserves_entitlements_and_cleans_up_every_keychain_exit() {
    let directory = std::env::temp_dir().join(format!(
        "buildbridge-resign-fixture-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let source_path = directory.join("fixture.c");
    let binary = directory.join("fixture");
    let helper = include_str!("signing_helper.c");
    let mut source = STUBS.to_string();
    for name in [
        "read_exact",
        "read_secret",
        "secure_zero",
        "open_and_unlock_keychain",
        "run_device_resign",
    ] {
        source.push_str(&c_function(helper, name));
        source.push('\n');
    }
    source.push_str(MAIN);
    fs::write(&source_path, source).unwrap();
    let compile = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&source_path)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("the host C compiler must be available to test the signing helper");
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let password = b"fixture signing password\n";
    for mode in [
        "success",
        "sign-failure",
        "unlock-failure",
        "open-failure",
        "bad-argc",
    ] {
        let mut child = Command::new(&binary)
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if mode != "bad-argc" {
            let mut input = child.stdin.take().unwrap();
            input
                .write_all(&(password.len() as u32).to_be_bytes())
                .unwrap();
            input.write_all(password).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{mode}: {stderr}");
        assert!(!stderr.contains("fixture signing password"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture signing password"));
    }
    fs::remove_dir_all(directory).unwrap();
}

const STUBS: &str = r#"
#include <arpa/inet.h>
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define MAX_SECRET_BYTES 512
typedef void *SecKeychainRef;
typedef int OSStatus;
enum { errSecSuccess = 0 };
static const char *mode;
static int opened, unlocked, signed_app, locked, released, wiped;
static const char *keychain_path = "/private/a keychain.keychain-db";
static void *secret;
static size_t secret_size;

static void *tracked_calloc(size_t count, size_t size) {
    secret_size = count * size;
    secret = calloc(count, size);
    return secret;
}
static void tracked_free(void *value) {
    assert(value == secret);
    for (size_t index = 0; index < secret_size; index++) {
        assert(((unsigned char *)value)[index] == 0);
    }
    wiped++;
    free(value);
}
static OSStatus SecKeychainOpen(const char *path, SecKeychainRef *keychain) {
    opened++;
    assert(strcmp(path, keychain_path) == 0);
    if (strcmp(mode, "open-failure") == 0) return -1;
    *keychain = (void *)1;
    return errSecSuccess;
}
static OSStatus SecKeychainUnlock(SecKeychainRef keychain, uint32_t length, const void *password, bool use_password) {
    unlocked++;
    assert(keychain == (void *)1 && use_password);
    assert(length == strlen("fixture signing password\n"));
    assert(memcmp(password, "fixture signing password\n", length) == 0);
    return strcmp(mode, "unlock-failure") == 0 ? -2 : errSecSuccess;
}
static void SecKeychainLock(SecKeychainRef keychain) {
    assert(keychain == (void *)1);
    locked++;
}
static void CFRelease(SecKeychainRef keychain) {
    assert(keychain == (void *)1);
    released++;
}
static int security_failure(const char *step, OSStatus status) {
    fprintf(stderr, "%s:%d\n", step, status);
    return 1;
}
static int run_tool(const char *tool, char **arguments) {
    const char *expected[] = {
        "/usr/bin/codesign", "--force", "--sign",
        "0123456789012345678901234567890123456789", "--keychain",
        "/private/a keychain.keychain-db",
        "--preserve-metadata=identifier,entitlements,requirements,flags,runtime",
        "--generate-entitlement-der", "/private/debug app.app", NULL
    };
    assert(strcmp(tool, expected[0]) == 0);
    size_t index = 0;
    for (; expected[index] != NULL; index++) {
        assert(arguments[index] != NULL);
        assert(strcmp(arguments[index], expected[index]) == 0);
        assert(strstr(arguments[index], "fixture signing password") == NULL);
    }
    assert(arguments[index] == NULL);
    signed_app++;
    return strcmp(mode, "sign-failure") != 0;
}
#define calloc tracked_calloc
#define free tracked_free
"#;

const MAIN: &str = r#"
int main(int argc, char **argv) {
    assert(argc == 2);
    mode = argv[1];
    char *arguments[] = {
        "helper", "--device-resign", "/private/a keychain.keychain-db",
        "0123456789012345678901234567890123456789", "/private/debug app.app", NULL
    };
    int bad_argc = strcmp(mode, "bad-argc") == 0;
    int open_failure = strcmp(mode, "open-failure") == 0;
    int unlock_failure = strcmp(mode, "unlock-failure") == 0;
    int result = run_device_resign(bad_argc ? 4 : 5, arguments);
    assert((result == 0) == (strcmp(mode, "success") == 0));
    assert(opened == !bad_argc);
    assert(unlocked == (!bad_argc && !open_failure));
    assert(signed_app == (!bad_argc && !open_failure && !unlock_failure));
    assert(locked == (!bad_argc && !open_failure));
    assert(released == locked);
    assert(wiped == !bad_argc);
    return 0;
}
"#;
