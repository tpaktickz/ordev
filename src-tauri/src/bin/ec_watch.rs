//! Long-running EC sampler used to explain spontaneous fan ramps.
//!
//! Prints one CSV line every 2 s: mode byte, trigger, clamp, turbo submode, PL1,
//! cTGP, battery limit, both fan RPMs, CPU/GPU temperatures and the PWM duty bytes.
//! Usage: `ec_watch [seconds] [lean]` (default 600). Pass `lean` to sample only
//! the fan duty/RPM plus the host CPU temperature, which keeps the EC mailbox
//! quiet enough that the sampler is not itself part of what it measures.

use std::thread;
use std::time::Duration;

use openrevo_core::driver::wmi_ec as ec;

fn rpm(b1: u16, b2: u16) -> Option<u16> {
    let hi = ec::wmi_read_ec(b1)? as u16;
    let lo = ec::wmi_read_ec(b2)? as u16;
    let v = (hi << 8) | lo;
    if v > 8000 {
        None
    } else {
        Some(v)
    }
}

fn temp(reg: u16) -> Option<i32> {
    ec::wmi_read_ec(reg).map(|v| v as i32)
}

/// Host CPU package temperature via hwmon, so the fan curve can be correlated
/// with the sensor the EC itself uses without another EC round-trip.
fn host_cpu_temp() -> Option<f64> {
    let mut best: Option<f64> = None;
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let name = std::fs::read_to_string(entry.path().join("name"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !matches!(name.as_str(), "coretemp" | "k10temp" | "zenpower") {
            continue;
        }
        let inputs = std::fs::read_dir(entry.path()).ok()?;
        for input in inputs.flatten() {
            let fname = input.file_name().to_string_lossy().to_string();
            if !fname.starts_with("temp") || !fname.ends_with("_input") {
                continue;
            }
            let label = std::fs::read_to_string(entry.path().join(fname.replace("_input", "_label")))
                .unwrap_or_default();
            if !label.trim().is_empty() && label.trim() != "Package id 0" {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(input.path()) {
                if let Ok(v) = raw.trim().parse::<f64>() {
                    let c = v / 1000.0;
                    best = Some(best.map_or(c, |b: f64| b.max(c)));
                }
            }
        }
    }
    best
}

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(600);
    let lean = std::env::args().any(|a| a == "lean");
    if lean {
        println!("t,table_ctrl,cpu_temp_c,cpu_rpm,gpu_rpm,duty_l,duty_r");
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < secs {
            let ctrl = ec::wmi_read_ec(0x0F5F).map(|v| format!("{v:#04x}"));
            let duty_l = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_L_DUTY_BYTE).map(|v| format!("{v:#04x}"));
            let duty_r = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_R_DUTY_BYTE).map(|v| format!("{v:#04x}"));
            let c = rpm(ec::ADDR_MAIN_FAN_RPM_B1, ec::ADDR_MAIN_FAN_RPM_B2);
            let g = rpm(ec::ADDR_SECOND_FAN_RPM_B1, ec::ADDR_SECOND_FAN_RPM_B2);
            println!(
                "{},{},{:?},{:?},{:?},{:?},{:?}",
                start.elapsed().as_secs(),
                ctrl.as_ref().map(|s| s.as_str()).unwrap_or("-"),
                host_cpu_temp(),
                c,
                g,
                duty_l.as_ref().map(|s| s.as_str()).unwrap_or("-"),
                duty_r.as_ref().map(|s| s.as_str()).unwrap_or("-"),
            );
            thread::sleep(Duration::from_secs(2));
        }
        return;
    }
    println!("t,mode,trig,clamp,submode,pl1,pl2,pl4,tgp,cpu_rpm,gpu_rpm,duty_l,duty_r,ac");
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < secs {
        let line = format!(
            "{},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?}",
            start.elapsed().as_secs(),
            ec::wmi_read_ec(ec::ADDR_MODE_INDEX).map(|v| format!("{v:#04x}")),
            ec::wmi_read_ec(ec::ADDR_TRIGGER_BYTE).map(|v| format!("{v:#04x}")),
            ec::wmi_read_ec(ec::ADDR_FAN_BOOST_CLAMP).map(|v| format!("{v:#04x}")),
            ec::wmi_read_ec(ec::ADDR_TURBO_SUBMODE).map(|v| format!("{v:#04x}")),
            temp(ec::ADDR_CUSTOM_PL1),
            temp(ec::ADDR_CUSTOM_PL2),
            temp(ec::ADDR_CUSTOM_PL4),
            temp(ec::ADDR_TGP_OFFSET),
            rpm(ec::ADDR_MAIN_FAN_RPM_B1, ec::ADDR_MAIN_FAN_RPM_B2),
            rpm(ec::ADDR_SECOND_FAN_RPM_B1, ec::ADDR_SECOND_FAN_RPM_B2),
            ec::wmi_read_ec(ec::ADDR_MAIN_FAN_L_DUTY_BYTE).map(|v| format!("{v:#04x}")),
            ec::wmi_read_ec(ec::ADDR_MAIN_FAN_R_DUTY_BYTE).map(|v| format!("{v:#04x}")),
            ec::wmi_read_ec(ec::ADDR_AC_RECOVERY).map(|v| format!("{v:#04x}")),
        );
        println!("{line}");
        thread::sleep(Duration::from_secs(2));
    }
}
