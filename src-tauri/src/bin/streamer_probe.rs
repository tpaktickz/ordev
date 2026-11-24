//! ITE 8291 keyboard-LED diagnostic.
//!
//! Drives the per-key RGB controller directly, bypassing the GUI, the service and
//! IPC: it finds the keyboard's MI_01 HID endpoint, runs the BetterRGB 0x33
//! handshake and streams a five second rainbow. If the keyboard animates, the
//! report layer works end to end.
//!
//! The Win32 path uses `SetupDi*` + `HidD_SetFeature` + overlapped writes; the
//! Linux path uses `/sys/class/hidraw` for enumeration and `/dev/hidrawN` with
//! `HIDIOCSFEATURE` plus plain writes, which is the same HID contract.

#[cfg(windows)]
include!("streamer_probe/impl_win.rs");

#[cfg(not(windows))]
mod linux_probe {
    use std::fs::{self, File};
    use std::io::Write;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// ITE vendor id and the two product ids known to speak the 0x33 protocol.
    const ITE_VID: &str = "0000048D";
    const ITE_PIDS: [&str; 2] = ["0000600B", "0000CE00"];
    const HIDIOCSFEATURE_BASE: libc::c_ulong = 0x4004_4806;

    fn hsv_to_rgb(hue: f64, s: f64, v: f64) -> (u8, u8, u8) {
        let h = ((hue % 360.0) + 360.0) % 360.0;
        let c = v * s;
        let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
        let m = v - c;
        let (r1, g1, b1) = if h < 60.0 {
            (c, x, 0.0)
        } else if h < 120.0 {
            (x, c, 0.0)
        } else if h < 180.0 {
            (0.0, c, x)
        } else if h < 240.0 {
            (0.0, x, c)
        } else if h < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };
        (
            ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
            ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
            ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        )
    }

    /// Finds the ITE MI_01 hidraw node. Returns (node, matches_vendor_product).
    fn find_endpoint() -> Option<String> {
        let entries = fs::read_dir("/sys/class/hidraw").ok()?;
        for entry in entries.flatten() {
            let node = entry.file_name().to_string_lossy().to_string();
            let uevent = fs::read_to_string(entry.path().join("device/uevent")).unwrap_or_default();
            let hid_id = uevent
                .lines()
                .find_map(|line| line.strip_prefix("HID_ID="))
                .unwrap_or("");
            // HID_ID = bus:vendor:product in hex, bus 0003 = USB.
            let mut parts = hid_id.split(':');
            let _bus = parts.next();
            let vendor = parts.next().unwrap_or("");
            let product = parts.next().unwrap_or("");
            if vendor.eq_ignore_ascii_case(ITE_VID) && ITE_PIDS.iter().any(|p| product.eq_ignore_ascii_case(p)) {
                return Some(format!("/dev/hidraw{}", node.trim_start_matches("hidraw")));
            }
        }
        None
    }

    fn set_feature(file: &File, report: &[u8; 9]) -> bool {
        let fd = std::os::fd::AsRawFd::as_raw_fd(file);
        let request = HIDIOCSFEATURE_BASE | ((report.len() as libc::c_ulong) << 16);
        unsafe { libc::ioctl(fd, request as _, report.as_ptr()) >= 0 }
    }

    fn write_output(file: &mut File, chunk: &[u8; 65]) -> bool {
        file.write_all(chunk).is_ok()
    }

    pub fn main() {
        println!("=================================================================");
        println!("   OpenRevo - BetterRGB ITE 8291 Standalone Minimal Diagnostic   ");
        println!("=================================================================");
        println!("This tool bypasses OpenRevo GUI, services, and IPC.");
        println!("It directly tests ITE 8291 USB HID communication and pushes a 5s RGB Rainbow.\n");

        println!("[1/4] Scanning hidraw nodes for an ITE 8291 endpoint...");
        let Some(path) = find_endpoint() else {
            eprintln!(
                "\n[FATAL] No ITE 8291 hidraw node found (VID 048D, PID 600B/CE00).\n\
                 Check `ls /dev/hidraw*` and `grep -H HID_ID /sys/class/hidraw/*/device/uevent`."
            );
            return;
        };
        println!("  -> Candidate: {}", path);

        let mut file = match File::options().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!(
                    "\n[FATAL] Cannot open {}: {}\n\
                     /dev/hidraw* is root-only by default; run with sudo or install the udev rule.",
                    path, err
                );
                return;
            }
        };
        println!("\n[2/4] Opened target device: {}", path);

        println!("\n[3/4] Executing BetterRGB 0x33 Handshake...");
        let step1: [u8; 9] = [0x00, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00];
        let ok1 = set_feature(&file, &step1);
        println!("  Step 1 SetFeature [0x12, 0x03]: result={}", ok1);
        sleep(Duration::from_millis(20));

        let clear_chunk: [u8; 65] = [0; 65];
        let ok_clear = write_output(&mut file, &clear_chunk);
        println!("  Step Clear Frame chunk: result={}", ok_clear);
        sleep(Duration::from_millis(20));

        let step2: [u8; 9] = [0x00, 0x08, 0x02, 0x33, 0x00, 0x32, 0x00, 0x00, 0x00];
        let ok2 = set_feature(&file, &step2);
        println!("  Step 2 SetFeature [0x08, 0x02, 0x33]: result={}", ok2);
        sleep(Duration::from_millis(30));

        if !ok1 || !ok2 {
            eprintln!("[WARNING] Handshake failed on step1 or step2!");
        } else {
            println!("  >>> Handshake SUCCESS! MCU entered 0x33 Per-Key Mode.");
        }

        println!("\n[4/4] Streaming smooth 60FPS Rainbow Wave for 5 seconds...");
        println!("Look at your physical keyboard right now! Does it light up with animated rainbow?\n");

        let step3: [u8; 9] = [0x00, 0x12, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00];
        let start_time = Instant::now();
        let duration = Duration::from_secs(5);
        let mut frame_count = 0u32;
        let mut t: f64 = 0.0;

        while start_time.elapsed() < duration {
            let frame_start = Instant::now();

            if !set_feature(&file, &step3) {
                eprintln!("  [WARN] Step 3 SetFeature failed at frame #{}", frame_count);
            }

            let mut buf = [0u8; 512];
            for i in 0..126 {
                let offset = i * 4;
                if offset + 2 < 512 {
                    let hue = (t + (i as f64) * 8.0) % 360.0;
                    let (r, g, b) = hsv_to_rgb(hue, 1.0, 1.0);
                    buf[offset] = r;
                    buf[offset + 1] = g;
                    buf[offset + 2] = b;
                }
            }

            for chunk_idx in 0..8 {
                let mut chunk = [0u8; 65];
                chunk[1..65].copy_from_slice(&buf[chunk_idx * 64..(chunk_idx + 1) * 64]);
                let _ = write_output(&mut file, &chunk);
            }

            frame_count += 1;
            t += 8.0;

            if frame_count % 30 == 0 {
                print!(
                    "\r  -> Streaming active: frame #{} (Elapsed: {:.1}s)   ",
                    frame_count,
                    start_time.elapsed().as_secs_f32()
                );
                let _ = std::io::stdout().flush();
            }

            let elapsed_ms = frame_start.elapsed().as_millis() as u64;
            if elapsed_ms < 16 {
                sleep(Duration::from_millis(16 - elapsed_ms));
            }
        }

        println!("\n\n[DONE] 5-second streaming completed (Total frames: {}).", frame_count);
        println!("Diagnostic finished. If the keyboard illuminated with a fluid rainbow wave, the report layer is functional.");
    }
}

#[cfg(not(windows))]
fn main() {
    linux_probe::main();
}
