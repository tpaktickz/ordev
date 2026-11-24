//! `openrevo-ec-helper` — the narrow privileged end of the EC write path.
//!
//! The daemon runs unprivileged. On a machine without `tongfang-mifs-wmi` and
//! without a writable `ec_sys` node, the only way to reach a bank other than 0
//! is the ACPI EC command/data port pair, which needs `CAP_SYS_RAWIO`. Rather
//! than ship a setuid binary, the daemon can call this helper through `pkexec`;
//! polkit decides whether a prompt is due and remembers the answer briefly.
//!
//! Deliberately dumb: it accepts a bounded list of byte writes on stdin, does
//! them in order, and prints a single result line. No paths, no shell, no
//! arbitrary register tricks, no reads of anything the caller names beyond the
//! EC address space. Every request is validated before the first port access.
//!
//! Usage:
//!   openrevo-ec-helper write          # stdin: one "w <addr16> <value8>" per line
//!   openrevo-ec-helper read <addr16>  # prints the byte as hex
//!   openrevo-ec-helper probe          # prints whether a channel exists

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("openrevo-ec-helper is a Linux-only component");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    use std::io::{Read as _, Write as _};

    // stdout is the response channel here: `OK <n>` / `ERR ...` / a hex byte.
    // Log lines go to stderr so callers can parse stdout blindly.
    openrevo_core::system::logger::set_stderr(true);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(|s| s.as_str()).unwrap_or("probe");

    match mode {
        "probe" => {
            let present = openrevo_core::pal::ec::available();
            let writable = openrevo_core::pal::ec::write_supported();
            println!("channel present={} writable={}", present, writable);
            std::process::exit(if present { 0 } else { 1 });
        }
        "read" => {
            let addr = match args.get(1).and_then(|s| parse_addr(s)) {
                Some(a) => a,
                None => {
                    eprintln!("usage: openrevo-ec-helper read <addr16>");
                    std::process::exit(2);
                }
            };
            match openrevo_core::pal::ec::read(addr) {
                Some(v) => {
                    println!("{:02x}", v);
                    std::process::exit(0);
                }
                None => {
                    eprintln!("read failed");
                    std::process::exit(1);
                }
            }
        }
        "write" => {
            let mut input = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("cannot read request: {}", e);
                std::process::exit(2);
            }
            let ops = match parse_ops(&input) {
                Ok(ops) => ops,
                Err(e) => {
                    eprintln!("rejected request: {}", e);
                    std::process::exit(2);
                }
            };
            let mut done = 0usize;
            for (addr, val) in ops {
                if !openrevo_core::pal::ec::write(addr, val) {
                    println!("ERR write failed at {:#06x}", addr);
                    std::io::stdout().flush().ok();
                    std::process::exit(1);
                }
                done += 1;
            }
            println!("OK {}", done);
            std::process::exit(0);
        }
        other => {
            eprintln!("unknown mode {other:?}; expected probe|read|write");
            std::process::exit(2);
        }
    }
}

/// Upper bound on a single request. A mode switch or a fan curve is a handful of
/// bytes; anything larger is either a bug or an attempt to use this helper as a
/// general purpose port writer.
#[cfg(target_os = "linux")]
const MAX_OPS: usize = 64;

#[cfg(target_os = "linux")]
fn parse_addr(s: &str) -> Option<u16> {
    let s = s.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u16::from_str_radix(s, 16).ok()
}

#[cfg(target_os = "linux")]
fn parse_ops(input: &str) -> Result<Vec<(u16, u8)>, String> {
    let mut ops = Vec::new();
    for (lineno, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("");
        if !tag.eq_ignore_ascii_case("w") {
            return Err(format!("line {}: expected 'w <addr> <value>'", lineno + 1));
        }
        let addr = parts
            .next()
            .and_then(parse_addr)
            .ok_or_else(|| format!("line {}: bad address", lineno + 1))?;
        let val = parts
            .next()
            .and_then(|v| u8::from_str_radix(v.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| format!("line {}: bad value (expected hex byte)", lineno + 1))?;
        if parts.next().is_some() {
            return Err(format!("line {}: trailing tokens", lineno + 1));
        }
        ops.push((addr, val));
        if ops.len() > MAX_OPS {
            return Err(format!("more than {} writes in one request", MAX_OPS));
        }
    }
    if ops.is_empty() {
        return Err("no writes requested".into());
    }
    Ok(ops)
}
