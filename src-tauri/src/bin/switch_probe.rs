//! Exercises the panel's quick hardware switches through the exact code path the
//! Slint UI uses (`system::switches::set_switch` / `get_switches_state`), so a
//! switch that silently does nothing shows up here instead of in the UI.
//!
//! Usage:
//!   switch_probe                 read state only
//!   switch_probe camera mic ...  toggle the named switches twice (off, on) and
//!                                 report the state the hardware reports back
//!   switch_probe --hit <addr> <hex>   raw EC write used to prove a bit moved

use std::thread;
use std::time::Duration;

use open_revo_lib::system::switches;

fn snapshot(tag: &str) {
    let s = switches::get_switches_state();
    println!(
        "{tag}: camera={} mic={} touchpad={} fn_lock={} win_key={} bluetooth={} wifi={} keep_awake={} osd={}",
        s.camera, s.mic, s.touchpad, s.fn_lock, s.win_key, s.bluetooth, s.wifi, s.keep_awake, s.osd
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() >= 2 && args[0] == "--write" {
        let addr = u16::from_str_radix(args[1].trim_start_matches("0x"), 16).unwrap_or(0);
        let val = u8::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap_or(0);
        let ok = open_revo_lib::driver::wmi_ec::wmi_write_ec(addr, val);
        println!(
            "write {addr:#06x}={val:#04x} ok={ok} readback={:?}",
            open_revo_lib::driver::wmi_ec::wmi_read_ec(addr)
        );
        return;
    }
    if args.len() >= 2 && args[0] == "--read" {
        let addr = u16::from_str_radix(args[1].trim_start_matches("0x"), 16).unwrap_or(0);
        println!(
            "{addr:#06x} = {:?}",
            open_revo_lib::driver::wmi_ec::wmi_read_ec(addr)
        );
        return;
    }

    if args.len() == 1 && args[0] == "--cam" {
        println!(
            "camera: cut_off={:?} state(true)={} state(false)={} raw727={:?}",
            open_revo_lib::system::switches::camera::ec_camera_cut_off(),
            open_revo_lib::system::switches::camera::get_realtime_camera_state(true, true),
            open_revo_lib::system::switches::camera::get_realtime_camera_state(false, false),
            open_revo_lib::driver::wmi_ec::wmi_read_ec(open_revo_lib::driver::wmi_ec::ADDR_CAMERA_CUTOFF)
        );
        return;
    }
    if args.len() == 1 && args[0] == "--bt" {
        let (path, powered) = open_revo_lib::system::switches::radio::bluez_debug();
        println!("bluez: adapter={path:?} powered={powered:?}");
        return;
    }

    let names: Vec<String> = args.iter().filter(|a| !a.starts_with("--")).cloned().collect();
    snapshot("initial");

    for name in &names {
        if name == "wifi" {
            println!("wifi: skipped by operator instruction");
            continue;
        }
        let state_names = switches::get_switches_state();
        let current = match name.as_str() {
            "camera" => state_names.camera,
            "mic" => state_names.mic,
            "touchpad" => state_names.touchpad,
            "fn_lock" => state_names.fn_lock,
            "bluetooth" => state_names.bluetooth,
            "keep_awake" => state_names.keep_awake,
            "osd" => state_names.osd,
            "win_key" => state_names.win_key,
            other => {
                println!("{other}: unknown switch");
                continue;
            }
        };

        println!("--- {name}: current={current}");
        for (label, target) in [("off", false), ("on", true)] {
            let started = std::time::Instant::now();
            match switches::set_switch(name, target) {
                Ok(_) => {
                    thread::sleep(Duration::from_millis(600));
                    let after = switches::get_switches_state();
                    let actual = match name.as_str() {
                        "camera" => after.camera,
                        "mic" => after.mic,
                        "touchpad" => after.touchpad,
                        "fn_lock" => after.fn_lock,
                        "bluetooth" => after.bluetooth,
                        "keep_awake" => after.keep_awake,
                        "osd" => after.osd,
                        "win_key" => after.win_key,
                        _ => false,
                    };
                    println!(
                        "    set {label} ({target}) -> state={actual} {} ({} ms)",
                        if actual == target { "OK" } else { "MISMATCH" },
                        started.elapsed().as_millis()
                    );
                }
                Err(err) => println!("    set {label} -> error: {err}"),
            }
        }
    }
    snapshot("final");
}
