//! `wmi_mulong` — Windows uses the WMI COM transport; Linux has no WMI stack, so the
//! same API is served by stubs (see the port plan: WMI is replaced by sysfs
//! channels, not emulated).

#[cfg(windows)]
#[path = "wmi_mulong_win.rs"]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod stub {
    pub fn probe_mulong_available() -> bool {
        false
    }

    pub fn verify_mulong_functional() -> bool {
        false
    }

    pub fn is_mulong_available() -> bool {
        false
    }

    pub fn mulong_read_ec(_addr: u16) -> Option<u8> {
        None
    }

    pub fn mulong_write_ec(_addr: u16, _val: u8) -> bool {
        false
    }

    pub fn mulong_exec_raw(_raw_val: u64) -> bool {
        false
    }

    pub fn mulong_query_raw(_raw_val: u64) -> Option<u64> {
        None
    }

    pub fn probe_hotswitch_support() -> bool {
        false
    }

    pub fn get_acpi_igpu_status() -> Option<u8> {
        None
    }

    pub fn set_acpi_igpu_mode(_enable: bool) -> bool {
        false
    }
}

#[cfg(not(windows))]
pub use stub::*;
