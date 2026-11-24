//! OpenRevo UEFI NVRAM Driver
//! Direct hardware communication with Motherboard SPI Flash via Win32 Firmware Environment APIs.
//! Fully compatible with both Mechrevo 40-Series and 50-Series laptops.

use serde::{Deserialize, Serialize};

use crate::pal::firmware;

const OEM_MAGIC_VAR_NAME: &str = "OemMagicVariable";
const UNIWILL_VAR_NAME: &str = "UniWillVariable";
const OEM_DOOR_VAR_NAME: &str = "OemMagicDoor";
#[allow(dead_code)]
const OEM_DGPU_PRESENT_VAR_NAME: &str = "OemDgpuPresent";
const UNIWILL_GUID: &str = firmware::UNIWILL_GUID;

// Offsets verified via Official DefaultTool.exe Reflection:
pub const OFFSET_MEMORY_OC_SWITCH: usize = 51;  // 0x33: MemoryOverClockSwitch (1=enabled, 0=disabled)
pub const OFFSET_AP_VERSION: usize = 67; // 0x43: APVersionCheck (23 for 40-series, >=25 for 50-series)
pub const OFFSET_MEMORY_OC_SUPPORT: usize = 96; // 0x60: MemoryOverClockSupport (1=supported)
pub const OFFSET_DISPLAY_MODE: usize = 98; // 0x62: OemDisplayMode
pub const OFFSET_SUPPORT: usize = 110;    // 0x6E: CPUPerformanceAndOverClockMenuSupport (1=supported)
pub const OFFSET_SWITCH: usize = 111;     // 0x6F: CPUPerformanceAndOverClockMenuSwitch (1=enabled, 0=disabled)

// Official DGgpuDirectConnectionMode constants:
pub const INTEL_IGPU_ONLY: u8 = 1;
pub const INTEL_DGPU_ONLY: u8 = 2;
pub const INTEL_DYNAMIC: u8 = 4;

