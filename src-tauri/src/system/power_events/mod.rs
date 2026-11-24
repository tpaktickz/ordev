//! Power-event pipelines shared by every platform.
//!
//! The *triggers* are platform specific — Windows listens on
//! `WM_POWERBROADCAST`, Linux listens on logind's `PrepareForSleep` and
//! UPower's `LidIsClosed` — but every reaction (degrade to Office mode before
//! sleep, restore the previous mode on resume, re-assert the EC state, stop the
//! water cooler, re-init display tuning) is identical and lives here.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::driver::ec_bridge::SharedEcBridge;

#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

#[cfg(not(windows))]
#[path = "logind.rs"]
mod imp;

#[cfg(windows)]
pub use imp::get_power_watcher_hwnd;

static IS_LISTENING: AtomicBool = AtomicBool::new(false);
static PREV_POWER_MODE: AtomicU8 = AtomicU8::new(0);
static GLOBAL_EC: parking_lot::RwLock<Option<SharedEcBridge>> = parking_lot::RwLock::new(None);

pub fn init_power_event_watcher(ec: SharedEcBridge) {
    *GLOBAL_EC.write() = Some(ec);
    start_power_event_watcher();
}

/// Start the background power watcher thread (idempotent).
pub fn start_power_event_watcher() {
    if IS_LISTENING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        if !imp::start() {
            IS_LISTENING.store(false, Ordering::SeqCst);
        }
    });
}

fn sleep_guard() {
    let cfg = crate::system::config::load_config();
    if cfg.sleep_auto_office {
        let cur_mode = cfg.power_mode;
        if cur_mode != 1 {
            println!("[Sleep Guard] Auto-switching from mode {} to Office Mode (1) before sleep / lid close!", cur_mode);
            PREV_POWER_MODE.store(cur_mode, Ordering::SeqCst);
            crate::system::power_mode::apply_full_power_mode(1);
        }
    }
}

fn wake_guard() {
    let cfg = crate::system::config::load_config();
    if cfg.sleep_auto_office {
        let prev = PREV_POWER_MODE.swap(0, Ordering::SeqCst);
        if prev > 1 && prev <= 4 {
            println!("[Sleep Guard] Waking up! Restoring previous power mode: {}", prev);
            crate::system::power_mode::apply_full_power_mode(prev);
        }
    }
}

/// System is going to sleep / hibernate / lid was closed for good.
pub(super) fn enter_suspend(reason: &str) {
    // If Keep-Awake is active or screen was turned off for overnight tasks,
    // NEVER degrade power mode to Office (1) and NEVER shut down water cooling!
    let is_running_tasks =
        crate::system::switches::is_keep_awake() || crate::system::switches::is_screen_off();
    if is_running_tasks {
        println!("[Power Guard] {} received but Keep-Awake or Screen-Off is ACTIVE. Preserving active power mode and water cooling for background tasks.", reason);
    } else if crate::hal::platform_probe::is_ultrabook() {
        println!("[Power Guard] Ultrabook entering Standby: skipping water cooler & lighting suspension.");
        crate::system::telemetry::set_screen_suspended(true);
        sleep_guard();
    } else {
        println!("[Power Guard] System is entering Sleep / Hibernate / Standby! Stopping water cooler & suspending lighting...");
        crate::system::telemetry::set_screen_suspended(true);
        sleep_guard();
        crate::driver::ble_cooler::get_global_cooler().stop_and_shutdown();
        let streamer = crate::driver::ite_streamer::get_global_streamer();
        streamer.suspend_for_sleep();
        let lighting = crate::driver::ite_lighting::get_global_lighting();
        lighting.turn_off_all();
        lighting.invalidate_devices();
        lighting.is_sleeping.store(true, Ordering::SeqCst);
    }
}

/// Lid closed: same idea as suspend, but the machine may still be awake.
pub(super) fn lid_closed() {
    crate::system::telemetry::set_screen_suspended(true);
    if crate::hal::platform_probe::is_ultrabook() {
        println!("[Power Guard] Ultrabook lid closed! Adjusting power mode only.");
        sleep_guard();
    } else {
        println!("[Power Guard] Laptop lid closed! Switching to office & stopping water cooler...");
        sleep_guard();
        crate::driver::ble_cooler::get_global_cooler().stop_and_shutdown();
        let streamer = crate::driver::ite_streamer::get_global_streamer();
        streamer.suspend_for_sleep();
        let lighting = crate::driver::ite_lighting::get_global_lighting();
        lighting.turn_off_all();
        lighting.invalidate_devices();
        lighting.is_sleeping.store(true, Ordering::SeqCst);
    }
}

