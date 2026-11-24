//! `wmi_event_listener` — Windows uses the WMI COM transport; Linux has no WMI stack, so the
//! same API is served by stubs (see the port plan: WMI is replaced by sysfs
//! channels, not emulated).

#[cfg(windows)]
#[path = "wmi_event_listener_win.rs"]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod stub {
    use crate::driver::ec_bridge::SharedEcBridge;

    /// No WMI event source on Linux; the chassis-key path is served by evdev
    /// (`/dev/input/event*`) once the hotkey backend lands.
    pub fn start_wmi_event_listener(_ec_bridge: SharedEcBridge) {
        crate::log_info!(
            "WMI_LISTENER",
            "WMI event listener is Windows-only on this build; chassis keys come from evdev"
        );
    }
}

#[cfg(not(windows))]
pub use stub::*;