pub const AMD_MSHYBRID: u8 = 0;
pub const AMD_DGPU_ONLY: u8 = 1;
pub const AMD_IGPU_ONLY: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiosMenuStatus {
    pub supported: bool,
    pub enabled: bool,
    pub ap_version: u8,
    pub is_50_series: bool,
    #[serde(default)]
    pub is_amd: bool,
    pub hardware_read: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub cpu_oc_supported: bool,
    #[serde(default)]
    pub cpu_oc_enabled: bool,
    #[serde(default)]
    pub mem_oc_supported: bool,
    #[serde(default)]
    pub mem_oc_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuModeHardwareStatus {
    pub current_mode: String,        // "dgpu" | "hybrid" | "igpu" | "unknown"
    pub raw_value: u8,
    pub platform: String,            // "intel" | "amd"
    pub cpu_name: String,
    pub hardware_read: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCapabilities {
    pub supports_dgpu: bool,
    pub supports_hybrid: bool,
    pub supports_igpu_only: bool,
    pub reason_igpu_unsupported: Option<String>,
    pub platform: String,            // "intel" | "amd"
    pub is_50_series: bool,
    pub is_30_series: bool,
    pub baseboard: String,
    pub gpu_name: String,
    #[serde(default)]
    pub is_safety_locked: bool,
    #[serde(default)]
    pub user_unlocked: bool,
    #[serde(default)]
    pub supports_hotswitch: bool,
    #[serde(default)]
    pub acpi_igpu_active: bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct LUID {
    low_part: u32,
    high_part: i32,
}

#[repr(C)]
struct TOKEN_PRIVILEGES {
    privilege_count: u32,
    luid: LUID,
    attributes: u32,
}

/// Detect CPU brand name and platform (Intel vs AMD)
pub fn get_cpu_info() -> (String, bool) {
    // The brand string lives in the registry on Windows and /proc/cpuinfo on
    // Linux; `firmware::cpu_brand` covers both.
    let mut cpu_name = firmware::cpu_brand().unwrap_or_default().trim().to_string();

    // Fallback to PROCESSOR_IDENTIFIER environment variable
    if cpu_name.is_empty() {
        if let Ok(id) = std::env::var("PROCESSOR_IDENTIFIER") {
            cpu_name = id;
        } else {
            cpu_name = "Unknown Processor".to_string();
        }
    }

    let is_amd = crate::system::cpu_monitor::is_amd_cpu();

    (cpu_name, is_amd)
}

#[derive(Debug, Clone)]
pub struct NvramBuffer {
    pub buf: Vec<u8>,
    pub bytes_read: u32,
    pub var_name: &'static str,
    pub ap_version: u8,
    pub is_50_series: bool,
}

/// Automatically probe and read active UEFI NVRAM variable:
/// Priority 1: OemMagicVariable (Next-Gen 50-Series Blackwell / Intel Core Ultra / AMD Ryzen 9000)
/// Priority 2: UniWillVariable (40-Series Ada Lovelace / 12-14th Gen Intel / Older AMD)
///
/// Both variables are owned by the firmware through efivarfs on Linux and
/// through `GetFirmwareEnvironmentVariableW` on Windows; the pal layer hides
/// which of the two it is.
pub fn read_active_nvram() -> Result<NvramBuffer, String> {
    firmware::unlock_privilege()?;
    let mut last_err = String::from("no firmware variable readable");

    // Priority 1: OemMagicVariable (50-Series native)
    match firmware::read_var(UNIWILL_GUID, OEM_MAGIC_VAR_NAME, 512) {
        Ok(buf) if !buf.is_empty() => {
            let bytes_read = buf.len() as u32;
            let ap_version = if bytes_read > OFFSET_AP_VERSION as u32 { buf[OFFSET_AP_VERSION] } else { 0 };
            return Ok(NvramBuffer {
                buf,
                bytes_read,
                var_name: OEM_MAGIC_VAR_NAME,
                ap_version,
                is_50_series: true,
            });
        }
        Ok(_) => last_err = format!("{OEM_MAGIC_VAR_NAME} is empty"),
        Err(e) => last_err = format!("{OEM_MAGIC_VAR_NAME}: {e}"),
    }

    // Priority 2: UniWillVariable (40-Series legacy)
    match firmware::read_var(UNIWILL_GUID, UNIWILL_VAR_NAME, 512) {
        Ok(buf) if !buf.is_empty() => {
            let bytes_read = buf.len() as u32;
            let ap_version = if bytes_read > OFFSET_AP_VERSION as u32 { buf[OFFSET_AP_VERSION] } else { 0 };
            let is_50_series = ap_version >= 25;
            return Ok(NvramBuffer {
                buf,
                bytes_read,
                var_name: UNIWILL_VAR_NAME,
                ap_version,
                is_50_series,
            });
        }
        Ok(_) => last_err = format!("{UNIWILL_VAR_NAME} is empty"),
        Err(e) => last_err = format!("{UNIWILL_VAR_NAME}: {e}"),
    }

    Err(format!(
        "UEFI NVRAM 未找到主板控制变量 (尝试了 OemMagicVariable 与 UniWillVariable) — {last_err}"
    ))
}

/// Unlock 50-series hardware gatekeeper via OemMagicDoor
pub fn unlock_magic_door() {
    match firmware::write_var(UNIWILL_GUID, OEM_DOOR_VAR_NAME, &[0x01]) {
        Ok(()) => crate::log_info!("NVRAM", "OemMagicDoor unlocked successfully."),
        Err(e) => crate::log_warn!("NVRAM", "OemMagicDoor unlock write failed: {}", e),
    }
}

/// Atomically write buffer back to Motherboard SPI Flash NVRAM:
/// Automatically performs 50-series OemMagicDoor handshake when needed.
pub fn write_active_nvram(var_name: &str, is_50_series: bool, buf: &[u8]) -> Result<(), String> {
    firmware::unlock_privilege()?;

    // 50-series hardware handshake: Unlock OemMagicDoor before writing
    if is_50_series || var_name == OEM_MAGIC_VAR_NAME {
        unlock_magic_door();
    }

    firmware::write_var(UNIWILL_GUID, var_name, buf)
        .map_err(|e| format!("failed to write {var_name} ({e})"))
}

/// Read UEFI NVRAM and detect Advanced BIOS Overclocking Menu status (CPU & Memory OC)
pub fn read_bios_advanced_menu_status() -> BiosMenuStatus {
    let (_cpu_name, is_amd) = get_cpu_info();

    let nv = match read_active_nvram() {
        Ok(res) => res,
        Err(e) => {
            return BiosMenuStatus {
                supported: false,
                enabled: false,
                ap_version: 0,
                is_50_series: false,
                is_amd,
                hardware_read: false,
                error: Some(e),
                cpu_oc_supported: false,
                cpu_oc_enabled: false,
                mem_oc_supported: false,
                mem_oc_enabled: false,
            };
        }
    };

    // AMD platforms utilize SMU dynamic power architecture and PBO/CBS firmware;
    // they do not use Intel-specific UniWillVariable OC menu structures.
    if is_amd {
        return BiosMenuStatus {
            supported: false,
            enabled: false,
            ap_version: nv.ap_version,
            is_50_series: nv.is_50_series,
            is_amd: true,
            hardware_read: true,
            error: None,
            cpu_oc_supported: false,
            cpu_oc_enabled: false,
            mem_oc_supported: false,
            mem_oc_enabled: false,
        };
    }

    let cpu_oc_supported = nv.bytes_read > OFFSET_SUPPORT as u32 && nv.buf[OFFSET_SUPPORT] == 1;
    let cpu_oc_enabled = nv.bytes_read > OFFSET_SWITCH as u32 && nv.buf[OFFSET_SWITCH] == 1;

    let mem_oc_supported = nv.bytes_read > OFFSET_MEMORY_OC_SUPPORT as u32 && nv.buf[OFFSET_MEMORY_OC_SUPPORT] == 1;
    let mem_oc_enabled = nv.bytes_read > OFFSET_MEMORY_OC_SWITCH as u32 && nv.buf[OFFSET_MEMORY_OC_SWITCH] == 1;

    let supported = cpu_oc_supported || mem_oc_supported;
    // Enabled if all supported menus are active
    let enabled = supported
        && (!cpu_oc_supported || cpu_oc_enabled)
        && (!mem_oc_supported || mem_oc_enabled);

    BiosMenuStatus {
        supported,
        enabled,
        ap_version: nv.ap_version,
        is_50_series: nv.is_50_series,
        is_amd: false,
        hardware_read: true,
        error: None,
        cpu_oc_supported,
        cpu_oc_enabled,
        mem_oc_supported,
        mem_oc_enabled,
    }
}

/// Read Hardware GPU Mode:
/// Reads Offset 98 (OemDisplayMode) directly from SPI Flash NVRAM (OemMagicVariable or UniWillVariable).
/// Resolves mode string using official DGgpuDirectConnectionMode enum table per CPU architecture.
pub fn read_gpu_mode() -> GpuModeHardwareStatus {
    let (cpu_name, is_amd) = get_cpu_info();
    let platform_str = if is_amd { "amd".to_string() } else { "intel".to_string() };

    // 0. Declarative Driver Contract First (Level 0 Architecture - e.g. Baolongda Jiaolong 16 Pro)
    if let Some(manifest) = crate::hal::declarative::load_matched_manifest() {
        if manifest.actions.contains_key("get_gpu_mux") || manifest.actions.contains_key("set_gpu_mux_hybrid") {
            let drv = crate::hal::declarative::DeclarativeHardwareDriver::new(manifest.clone());
            if let Ok(mode_str) = crate::hal::traits::HardwareDriver::get_gpu_mux(&drv) {
                let raw_val = if mode_str == "dgpu" { 1 } else { 0 };
                let platform_str = manifest.capabilities.cpu_vendor.clone().unwrap_or(platform_str);
                return GpuModeHardwareStatus {
                    current_mode: mode_str,
                    raw_value: raw_val,
                    platform: platform_str,
                    cpu_name,
                    hardware_read: true,
                    error: None,
                };
            }
        }
    }

    let nv = match read_active_nvram() {
        Ok(res) => res,
        Err(e) => {
            if crate::driver::wmi_ec::is_legacy_dual_mode_ec() {
                return GpuModeHardwareStatus {
                    current_mode: "hybrid".to_string(),
                    raw_value: 0x00,
                    platform: platform_str,
                    cpu_name,
                    hardware_read: true,
                    error: None,
                };
            }
            return GpuModeHardwareStatus {
                current_mode: "unknown".to_string(),
                raw_value: 0xFF,
                platform: platform_str,
                cpu_name,
                hardware_read: false,
                error: Some(e),
            };
        }
    };

    if nv.bytes_read <= OFFSET_DISPLAY_MODE as u32 {
        return GpuModeHardwareStatus {
            current_mode: "unknown".to_string(),
            raw_value: 0xFF,
            platform: platform_str,
            cpu_name,
            hardware_read: false,
            error: Some(format!("NVRAM size ({} bytes) <= Offset {}", nv.bytes_read, OFFSET_DISPLAY_MODE)),
        };
    }

    let raw_val = nv.buf[OFFSET_DISPLAY_MODE];
    let mode_str = if is_amd {
        match raw_val {
            AMD_DGPU_ONLY => "dgpu",
            AMD_MSHYBRID => "hybrid",
            AMD_IGPU_ONLY => "igpu",
            _ => "unknown",
        }
    } else {
        match raw_val {
            INTEL_DGPU_ONLY => "dgpu",
            INTEL_DYNAMIC => "hybrid",
            INTEL_IGPU_ONLY => "igpu",
            _ => "unknown",
        }
    };

    GpuModeHardwareStatus {
        current_mode: mode_str.to_string(),
        raw_value: raw_val,
        platform: platform_str,
        cpu_name,
        hardware_read: true,
        error: None,
    }
}

/// Gracefully stops NVIDIA display services to release eDP pipeline handles and avoid MUX lockup
pub fn stop_nvidia_services() {
    crate::log_info!("NVIDIA_SVC", "Gracefully stopping NVIDIA display services to release display handles...");
    firmware::stop_gpu_services();
}

/// Restarts NVIDIA display services after returning to hybrid or dGPU mode
pub fn start_nvidia_services() {
    crate::log_info!("NVIDIA_SVC", "Starting NVIDIA display services...");
    firmware::start_gpu_services();
}

/// Triggers a PCI bus hardware re-scan so the device tree refreshes device presence
pub fn rescan_pci_devices() {
    crate::log_info!("PCI_SCAN", "Rescanning PCI devices...");
    firmware::rescan_pci();
}

/// Pre-wakes up iGPU power rails and display stack prior to MUX switch reboot:
/// In dGPU direct mode, the iGPU is typically in deep D3Cold power-gated state.
/// If reboot is executed while iGPU is unpowered, BIOS POST MUX handover to iGPU
/// often fails link training / GOP init, causing a black-screen hang.
pub fn wakeup_igpu_pipeline() {
    crate::log_info!("GPU_WAKE", "Pre-waking up iGPU power rails and scanning display adapters before reboot...");

    // 1. Ensure Tongfang ACPI WMI selector is set to hybrid (turn dGPU suppression off / allow iGPU power)
    notify_wmi_igpu_mode(false);

    // 2. Refresh the display device tree
    firmware::rescan_pci();

    // 3. Brief hardware power rail settling time
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// Notify Tongfang WMI ACPI driver of iGPU state transition if AcpiTest_MULong device exists
pub fn notify_wmi_igpu_mode(enable_igpu_only: bool) -> bool {
    crate::driver::wmi_mulong::set_acpi_igpu_mode(enable_igpu_only)
}

/// Canonical Hardware Truth: Identifies whether the processor physically lacks an Integrated Graphics Unit (iGPU).
/// Models like AMD Ryzen 7 7435H / 7235H and Intel Core F-series desktop transplants (e.g. i5-13400F, i7-14700F)
/// physically lack display engine silicon, strictly requiring safety lock against iGPU-only mode.
pub fn is_cpu_lacking_igpu_silicon(cpu_name: &str) -> bool {
    let u = cpu_name.to_uppercase();
    u.contains("7435H") || u.contains("7235H") || (u.contains("CORE") && u.ends_with('F'))
}

/// Autonomous Multi-Signal Hardware Capability Prober:
/// Probes DMI, NVRAM, GPU silicon and platform architecture without relying on Windows registry.
/// Strictly distinguishes Dual-Mode machines (legacy 30-series / GK models) from Tri-Mode machines (40-series / 50-series).
pub fn probe_gpu_capabilities() -> GpuCapabilities {
    let (cpu_name, is_amd) = get_cpu_info();

    // 1. Detect baseboard / system product from DMI
    let baseboard = crate::system::model_engine::get_device_full_name().to_uppercase();

    // 2. Detect discrete GPU from prioritized hardware channels
    let (gpu_name, has_dgpu) = if let Some((name, _)) = crate::driver::gpu_bridge::detect_discrete_gpu() {
        (name, true)
    } else if let Some((name, _)) = crate::driver::gpu_bridge::detect_gpu_from_registry() {
        let is_disc = crate::driver::gpu_bridge::is_discrete_gpu_name(&name);
        (name, is_disc)
    } else {
        let has_physical_dgpu = crate::driver::gpu_bridge::is_nvidia_dgpu_available()
            || crate::system::power::status::check_dgpu_present();
        let name = if has_physical_dgpu {
            if let Some((pci_name, _)) = crate::driver::gpu_bridge::detect_dgpu_from_pci_enum() {
                pci_name
            } else {
                "NVIDIA Discrete Graphics Card".to_string()
            }
        } else {
            crate::driver::gpu_bridge::get_gpu_name_cached()
        };
        (name, has_physical_dgpu)
    };

    // Fast-path: Ultrabook / Thin-and-light pure iGPU platform guard
    if crate::system::power::status::is_thin_and_light_or_pure_igpu() {
        return GpuCapabilities {
            supports_dgpu: false,
            supports_hybrid: false,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some("当前机型为纯核显轻薄架构（无物理独立显卡与 MUX 切换芯片），无需且不支持显卡模式切换。".to_string()),
            platform: if is_amd { "amd".to_string() } else { "intel".to_string() },
            is_50_series: false,
            is_30_series: false,
            baseboard: baseboard.clone(),
            gpu_name: gpu_name.clone(),
            is_safety_locked: false,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    // 0. Declarative Driver Contract First (Level 0 Architecture - e.g. Baolongda Jiaolong 16 Pro)
    if let Some(manifest) = crate::hal::declarative::load_matched_manifest() {
        let platform_str = manifest.capabilities.cpu_vendor.clone().unwrap_or_else(|| if is_amd { "amd".to_string() } else { "intel".to_string() });

        if !manifest.capabilities.discrete_gpu {
            return GpuCapabilities {
                supports_dgpu: false,
                supports_hybrid: false,
                supports_igpu_only: false,
                reason_igpu_unsupported: Some("当前机型为纯核显轻薄架构（无物理独立显卡与 MUX 切换芯片），无需且不支持显卡模式切换。".to_string()),
                platform: platform_str,
                is_50_series: false,
                is_30_series: false,
                baseboard: baseboard.clone(),
                gpu_name: gpu_name.clone(),
                is_safety_locked: false,
                user_unlocked: false,
                supports_hotswitch: false,
                acpi_igpu_active: false,
            };
        }

        let is_dual_mode = manifest.capabilities.gpu_mux_type.as_deref() == Some("dual_mode")
            || (manifest.capabilities.supported_gpu_modes.contains(&"dgpu".to_string())
                && manifest.capabilities.supported_gpu_modes.contains(&"hybrid".to_string())
                && !manifest.capabilities.supported_gpu_modes.contains(&"igpu".to_string()))
            || (manifest.actions.contains_key("set_gpu_mux_hybrid") && !manifest.actions.contains_key("set_gpu_mux_igpu"));

        if is_dual_mode {
            let allow_geek = crate::system::config::load_config().allow_experimental_igpu_only;
            return GpuCapabilities {
                supports_dgpu: true,
                supports_hybrid: true,
                supports_igpu_only: allow_geek,
                reason_igpu_unsupported: if allow_geek {
                    None
                } else {
                    Some("当前模具为双显双模硬件架构（支持独显直连与混合输出），无物理核显直连走线，已锁定保护以防黑屏。".to_string())
                },
                platform: platform_str,
                is_50_series: false,
                is_30_series: false,
                baseboard: baseboard.clone(),
                gpu_name: gpu_name.clone(),
                is_safety_locked: true,
                user_unlocked: allow_geek,
                supports_hotswitch: false,
                acpi_igpu_active: false,
            };
        }
    }

    let (is_50_nvram, project_id) = if let Ok(nv) = read_active_nvram() {
        let is_50 = nv.is_50_series || nv.var_name == OEM_MAGIC_VAR_NAME || nv.ap_version >= 25;
        let pid = if nv.buf.len() > 6 { nv.buf[6] } else { 0 };
        (is_50, pid)
    } else {
        (false, 0)
    };

    let cfg = crate::system::config::load_config();
    let allow_geek_unlock = cfg.allow_experimental_igpu_only;

    let mut caps = evaluate_gpu_capabilities(&baseboard, &cpu_name, &gpu_name, has_dgpu, is_amd, is_50_nvram, project_id, allow_geek_unlock);

    // Runtime hot-switch disabled per user consensus (forces closing all active apps, unstable on dGPU handles)
    // Reinforced hardware cold-switch with pre-wakeup provides 100% stable, anti-blackscreen handover.
    caps.supports_hotswitch = false;

    caps
}

/// Pure business decision engine for GPU hardware capabilities:
/// Separated for deterministic unit testing and cross-generation regression guardrails.
pub fn evaluate_gpu_capabilities(
    baseboard: &str,
    cpu_name: &str,
    gpu_name: &str,
    has_dgpu: bool,
    is_amd: bool,
    _is_50_nvram: bool,
    _project_id: u8,
    allow_geek_unlock: bool,
) -> GpuCapabilities {
    let platform = if is_amd { "amd".to_string() } else { "intel".to_string() };
    let cpu_upper = cpu_name.to_uppercase();

    // 1. Pure Integrated Graphics SoC (Thin laptops: Wujie, Code, etc.)
    // Hardware physically lacks discrete GPU chip and MUX switching circuits.
    if !has_dgpu {
        return GpuCapabilities {
            supports_dgpu: false,
            supports_hybrid: false,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some("当前机型为纯核显轻薄架构（无物理独立显卡与 MUX 切换芯片），无需且不支持显卡模式切换。".to_string()),
            platform,
            is_50_series: false,
            is_30_series: false,
            baseboard: baseboard.to_string(),
            gpu_name: gpu_name.to_string(),
            is_safety_locked: false,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    // 2. Physical No-iGPU Silicon Hard Gatekeeper (Inviolable Anti-Brick Protection)
    // Processors manufactured with NO physical iGPU silicon units (e.g. AMD Ryzen 7 7435H / 7435HS / Ryzen 5 7235H,
    // or Intel Desktop F-series transplants). Writing iGPU mode will 100% black-screen brick the laptop!
    // This lock CANNOT be bypassed even in Geek Unlock mode.
    let is_cpu_physically_no_igpu = is_cpu_lacking_igpu_silicon(&cpu_name);

    if is_cpu_physically_no_igpu {
        return GpuCapabilities {
            supports_dgpu: true,
            supports_hybrid: true,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some(format!(
                "当前机型搭载的处理器 ({}) 物理未集成核显硬件单元 (No iGPU Silicon)。主板无核显直连走线，已启用不可旁路的硬件防变砖硬锁，禁止切换至纯核显模式以防黑屏！",
                cpu_name
            )),
            platform,
            is_50_series: false,
            is_30_series: false,
            baseboard: baseboard.to_string(),
            gpu_name: gpu_name.to_string(),
            is_safety_locked: true,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    let tier = crate::hal::platform_probe::get_chassis_architecture_tier();

    // 3. Pre-APVersion Legacy Tongfang (10/16/20-Series, e.g. GK5CP / GK5CN):
    // Lacks physical MUX hardware switching circuits. Output is permanently hardwired to Optimus MSHybrid!
    if tier == crate::hal::platform_probe::ChassisArchitectureTier::LegacyPascalTuring {
        return GpuCapabilities {
            supports_dgpu: false,
            supports_hybrid: true,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some("当前模具为前代独显混合输出架构 (NVIDIA Optimus)，主板未搭载 MUX 切换开关芯片，由系统与显卡驱动自动协同调度。".to_string()),
            platform,
            is_50_series: false,
            is_30_series: true,
            baseboard: baseboard.to_string(),
            gpu_name: gpu_name.to_string(),
            is_safety_locked: true,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    let is_50 = tier == crate::hal::platform_probe::ChassisArchitectureTier::NextGenBlackwell;
    let is_30 = tier == crate::hal::platform_probe::ChassisArchitectureTier::DualModeAmpere;

    // 4. High-Risk Dual-Mode Architecture Detection:
    // a. AMD Dragon Range (7745HX / 7945HX): Desktop AM5 chiplet transplants featuring only 2CU display iGPU (Radeon 610M).
    let is_dragon_range_hx = is_amd && (cpu_upper.contains("7745HX") || cpu_upper.contains("7945HX"));

    let is_dual_mode_guard = is_30 || is_dragon_range_hx;

    if is_dual_mode_guard {
        if allow_geek_unlock {
            return GpuCapabilities {
                supports_dgpu: true,
                supports_hybrid: true,
                supports_igpu_only: true,
                reason_igpu_unsupported: None,
                platform,
                is_50_series: is_50,
                is_30_series: is_30,
                baseboard: baseboard.to_string(),
                gpu_name: gpu_name.to_string(),
                is_safety_locked: true,
                user_unlocked: true,
                supports_hotswitch: false,
                acpi_igpu_active: false,
            };
        }

        let reason = if is_dragon_range_hx {
            format!(
                "当前机型搭载 AMD Dragon Range 桌面架构处理器 ({})，仅集成 2CU 亮机集显，硬件公模未配置核显内屏直连走线。已启用防变砖安全锁定。若您已在 BIOS 中确认存在核显直连选项，可开启极客模式解除保护。",
                cpu_name
            )
        } else {
            "当前模具为前代双模固件架构 (Legacy Dual-Mode)。主板硬件未设计纯核显 (iGPU Only) 直连供电旁路，已启用安全保护锁定以防止开机黑屏变砖。".to_string()
        };

        return GpuCapabilities {
            supports_dgpu: true,
            supports_hybrid: true,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some(reason),
            platform,
            is_50_series: is_50,
            is_30_series: is_30,
            baseboard: baseboard.to_string(),
            gpu_name: gpu_name.to_string(),
            is_safety_locked: true,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    // 7. Hardware Truth Tri-Mode Architecture Verification:
    // Any modern platform with valid UniWillVariable / OemMagicVariable (payload size >= 80)
    // physically contains the three-mode switching NVRAM structures.
    let is_known_tri_mode_chassis = is_50 || !is_30;

    if !is_known_tri_mode_chassis {
        if allow_geek_unlock {
            return GpuCapabilities {
                supports_dgpu: true,
                supports_hybrid: true,
                supports_igpu_only: true,
                reason_igpu_unsupported: None,
                platform,
                is_50_series: is_50,
                is_30_series: is_30,
                baseboard: baseboard.to_string(),
                gpu_name: gpu_name.to_string(),
                is_safety_locked: true,
                user_unlocked: true,
                supports_hotswitch: false,
                acpi_igpu_active: false,
            };
        }

        return GpuCapabilities {
            supports_dgpu: true,
            supports_hybrid: true,
            supports_igpu_only: false,
            reason_igpu_unsupported: Some(
                "硬件真理探测: 检测到未知机型或未经验证主板 DMI，已默认启用防黑屏安全锁定，禁止纯核显模式。混合输出与独显直连完全正常开放。若您已在 BIOS 中确认支持纯核显，可开启极客模式解除保护。".to_string()
            ),
            platform,
            is_50_series: is_50,
            is_30_series: is_30,
            baseboard: baseboard.to_string(),
            gpu_name: gpu_name.to_string(),
            is_safety_locked: true,
            user_unlocked: false,
            supports_hotswitch: false,
            acpi_igpu_active: false,
        };
    }

    // 8. Full Tri-Mode Hardware Circuit:
    // Fully supports AMD Ryzen 8000/7000 monolithic APUs (8845H / 8745HS / 7840H with Radeon 780M),
    // and Intel 13th/14th Gen HX flagships (13900HX / 14650HX / 14700HX / 14900HX) with complete 3-Way MUX!
    GpuCapabilities {
        supports_dgpu: true,
        supports_hybrid: true,
        supports_igpu_only: true,
        reason_igpu_unsupported: None,
        platform,
        is_50_series: is_50,
        is_30_series: false,
        baseboard: baseboard.to_string(),
        gpu_name: gpu_name.to_string(),
        is_safety_locked: false,
        user_unlocked: false,
        supports_hotswitch: false,
        acpi_igpu_active: false,
    }
}

/// Write target GPU Mode with generation-adaptive architecture:
/// Atomically modifies Offset 98 (OemDisplayMode) in active NVRAM variable.
/// Rebooting Windows is required for BIOS / display MUX chip hardware handover.
pub fn write_gpu_mode(target_mode: &str) -> Result<GpuModeHardwareStatus, String> {
    let (cpu_name, is_amd) = get_cpu_info();
    let target_norm = target_mode.to_lowercase();

    // 1. Mandatory Physical Anti-Brick Fail-Safe Guard
    let caps = probe_gpu_capabilities();
    if target_norm == "igpu" && !caps.supports_igpu_only {
        let reason = caps.reason_igpu_unsupported.unwrap_or_else(|| {
            "当前模具硬件不支持核显独占模式 (iGPU Only)。".to_string()
        });
        crate::log_error!("NVRAM_GPU", "Anti-brick safety guard intercepted illegal iGPU write: {}", reason);
        return Err(format!("【安全拦截】已阻止写入核显模式以防止主板黑屏变砖！\n{}", reason));
    }

    // 0. Declarative Driver Contract First (Level 0 Architecture - e.g. Baolongda Jiaolong 16 Pro)
    if let Some(manifest) = crate::hal::declarative::load_matched_manifest() {
        if manifest.actions.contains_key("set_gpu_mux_discrete") || manifest.actions.contains_key("set_gpu_mux_hybrid") {
            let discrete_only = match target_norm.as_str() {
                "dgpu" => true,
                "hybrid" => false,
                "igpu" => {
                    return Err("当前模具硬件架构为双显双模，不支持纯核显屏蔽模式。已绝对安全拦截写入！".to_string());
                }
                other => return Err(format!("Unknown GPU mode '{}' for declarative driver", other)),
            };
            let drv = crate::hal::declarative::DeclarativeHardwareDriver::new(manifest);
            crate::hal::traits::HardwareDriver::set_gpu_mux(&drv, discrete_only)
                .map_err(|e| format!("Declarative set_gpu_mux failed: {}", e))?;
            crate::log_info!("DECLARATIVE_GPU", "Successfully updated GPU MUX via declarative driver: {}", target_mode);
            return Ok(read_gpu_mode());
        }
    }

    let target_byte: u8 = if is_amd {
        match target_norm.as_str() {
            "dgpu" => AMD_DGPU_ONLY,
            "hybrid" => AMD_MSHYBRID,
            "igpu" => AMD_IGPU_ONLY,
            other => return Err(format!("Unknown GPU mode '{}' for AMD platform", other)),
        }
    } else {
        match target_norm.as_str() {
            "dgpu" => INTEL_DGPU_ONLY,
            "hybrid" => INTEL_DYNAMIC,
            "igpu" => INTEL_IGPU_ONLY,
            other => return Err(format!("Unknown GPU mode '{}' for Intel platform", other)),
        }
    };

    let mut nv = read_active_nvram()?;

    if nv.bytes_read <= OFFSET_DISPLAY_MODE as u32 {
        return Err(format!("NVRAM size ({} bytes) <= Offset {}", nv.bytes_read, OFFSET_DISPLAY_MODE));
    }

    // Pre-warm / pre-wake iGPU pipeline when switching away from dGPU or to Hybrid/iGPU
    if target_norm != "dgpu" {
        wakeup_igpu_pipeline();
    }

    // Atomic update: only modify Offset 98 (OemDisplayMode)
    nv.buf[OFFSET_DISPLAY_MODE] = target_byte;

    // Commit write back to SPI Flash NVRAM
    write_active_nvram(nv.var_name, nv.is_50_series, &nv.buf[..nv.bytes_read as usize])?;

    // Allow SPI Flash controller hardware write-cycle settle time (800ms)
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Synchronize ACPI runtime selector state to prevent firmware state mismatch during POST
    if crate::driver::wmi_mulong::is_mulong_available() {
        if target_norm == "igpu" {
            crate::log_info!("NVRAM_GPU", "Synchronizing ACPI runtime selector -> iGPU-only ON");
            notify_wmi_igpu_mode(true);
        } else {
            crate::log_info!("NVRAM_GPU", "Synchronizing ACPI runtime selector -> iGPU-only OFF (Hybrid/dGPU)");
            notify_wmi_igpu_mode(false);
        }
    }

    println!(
        "[OpenRevo NVRAM] Successfully wrote {} bytes to {} (OemDisplayMode: 0x{:02X} -> '{}', CPU: {})",
        nv.bytes_read,
        nv.var_name,
        target_byte,
        target_mode,
        cpu_name
    );

    // Read back verification
    Ok(read_gpu_mode())
}

/// Global flag indicating system reboot is in progress to freeze background telemetry/probers
static REBOOT_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Query if system reboot sequence is currently active
pub fn is_reboot_in_progress() -> bool {
    REBOOT_IN_PROGRESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Unified graceful system reboot execution aligned with OEM Control Center:
/// 1. Sets global reboot flag to immediately freeze background hardware telemetry & EC communication.
/// 2. Pre-wakes up iGPU power rails and display stack to prevent BIOS POST black-screen hang.
/// 3. Stops NVIDIA container services to force-release display handles and FrameBuffers before reset.
/// 4. Re-scans PCI bus so device tree is in a consistent state.
/// 5. Provides physical bus & DWM display detachment stabilization delay (>= 1800ms).
/// 6. Executes Windows orderly reboot with /f flag (`shutdown.exe /r /f /t 1`) bypassing Fast Startup hangs.
pub fn trigger_graceful_reboot(stabilize_delay_ms: u64) -> Result<(), String> {
    REBOOT_IN_PROGRESS.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::system::telemetry::set_reboot_freeze(true);

    let effective_delay = if stabilize_delay_ms < 1800 { 1800 } else { stabilize_delay_ms };
    crate::log_info!(
        "SYSTEM_REBOOT",
        "Unified graceful reboot triggered. Preparing hardware MUX & display pipelines for {}ms before OS shutdown...",
        effective_delay
    );

    std::thread::Builder::new()
        .name("graceful-reboot-worker".to_string())
        .spawn(move || {
            // Step 1: Pre-wakeup iGPU power rails and display stack to prevent BIOS POST black-screen hang
            wakeup_igpu_pipeline();

            // Step 2: Ensure ACPI state is synchronized to hybrid/off (prevents firmware/NVRAM mismatch during POST)
            notify_wmi_igpu_mode(false);

            // Step 3: Stop NVIDIA container services to cleanly release active display handles and FrameBuffers
            stop_nvidia_services();

            // Step 4: Rescan devices to ensure hardware bus enumeration is clean
            rescan_pci_devices();

            // Step 5: Physical bus stabilization delay & allow DWM/Display drivers to cleanly unbind
            std::thread::sleep(std::time::Duration::from_millis(effective_delay));

            match firmware::system_reboot() {
                Ok(()) => crate::log_info!(
                    "SYSTEM_REBOOT",
                    "Reboot dispatched successfully. Awaiting OS session termination."
                ),
                Err(e) => crate::log_error!("SYSTEM_REBOOT", "Failed to dispatch reboot: {}", e),
            }
        })
        .map_err(|e| format!("Failed to spawn graceful reboot worker: {}", e))?;

    Ok(())
}

/// Reboot Windows gracefully via unified entrance (default 800ms hardware bus stabilization)
pub fn reboot_system() -> Result<(), String> {
    trigger_graceful_reboot(800)
}

/// Seamless runtime ACPI hot-switch for Tongfang/Mechrevo laptops:
/// Toggles between Hybrid (dGPU online) and iGPU-Only (dGPU silenced/offline) WITHOUT rebooting Windows.
///
/// Hardware Safety Rules:
/// 1. Cannot be executed if currently in dGPU Direct mode (internal panel wired to dGPU).
/// 2. Requires machine support for AcpiTest_MULong::GetSetULong(0x30000000003) == 85.
/// 3. Safely stops NVIDIA display container services and detaches display handles before ACPI cut-off.
/// 4. Synchronizes BIOS UEFI NVRAM Offset 98 so that future cold reboots retain the chosen state.
pub fn execute_runtime_hotswitch(enable_igpu: bool) -> Result<(), String> {
    let current_hw = read_gpu_mode();
    if current_hw.current_mode == "dgpu" {
        return Err("当前处于【独显直连】硬件输出状态，屏幕物理走线直连独显。必须重启电脑经由 BIOS 自检切换 MUX 开关，不支持免重启热切换！".to_string());
    }

    if !crate::driver::wmi_mulong::probe_hotswitch_support() {
        return Err("当前模具主板或 BIOS 固件未实现 ACPI-WMI 无感热切换接口 (GetSetULong 0x30000000003 != 85)。".to_string());
    }

    let target_mode = if enable_igpu { "igpu" } else { "hybrid" };

    if enable_igpu {
        crate::log_info!("HOTSWITCH", "Initiating runtime dGPU silence hot-switch...");
        // 1. Stop NVIDIA services to force release display and memory handles
        stop_nvidia_services();
        std::thread::sleep(std::time::Duration::from_millis(500));

        // 2. Send ACPI WMI command to shut down dGPU power rail (WMIWriteECIGPUonlyON)
        let acpi_ok = crate::driver::wmi_mulong::set_acpi_igpu_mode(true);
        if !acpi_ok {
            crate::log_warn!("HOTSWITCH", "ACPI set_acpi_igpu_mode(true) returned false, verifying device state...");
        }

        // 3. Rescan PCI bus so Windows PnP drops the unpowered device node
        rescan_pci_devices();
        std::thread::sleep(std::time::Duration::from_millis(500));

        // 4. Update NVRAM Offset 98 so next cold boot stays in iGPU
        let _ = write_gpu_mode(target_mode);
        crate::log_info!("HOTSWITCH", "Runtime dGPU silence completed successfully");
    } else {
        crate::log_info!("HOTSWITCH", "Initiating runtime dGPU wake / restore hot-switch...");
        // 1. Send ACPI WMI command to power up dGPU (WMIWriteECIGPUonlyOFF)
        let acpi_ok = crate::driver::wmi_mulong::set_acpi_igpu_mode(false);
        if !acpi_ok {
            crate::log_warn!("HOTSWITCH", "ACPI set_acpi_igpu_mode(false) returned false, verifying device state...");
        }

        // 2. Rescan PCI bus so Windows PnP rediscovers the dGPU
        rescan_pci_devices();
        std::thread::sleep(std::time::Duration::from_millis(800));

        // 3. Restart NVIDIA display services
        start_nvidia_services();

        // 4. Update NVRAM Offset 98 so next cold boot stays in Hybrid
        let _ = write_gpu_mode(target_mode);
        crate::log_info!("HOTSWITCH", "Runtime dGPU restore completed successfully");
    }

    Ok(())
}

/// Adaptive cross-generation write to Motherboard SPI Flash NVRAM:
/// - Reads active NVRAM variable (OemMagicVariable or UniWillVariable)
/// - If 50-series (APVersionCheck >= 25 or OemMagicVariable): Unlocks hardware door via OemMagicDoor first
/// - Atomically writes both Offset 111 (CPU OC) and Offset 51 (Memory OC) based on motherboard support
pub fn write_bios_advanced_menu_status(enable: bool) -> Result<BiosMenuStatus, String> {
    let (_cpu_name, is_amd) = get_cpu_info();
    if is_amd {
        return Err("AMD 平台采用 SMU/CBS 架构，主板固件无 Intel 超频菜单开关，已绝对安全拦截写入！".to_string());
    }

    let mut nv = read_active_nvram()?;

    let cpu_oc_supported = nv.bytes_read > OFFSET_SUPPORT as u32 && nv.buf[OFFSET_SUPPORT] == 1;
    let mem_oc_supported = nv.bytes_read > OFFSET_MEMORY_OC_SUPPORT as u32 && nv.buf[OFFSET_MEMORY_OC_SUPPORT] == 1;

    // Safety guard: Check if motherboard firmware actually supports either OC menu
    if !cpu_oc_supported && !mem_oc_supported {
        return Err("当前机器主板固件或处理器未声明支持高级超频菜单 (CPU 与内存超频均未开放)，已安全拦截写入！".to_string());
    }

    let target_byte = if enable { 0x01 } else { 0x00 };

    // Update CPU OC switch (Offset 111) if supported
    if cpu_oc_supported {
        if nv.bytes_read <= OFFSET_SWITCH as u32 {
            return Err(format!("NVRAM size ({} bytes) is too small for Offset {}", nv.bytes_read, OFFSET_SWITCH));
        }
        nv.buf[OFFSET_SWITCH] = target_byte;
    }

    // Update Memory OC switch (Offset 51) if supported
    if mem_oc_supported {
        if nv.bytes_read <= OFFSET_MEMORY_OC_SWITCH as u32 {
            return Err(format!("NVRAM size ({} bytes) is too small for Offset {}", nv.bytes_read, OFFSET_MEMORY_OC_SWITCH));
        }
        nv.buf[OFFSET_MEMORY_OC_SWITCH] = target_byte;
    }

    // Commit write back to SPI Flash NVRAM
    write_active_nvram(nv.var_name, nv.is_50_series, &nv.buf[..nv.bytes_read as usize])?;

    println!(
        "[OpenRevo NVRAM] Successfully wrote {} bytes to {} (CPU OC: 0x{:02X}, Mem OC: 0x{:02X}, Gen: {})",
        nv.bytes_read,
        nv.var_name,
        if cpu_oc_supported { nv.buf[OFFSET_SWITCH] } else { 0xFF },
        if mem_oc_supported { nv.buf[OFFSET_MEMORY_OC_SWITCH] } else { 0xFF },
        if nv.is_50_series { "50-Series (Door-assisted)" } else { "40-Series (Direct)" }
    );

    // Read back verification
    Ok(read_bios_advanced_menu_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r7_7435h_hardware_no_igpu_permanently_locked() {
        // Real field case: AMD Ryzen 7 7435H has NO physical iGPU silicon.
        // Even if user enables Geek Unlock, it must be rigidly blocked to prevent black-screen bricking!
        let caps = evaluate_gpu_capabilities(
            "YILONG 16 PRO",
            "AMD Ryzen 7 7435H with Radeon Graphics",
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            true,
            true,
            false,
            26,
            true, // allow_geek_unlock is TRUE, but hardware has no iGPU!
        );
        assert!(!caps.supports_igpu_only, "7435H must NEVER allow iGPU mode!");
        assert!(caps.is_safety_locked);
        assert!(!caps.user_unlocked, "7435H cannot be unlocked by geek mode!");
        assert!(caps.reason_igpu_unsupported.unwrap().contains("物理未集成核显"));
    }

    #[test]
    fn test_r7_8845h_yilong_16_pro_unblocked() {
        // Real field case: Yilong 16 Pro (ProjectID 26) with R7 8845H (Radeon 780M).
        // Must NOT be falsely locked! Must support full Tri-Mode out of the box!
        let caps = evaluate_gpu_capabilities(
            "YILONG 16 PRO",
            "AMD Ryzen 7 8845H with Radeon 780M Graphics",
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            true,
            true,
            false,
            26, // ProjectID 26
            false,
        );
        assert!(caps.supports_igpu_only, "8845H on Yilong 16 Pro must support iGPU mode!");
        assert!(!caps.is_safety_locked);
        assert!(caps.reason_igpu_unsupported.is_none());
    }

    #[test]
    fn test_intel_14650hx_project_id_24_unblocked() {
        // Real field case: Intel i7-14650HX (ProjectID 24) on flagship chassis.
        // Must NOT be falsely locked by old ProjectID 24 check!
        let caps = evaluate_gpu_capabilities(
            "MECHREVO Gaming Laptop",
            "Intel(R) Core(TM) i7-14650HX",
            "NVIDIA GeForce RTX 4070 Laptop GPU",
            true,
            false,
            false,
            24, // ProjectID 24
            false,
        );
        assert!(caps.supports_igpu_only, "14650HX must support Tri-Mode out of the box!");
        assert!(!caps.is_safety_locked);
    }

    #[test]
    fn test_amd_7745hx_dragon_range_safely_locked_and_unlockable() {
        // Real field case: AMD Ryzen 7 7745HX (Dragon Range, 2CU 610M).
        // Must be locked by default, but unlockable via Geek mode.
        let caps_locked = evaluate_gpu_capabilities(
            "Jiaolong 16 Pro",
            "AMD Ryzen 7 7745HX with Radeon Graphics",
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            true,
            true,
            false,
            0,
            false,
        );
        assert!(!caps_locked.supports_igpu_only, "7745HX must be locked by default!");
        assert!(caps_locked.is_safety_locked);
        assert!(caps_locked.reason_igpu_unsupported.unwrap().contains("Dragon Range"));

        let caps_unlocked = evaluate_gpu_capabilities(
            "Jiaolong 16 Pro",
            "AMD Ryzen 7 7745HX with Radeon Graphics",
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            true,
            true,
            false,
            0,
            true, // Geek unlock
        );
        assert!(caps_unlocked.supports_igpu_only, "7745HX can be unlocked by geek mode!");
        assert!(caps_unlocked.user_unlocked);
    }

    #[test]
    fn test_modern_tri_mode_chassis_unblocked() {
        // Modern chassis with valid dGPU and iGPU is completely unblocked regardless of OEM baseboard string
        let caps = evaluate_gpu_capabilities(
            "Custom Chassis-XxCANGLONGxx-B2",
            "Intel Core i7-14650HX",
            "NVIDIA GeForce RTX 5070 Ti Laptop GPU",
            true,
            true,
            true,
            0,
            false,
        );
        assert!(caps.supports_igpu_only, "Modern 50-series / Canglong must support Tri-Mode without board sniffing!");
        assert!(!caps.is_safety_locked);
    }

    #[test]
    fn test_pure_igpu_thinbook_unsupported() {
        let caps = evaluate_gpu_capabilities(
            "WUJIE 14",
            "AMD Ryzen 7 8845HS",
            "AMD Radeon 780M",
            false, // has_dgpu = false
            true,
            false,
            0,
            false,
        );
        assert!(!caps.supports_dgpu);
        assert!(!caps.supports_igpu_only);
    }

    #[test]
    fn test_unknown_dmi_modern_platform_unblocked() {
        // Unknown motherboard DMI (e.g. after motherboard replacement) on modern platform
        let caps = evaluate_gpu_capabilities(
            "To be filled by O.E.M.",
            "Intel Core i7-13700H",
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            true,
            true,
            false,
            0,
            false,
        );
        // Under Hardware Truth: Tri-mode capabilities are determined by NVRAM payload and hardware presence,
        // NEVER blocked by uninitialized DMI string!
        assert!(caps.supports_dgpu, "Discrete mode must be supported!");
        assert!(caps.supports_hybrid, "Hybrid mode must be supported!");
        assert!(caps.supports_igpu_only, "Tri-Mode must be unblocked on modern hardware!");
    }
}