/// Full resume pipeline: restore power mode, cooler, lighting and EC state.
pub(super) fn resume_pipeline(event_name: &str) {
    crate::system::telemetry::set_screen_suspended(false);
    if crate::hal::platform_probe::is_ultrabook() {
        println!("[Power Guard] Ultrabook {}! Restoring power mode only.", event_name);
        wake_guard();
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            crate::system::display_tuning::init_display_tuning();
        });
        return;
    }

    println!("[Power Guard] Laptop {}! Restoring power mode, cooler & lighting...", event_name);
    wake_guard();

    std::thread::spawn(|| {
        let cooler = crate::driver::ble_cooler::get_global_cooler();
        cooler.resume_after_wake();
        cooler.start_active_cooling();
    });

    std::thread::spawn(|| {
        crate::driver::ite_lighting::get_global_lighting().resume_after_wake();
        std::thread::sleep(std::time::Duration::from_millis(500));
        crate::system::display_tuning::init_display_tuning();
    });

    // Reassert all active plugin watchers after system resume (e.g. GPU undervolt curves, battery keepers)
    crate::system::watcher_hub::reassert_active_watchers("system_resume");

    let cfg = crate::system::config::load_config();
    crate::system::power::restore_ec_power_options(&cfg);
    if let Some(ec) = GLOBAL_EC.read().clone() {
        let cfg_limit = cfg.battery_limit;
        let cfg_override = cfg.battery_hardware_limit_enabled;
        println!(
            "[Power Guard] System resumed: Re-asserting battery limit ({}%) with override={}",
            cfg_limit, cfg_override
        );
        ec.lock().apply_battery_limit_with_override(cfg_limit, cfg_override);

        // Delayed re-assertion to defend against late ACPI battery driver initialization
        let ec_delayed = ec.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2000));
            let cfg_delayed = crate::system::config::load_config();
            if cfg_delayed.battery_hardware_limit_enabled && cfg_delayed.battery_limit < 95 {
                ec_delayed
                    .lock()
                    .apply_battery_limit_with_override(cfg_delayed.battery_limit, true);
            }
        });
    }

    crate::driver::wmi_ec::wmi_modify_ec_mask(
        crate::driver::wmi_ec::ADDR_FN_LOCK,
        0x10,
        cfg.fn_lock_enabled,
    );
    let is_ultrabook = crate::hal::platform_probe::is_ultrabook();
    let supports_legacy_ec = crate::hal::platform_probe::is_tongfang_family() && !is_ultrabook;
    if supports_legacy_ec {
        let hw_locked = crate::driver::wmi_ec::is_winkey_locked_hw();
        if hw_locked != cfg.win_key_locked {
            crate::driver::wmi_ec::send_winkey_lock_trigger();
        }
    }
    crate::system::hooks::set_win_key_locked(cfg.win_key_locked);
    if !cfg.touchpad_enabled {
        crate::driver::wmi_ec::wmi_modify_ec_mask(
            crate::driver::wmi_ec::ADDR_BATTERY_PROTECTION_MODE,
            crate::driver::wmi_ec::TOUCHPAD_BITMASK,
            false,
        );
        crate::system::switches::touchpad::write_ptp_registry_enabled(false);
    }
    if !cfg.mic_enabled {
        let _ = crate::system::switches::audio::set_microphone_state(false);
    }
    if cfg.keep_awake_enabled {
        crate::system::switches::set_keep_awake(true);
    }
    if !cfg.wifi_enabled {
        let _ = crate::system::switches::radio::set_wifi_radio_state_native(false);
    }
    if !cfg.bluetooth_enabled {
        let _ = crate::system::switches::radio::set_native_bluetooth_radio_state(false);
    }
}

/// Shutdown / restart: leave the hardware in a safe state.
pub(super) fn system_shutdown() {
    println!("[Power Guard] System is shutting down or restarting! Stopping water cooler & lighting...");
    crate::system::telemetry::set_screen_suspended(true);
    crate::system::display_tuning::reset_display_tuning_to_default();
    if !crate::hal::platform_probe::is_ultrabook() {
        crate::driver::ble_cooler::get_global_cooler().stop_and_shutdown();
        crate::driver::ite_streamer::get_global_streamer().suspend_for_sleep();
        let lighting = crate::driver::ite_lighting::get_global_lighting();
        lighting.turn_off_all();
        lighting.invalidate_devices();
    }
}
