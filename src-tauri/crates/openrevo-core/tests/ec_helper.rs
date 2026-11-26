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

/// Mirrors `MAX_OPS` in `src/bin/openrevo-ec-helper.rs`. One fan-table commit is
/// 96 duty + threshold writes, so the cap is 192; keep the two in sync.
const MAX_OPS: usize = 192;

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
    // The write after MAX_OPS must be refused before any port access.
    let payload: String = (0..=MAX_OPS).map(|i| format!("w {:04x} 00\n", i)).collect();
    let (code, _, stderr) = run(&["write"], &payload);
    assert_eq!(code, 2, "{} writes should be rejected", MAX_OPS + 1);
    assert!(
        stderr.contains(&format!("more than {MAX_OPS} writes")),
        "stderr: {stderr}"
    );
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

// ---- broker (`serve`) -------------------------------------------------------

/// A running broker for the duration of one test. The child never exits on its
/// own, so dropping the guard is what stops it.
struct Broker {
    child: std::process::Child,
    socket: std::path::PathBuf,
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Broker {
    /// Starts a broker on a socket in its own temporary directory.
    fn start(tag: &str) -> Option<Broker> {
        let dir = std::env::temp_dir().join(format!("openrevo-ec-test-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let socket = dir.join("ec.sock");
        let child = Command::new(HELPER)
            .args(["serve", socket.to_str()?])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let broker = Broker { child, socket };
        // The listener binds before the first line of stdout, but the file only
        // becomes connectable once bind() has run, so wait for it.
        for _ in 0..200 {
            if broker.socket.exists() {
                return Some(broker);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    /// One request line, one reply line.
    fn ask(&self, line: &str) -> String {
        use std::io::{BufRead as _, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(&self.socket)
            .expect("broker must accept connections");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
        let mut writer = stream.try_clone().expect("socket clone");
        writer.write_all(format!("{line}\n").as_bytes()).expect("request");
        writer.flush().ok();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).expect("reply");
        reply.trim().to_string()
    }
}

#[test]
fn broker_answers_probe() {
    let Some(broker) = Broker::start("probe") else {
        eprintln!("cannot start the broker here; skipping");
        return;
    };
    let reply = broker.ask("probe");
    assert!(
        reply.starts_with("p present="),
        "unexpected probe reply: {reply:?}"
    );
}

#[test]
fn broker_rejects_malformed_requests_without_dying() {
    let Some(broker) = Broker::start("reject") else {
        eprintln!("cannot start the broker here; skipping");
        return;
    };
    for bad in ["r", "r zz", "r 0700 11", "w", "w 0700", "frobnicate", ""] {
        let reply = broker.ask(bad);
        assert!(
            reply.starts_with("err"),
            "request {bad:?} should be refused, got {reply:?}"
        );
    }
    // Still serving after all of that.
    assert!(broker.ask("probe").starts_with("p present="));
}

#[test]
fn broker_bounds_a_single_write_batch() {
    let Some(broker) = Broker::start("batch") else {
        eprintln!("cannot start the broker here; skipping");
        return;
    };
    // One past the helper's cap; the address range does not matter because the
    // request is refused before any of them is touched.
    let mut line = String::from("w");
    for _ in 0..=MAX_OPS {
        line.push_str(" 0000 00");
    }
    let reply = broker.ask(&line);
    assert!(
        reply.starts_with(&format!("err more than {MAX_OPS}")),
        "oversized batch should be refused, got {reply:?}"
    );
}

#[test]
fn broker_reads_are_stable_across_two_calls() {
    let Some(broker) = Broker::start("read2") else {
        eprintln!("cannot start the broker here; skipping");
        return;
    };
    // Whatever the machine is, the same register read twice in a row must give
    // the same shape of answer: either a byte or an explicit "unavailable".
    let first = broker.ask("r 0740");
    let second = broker.ask("r 0740");
    assert_eq!(
        first.split_whitespace().next(),
        second.split_whitespace().next(),
        "register 0x0740 answered differently: {first:?} then {second:?}"
    );
    assert!(
        first.starts_with("v ") || first.starts_with("n "),
        "unexpected read reply: {first:?}"
    );
}
