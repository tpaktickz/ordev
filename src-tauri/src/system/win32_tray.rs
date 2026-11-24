//! System tray facade.
//!
//! Windows uses a message-only `Shell_NotifyIconW` icon with a Win32 popup
//! menu. Linux uses a freedesktop StatusNotifierItem (ksni), which every
//! supported desktop shell can host without KDE-only plumbing. The menu model
//! and the actions behind it are shared and live here; each backend only
//! renders them.

use crate::driver::ec_bridge::SharedEcBridge;

pub static GLOBAL_EC_BRIDGE: std::sync::OnceLock<SharedEcBridge> = std::sync::OnceLock::new();

/// Kept for the single-instance wake path; on Linux it is just a stable id.
pub const WM_TRAY_ICON: u32 = 0x0400 + 101;
pub const WM_TRAY_WAKE_INSTANCE: u32 = 0x0400 + 102;

#[cfg(windows)]
#[path = "tray_win.rs"]
mod imp;

#[cfg(not(windows))]
#[path = "tray_linux.rs"]
mod imp;

pub use imp::run_tray_loop;

/// One selectable tray menu entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TrayAction {
    ShowFullPanel,
    ShowMiniDrawer,
    TurnOffScreen,
    SetPowerMode(u8),
    ToggleFanBoost,
    ToggleOemTakeover,
    Quit,
}

/// Runs a menu action. Every backend funnels through here so the two tray
/// implementations cannot drift apart.
pub(super) fn perform(action: TrayAction) {
    match action {
        TrayAction::ShowFullPanel => {
            let mut cfg = crate::system::config::load_config();
            cfg.last_window_expanded = true;
            let _ = crate::system::config::save_config(&cfg);
            crate::system::slint_window::send_slint_ipc_message(serde_json::json!({
                "action": "expand_dashboard",
                "is_expanded": true
            }));
            crate::system::slint_window::show_slint_window();
        }
        TrayAction::ShowMiniDrawer => {
            let mut cfg = crate::system::config::load_config();
            cfg.last_window_expanded = false;
            let _ = crate::system::config::save_config(&cfg);
            crate::system::slint_window::send_slint_ipc_message(serde_json::json!({
                "action": "collapse_dashboard",
                "is_expanded": false
            }));
            crate::system::slint_window::show_slint_window();
        }
        TrayAction::TurnOffScreen => {
            let _ = crate::system::switches::turn_off_screen();
        }
        TrayAction::SetPowerMode(mode) => set_power_mode_standalone(mode),
        TrayAction::ToggleFanBoost => {
            if let Some(ec) = GLOBAL_EC_BRIDGE.get() {
                let next = !ec.lock().is_fan_boost();
                ec.lock().set_fan_boost(next);
                let mut cfg = crate::system::config::load_config();
                cfg.fan_boost = next;
                let _ = crate::system::config::save_config(&cfg);
                crate::system::osd_window::show_fan_boost(next);
                crate::system::slint_window::send_slint_ipc_message(serde_json::json!({
                    "action": "sync_state",
                    "fan_boost": next
                }));
            }
        }
        TrayAction::ToggleOemTakeover => {
            if crate::system::process::is_oem_service_running() {
                let _ = crate::system::service_mgr::pause_oem_services();
            } else {
                let _ = crate::system::service_mgr::resume_oem_services();
            }
        }
        TrayAction::Quit => shutdown_everything(),
    }
}

/// Mode row state for the menu's radio marks.
pub(super) fn current_menu_state() -> (u8, bool, bool) {
    let cfg = crate::system::config::load_config();
    let current_mode = match GLOBAL_EC_BRIDGE.get() {
        Some(ec) => ec.lock().get_power_mode(),
        None => cfg.power_mode,
    };
    let current_fan = match GLOBAL_EC_BRIDGE.get() {
        Some(ec) => ec.lock().is_fan_boost(),
        None => cfg.fan_boost,
    };
    (current_mode, current_fan, cfg.last_window_expanded)
}

fn set_power_mode_standalone(mode: u8) {
    let is_dc = crate::commands::hardware_cmd::is_dc_round_plugged_in();
    let mut cfg = crate::system::config::load_config();
    if (mode == 3 || mode == 4) && !is_dc && !cfg.unlock_battery_power_limit {
        crate::log_warn!(
            "TRAY",
            "Blocked Turbo/Custom mode: Running on battery and unlock limit not enabled."
        );
        return;
    }

    if let Some(ec) = GLOBAL_EC_BRIDGE.get() {
        ec.lock().set_power_mode(mode);
    }
    cfg.power_mode = mode;
    if is_dc {
        cfg.power_mode_ac = mode;
    } else {
        cfg.power_mode_battery = mode;
    }
    let _ = crate::system::config::save_config(&cfg);
    crate::system::osd_window::show_power_mode(mode);
    crate::system::slint_window::send_slint_ipc_message(serde_json::json!({
        "action": "sync_state",
        "power_mode": mode
    }));
}

fn shutdown_everything() {
    crate::log_info!("TRAY", "Tray quit triggered. Performing safe shutdown...");
    crate::system::osd_window::shutdown_osd_process();
    crate::system::slint_window::shutdown_slint_process();
    crate::system::tray::safe_hardware_shutdown_on_exit();
    crate::system::plugin_service::shutdown_all_plugin_services();
    crate::remove_core_pid_file();
}
