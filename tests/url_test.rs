//! Public round-trip tests for mesh:// URL parsing.

// The main module is not exported as a library, so re-exercise parsing
// at the CLI level via assert_cmd for smoke-checking the binary's arg
// validation. Pure URL parsing is unit-tested inside `src/url.rs`.

use assert_cmd::Command;

#[test]
fn missing_url_prints_usage() {
    let mut cmd = Command::cargo_bin("git-remote-mesh").unwrap();
    cmd.arg("origin"); // remote name but no URL
    let output = cmd.assert().failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).to_string();
    assert!(
        stderr.contains("usage:"),
        "expected usage hint, got: {stderr}"
    );
}

#[test]
fn bad_scheme_rejected() {
    let mut cmd = Command::cargo_bin("git-remote-mesh").unwrap();
    cmd.arg("origin").arg("https://example.com/foo");
    let output = cmd.assert().failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).to_string();
    assert!(
        stderr.contains("invalid scheme") || stderr.contains("invalid mesh URL"),
        "expected scheme complaint, got: {stderr}"
    );
}
