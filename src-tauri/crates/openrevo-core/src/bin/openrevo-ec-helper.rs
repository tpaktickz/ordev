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
//! The `serve` mode is the one the app actually talks to: it holds a unix
//! socket open so reads cost a round trip instead of a process, which matters
//! because telemetry reads several registers a second. Socket group and mode do
//! the authorisation - connecting already requires write permission on
//! `openrevo-ec.sock`, which only members of `openrevo-ec` and root have.
//!
//! Usage:
//!   openrevo-ec-helper serve [socket]  # long-running, socket defaults to /run/openrevo/ec.sock
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
        "serve" => {
            let socket = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| DEFAULT_SOCKET.to_string());
            if let Err(e) = serve(&socket) {
                eprintln!("cannot serve on {}: {}", socket, e);
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("unknown mode {other:?}; expected probe|read|write|serve");
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

/// Where the app looks for the broker. Matches `BROKER_SOCKET` in `pal::ec`.
#[cfg(target_os = "linux")]
const DEFAULT_SOCKET: &str = "/run/openrevo/ec.sock";

/// The group that may talk to the broker. Root always may.
#[cfg(target_os = "linux")]
const SOCKET_GROUP: &str = "openrevo-ec";

#[cfg(target_os = "linux")]
fn gid_of(name: &str) -> Option<u32> {
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    groups.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next() != Some(name) {
            return None;
        }
        fields.next()?;
        fields.next()?.parse().ok()
    })
}

#[cfg(target_os = "linux")]
fn serve(socket: &str) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    use std::os::unix::net::UnixListener;

    if let Some(dir) = std::path::Path::new(socket).parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o755).create(dir)?;
    }
    // A socket left behind by a crashed run would otherwise block the bind.
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    // We are the broker now: our own readers must use the privileged transports
    // directly, never the socket, or every request recurses into itself.
    openrevo_core::pal::ec::linux::disable_broker_client();
    // Connecting is the authorisation check, so the mode is what counts here.
    // No group on the machine means root-only; that is a safe default.
    // The unit runs with this group, so the socket is born with it; only root
    // can chown and dropping CAP_CHOWN from the bounding set costs more than it
    // buys. A standalone run outside the service keeps the caller's group.
    if gid_of(SOCKET_GROUP).is_none() {
        eprintln!(
            "warning: group {} does not exist (systemd-sysusers should create it); \
             the socket stays in the caller's group",
            SOCKET_GROUP
        );
    }
    std::fs::set_permissions(socket, std::os::unix::fs::PermissionsExt::from_mode(0o660))?;
    println!("serving on {}", socket);

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("accept failed: {}", e);
                continue;
            }
        };
        // One thread per connection: a client may hold the socket for hours,
        // and a slow client must not stall the next one. Concurrent port
        // transactions are serialised inside pal::ec, not here.
        std::thread::spawn(move || {
            let _ = handle_client(stream);
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn handle_client(mut stream: std::os::unix::net::UnixStream) -> std::io::Result<()> {
    use std::io::{BufRead as _, BufReader, Write as _};

    let reader = BufReader::new(stream.try_clone()?);
    for line in reader.lines() {
        let line = line?;
        let reply = handle_request(line.trim());
        stream.write_all(reply.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    Ok(())
}

/// One request line in, one response line out. Anything malformed answers
/// `err <text>` rather than closing the connection, so a client only has to
/// resynchronise on its own bugs.
#[cfg(target_os = "linux")]
fn handle_request(line: &str) -> String {
    use openrevo_core::pal::ec;
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("r") => {
            let Some(addr) = parts.next().and_then(parse_addr) else {
                return "err bad address".into();
            };
            if parts.next().is_some() {
                return "err trailing tokens".into();
            }
            match ec::read(addr) {
                Some(val) => format!("v {:04x} {:02x}", addr, val),
                None => format!("n {:04x}", addr),
            }
        }
        Some("w") => {
            let mut ops = Vec::new();
            while let Some(addr) = parts.next().and_then(parse_addr) {
                let Some(val) = parts
                    .next()
                    .and_then(|v| u8::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                else {
                    return format!("err bad value for {addr:#06x}");
                };
                ops.push((addr, val));
                if ops.len() > MAX_OPS {
                    return format!("err more than {MAX_OPS} writes");
                }
            }
            if ops.is_empty() {
                return "err no writes".into();
            }
            for (addr, val) in ops {
                if !ec::write(addr, val) {
                    return format!("err write {:#06x} failed", addr);
                }
            }
            "ok".into()
        }
        Some("probe") => format!(
            "p present={} writable={}",
            ec::available(),
            ec::write_supported()
        ),
        Some(other) => format!("err unknown request {other:?}"),
        None => "err empty".into(),
    }
}
