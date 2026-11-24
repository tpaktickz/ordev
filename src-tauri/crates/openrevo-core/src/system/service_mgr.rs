//! Service Manager and Driver Installation Subsystem
//!
//! On Windows this installs and heals the Uniwill ACPI KMDF PnP driver through
//! pnputil and keeps the OEM capability registry entries honest. On Linux the
//! driver role is filled by the kernel (ec_sys / tongfang-mifs-wmi), so the
//! staging and pnputil steps simply fail soft and the registry writes fall
//! through [`crate::pal::reg`]'s no-op backend.
//!
//! OEM takeaway/restore logic lives in `crate::system::takeover::*`; the
//! re-exports below keep the old path working.

use std::path::PathBuf;

use crate::pal::process::run_quiet;
use crate::pal::reg::{self, RegRoot};

pub use crate::system::takeover::{
    disable_scheduled_tasks, enable_scheduled_tasks, find_official_control_center, is_oem_installed,
    is_oem_running, pause_oem_services, refresh_tray_notification_area, resume_oem_services,
    ControlCenterLocation, OEM_PROCESSES, OEM_SERVICES, OEM_TASKS,
};

pub const DRIVER_SYS_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.sys");
pub const DRIVER_INF_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.inf");
pub const DRIVER_CAT_BYTES: &[u8] = include_bytes!("../../../../resources/driver/UWACPIDriver.cat");

fn reg_key_exists(root: RegRoot, subkey: &str) -> bool {
    reg::key_exists(root, subkey)
}

fn set_reg_dword(root: RegRoot, subkey: &str, val_name: &str, val: u32) {
    reg::set_dword_or_create(root, subkey, val_name, val);
}

fn set_reg_string(root: RegRoot, subkey: &str, val_name: &str, val: &str) {
    reg::set_string(root, subkey, val_name, val);
}

pub fn run_cmd_captured(program: &str, args: &[&str]) -> (Option<i32>, String, String) {
    run_quiet(program, args)
}

fn get_driver_staging_dir() -> PathBuf {
    if let Ok(prog_data) = std::env::var("ProgramData") {
        PathBuf::from(prog_data).join("OpenRevo").join("driver")
    } else if let Ok(app_data) = std::env::var("APPDATA") {
        PathBuf::from(app_data).join("OpenRevo").join("driver")
    } else {
        std::env::temp_dir().join("OpenRevo_driver")
    }
}

/// Install the Uniwill ACPI KMDF PnP driver via Microsoft PnP Utility (pnputil).
pub fn install_pnp_acpi_driver() -> Result<bool, String> {
    let staging_dir = get_driver_staging_dir();
    if let Err(e) = std::fs::create_dir_all(&staging_dir) {
        crate::log_error!("SERVICE_MGR", "Failed to create driver staging dir {:?}: {}", staging_dir, e);
        return Err(e.to_string());
    }

    let inf_path = staging_dir.join("UWACPIDriver.inf");
    let sys_path = staging_dir.join("UWACPIDriver.sys");
    let cat_path = staging_dir.join("UWACPIDriver.cat");

    let _ = std::fs::write(&inf_path, DRIVER_INF_BYTES);
    let _ = std::fs::write(&sys_path, DRIVER_SYS_BYTES);
    let _ = std::fs::write(&cat_path, DRIVER_CAT_BYTES);

    crate::log_info!("SERVICE_MGR", "Invoking pnputil to install WDF PnP driver from {:?}...", inf_path);
    let (code, stdout, stderr) = run_cmd_captured("pnputil.exe", &[
        "/add-driver",
        inf_path.to_str().unwrap_or("UWACPIDriver.inf"),
        "/install",
    ]);

    crate::log_info!("SERVICE_MGR", "pnputil /add-driver /install finished: code={:?}, stdout={}, stderr={}", code, stdout, stderr);

    let _ = run_quiet("pnputil.exe", &["/scan-devices"]);

    let available = crate::driver::wmi_ec::mark_driver_rechecked();
    crate::log_info!("SERVICE_MGR", "PnP driver post-install probe result: available={}", available);
    if available {
        crate::system::boot_trace::boot_log("DRIVER_HEALED", "UWACPIDriver PnP self-healing succeeded. DriverStore & device link restored.");
    } else {
        crate::system::boot_trace::boot_log("DRIVER_FAIL", "UWACPIDriver PnP self-healing completed but \\\\.\\ACPIDriver device link is not yet active.");
    }
    Ok(available)
}

/// Ensure ACPI kernel driver (UWACPIDriver.sys) and static capability registry entries are present and healthy.
pub fn ensure_acpi_driver_ready() -> Result<(), String> {
    #[cfg(windows)]
    if !crate::hal::platform_probe::needs_acpi_kernel_driver() {
        crate::log_info!(
            "SERVICE_MGR",
            "Platform uses native WMI/driverless mailbox channel. UWACPIDriver.sys installation bypassed."
        );
        return Ok(());
    }

    crate::log_info!("SERVICE_MGR", "Checking ACPI kernel driver & static registry capability readiness...");

    if crate::driver::wmi_ec::is_acpi_driver_available() {
        crate::log_info!("SERVICE_MGR", "UWACPIDriver device is active and accessible (0ms fast path). Skipping installation.");
        self_heal_oem_registry_pollution();
        return Ok(());
    }

    crate::log_warn!("SERVICE_MGR", "UWACPIDriver device unavailable. Executing PnP DriverStore self-healing...");
    let _ = install_pnp_acpi_driver();

    if !crate::system::process::is_oem_installed() {
        let item_support = "SOFTWARE\\OEM\\GamingCenter2\\ItemSupport";
        let static_dwords = [
            "RamFan1p5Support",
            "DGpuDirectConnectionSupport",
            "TurboModeSupport",
            "PerformanceReady",
            "ServiceReady",
            "FanSettingsSupport",
            "ColorCalibrationSupport",
            "OcSettingsSupport",
            "SystemMonitorSupport",
            "CPUPerformanceAndOverClockMenuSupport",
        ];
        for k in static_dwords {
            set_reg_dword(RegRoot::Machine, item_support, k, 1);
        }
        let flags = crate::driver::wmi_ec::get_hardware_support_flags();
        if flags.has_water_cooler {
            set_reg_dword(RegRoot::Machine, item_support, "LiquidCoolingSupport", 1);
            set_reg_dword(RegRoot::Machine, item_support, "LiquidCoolingAutoModeSupport", 1);
        }
        set_reg_string(RegRoot::Machine, "SOFTWARE\\OEM\\GamingCenter2\\MyFanTable", "EcVersion", "1.17");
    }

    self_heal_oem_registry_pollution();

    crate::log_info!("SERVICE_MGR", "UWACPIDriver & OEM capability safeguards verified and ready (native Win32).");
    Ok(())
}

/// Automatically detect and self-heal historical registry pollution.
pub fn self_heal_oem_registry_pollution() {
    let flags = crate::driver::wmi_ec::get_hardware_support_flags();
    if !flags.has_water_cooler {
        let item_support = "SOFTWARE\\OEM\\GamingCenter2\\ItemSupport";
        if reg_key_exists(RegRoot::Machine, item_support) {
            set_reg_dword(RegRoot::Machine, item_support, "LiquidCoolingSupport", 0);
            set_reg_dword(RegRoot::Machine, item_support, "LiquidCoolingAutoModeSupport", 0);
            crate::log_info!("SERVICE_MGR", "Self-healed OEM registry: cleared ghost water cooler support on air-cooled machine");
        }
    }
}
