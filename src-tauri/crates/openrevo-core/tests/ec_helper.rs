//! Contract tests for the privileged EC helper.
//!
//! The helper is the only component that runs as root, so its input validation
//! is part of the security surface: anything malformed must be rejected before
//! the first port access, and the rejection must be a distinct exit code so the
//! calling side can tell "rejected" (2) from "hardware said no" (1).
//!
//! Nothing here touches real hardware: requests either fail validation or run
//! against whatever EC the machine happens to have, and the assertions stay
//! true in both cases.

use std::io::Write;
use std::process::{Command, Stdio};

const HELPER: &str = env!("CARGO_BIN_EXE_openrevo-ec-helper");

fn run(args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(HELPER)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("helper must be runnable");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .ok();
    let out = child.wait_with_output().expect("helper must exit");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

#[test]
fn probe_reports_a_channel_field() {
    let (code, stdout, _) = run(&["probe"], "");
    assert!(
        stdout.starts_with("channel present="),
        "unexpected probe output: {stdout:?}"
    );
    assert!(code == 0 || code == 1, "unexpected probe exit code {code}");
}

#[test]
fn unknown_mode_is_rejected() {
    let (code, _, stderr) = run(&["frobnicate"], "");
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown mode"), "stderr: {stderr}");
}

#[test]
fn read_requires_a_hex_address() {
    let (code, _, stderr) = run(&["read", "not-an-address"], "");
    assert_eq!(code, 2);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}

#[test]
fn malformed_write_line_is_rejected() {
    for bad in ["x 1 2", "w zz 11", "w 0700", "w 0700 11 22", "w 0700 qq"] {
        let (code, _, stderr) = run(&["write"], &format!("{bad}\n"));
        assert_eq!(code, 2, "input {bad:?} should be rejected");
        assert!(
            stderr.contains("rejected request"),
            "input {bad:?} gave stderr {stderr:?}"
        );
    }
}

#[test]
fn empty_request_is_rejected() {
    let (code, _, stderr) = run(&["write"], "\n# only a comment\n");
    assert_eq!(code, 2);
    assert!(stderr.contains("no writes requested"), "stderr: {stderr}");
}

#[test]
fn oversized_request_is_rejected() {
    // MAX_OPS is 64; the 65th write must be refused before any port access.
    let payload: String = (0..65).map(|i| format!("w {:04x} 00\n", i)).collect();
    let (code, _, stderr) = run(&["write"], &payload);
    assert_eq!(code, 2, "65 writes should be rejected");
    assert!(stderr.contains("more than 64 writes"), "stderr: {stderr}");
}

#[test]
fn valid_request_reaches_the_hardware_and_reports_one_line() {
    let (code, stdout, _) = run(&["write"], "w 0700 2a\n");
    assert!(code == 0 || code == 1, "unexpected exit code {code}");
    if code == 0 {
        assert_eq!(stdout, "OK 1");
    } else {
        assert!(
            stdout.starts_with("ERR write failed at 0x0700"),
            "stdout: {stdout}"
        );
    }
}
