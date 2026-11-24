//! `wmi_ip3` — Windows uses the WMI COM transport; Linux has no WMI stack, so the
//! same API is served by stubs (see the port plan: WMI is replaced by sysfs
//! channels, not emulated).

#[cfg(windows)]
#[path = "wmi_ip3_win.rs"]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod stub {
    pub const IP3_BL_GROUP: u8 = 3;
    pub const IP3_BL_OFFSET: u8 = 0xE2;

    pub fn probe_ip3_available() -> bool {
        false
    }

    pub fn is_ip3_available() -> bool {
        false
    }

    pub fn ip3_get_power_mode() -> Option<u8> {
        None
    }

    pub fn ip3_set_power_mode(_raw_mode: u8) -> bool {
        false
    }

    pub fn ip3_read_ec_reg(_group: u8, _offset: u8) -> Option<u8> {
        None
    }

    pub fn ip3_write_ec_reg(_group: u8, _offset: u8, _value: u8) -> bool {
        false
    }

    pub fn ip3_set_keyboard_backlight_level(_level_idx: u8) -> bool {
        false
    }

    pub fn ip3_get_keyboard_backlight_level() -> Option<u8> {
        None
    }
}

#[cfg(not(windows))]
pub use stub::*;
