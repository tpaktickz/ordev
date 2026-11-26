//! Verifies the Linux brightness path end-to-end (session-bus first, sysfs fallback).
//!
//! Run as the desktop user, not root: the point is to prove an unprivileged
//! process can drive the panel backlight on Wayland.

use std::thread;
use std::time::Duration;

use openrevo_core::system::display;

fn main() {
    println!("== brightness ==");
    println!("initial get_brightness() => {:?}", display::get_brightness());
    let _ = std::fs::read_to_string("/sys/class/backlight/intel_backlight/brightness")
        .map(|v| println!("sysfs intel_backlight brightness = {}", v.trim()));

    for level in [40u8, 90, 55] {
        match display::set_brightness(level) {
            Ok(()) => println!("set_brightness({level}) => ok"),
            Err(e) => println!("set_brightness({level}) => ERR {e}"),
        }
        thread::sleep(Duration::from_millis(400));
        let sysfs = std::fs::read_to_string("/sys/class/backlight/intel_backlight/brightness")
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        println!(
            "  readback: api={:?} sysfs={}",
            display::get_brightness(),
            sysfs
        );
    }

    println!("\n== turbo submode ==");
    println!(
        "chassis tier = {:?}",
        openrevo_core::hal::platform_probe::get_chassis_architecture_tier()
    );
    println!(
        "nextgen_oem_magic_ec = {}",
        openrevo_core::driver::wmi_ec::is_nextgen_oem_magic_ec()
    );
    println!(
        "turbo_submode::is_supported() = {}",
        openrevo_core::system::power::turbo_submode::is_supported()
    );
    println!(
        "turbo_submode::current_silent() = {:?}",
        openrevo_core::system::power::turbo_submode::current_silent()
    );
    println!(
        "raw ADDR_TURBO_SUBMODE = {:?}",
        openrevo_core::driver::wmi_ec::wmi_read_ec(openrevo_core::hal::vault::ec::ADDR_TURBO_SUBMODE)
    );
}
