// Tongfang MIFS WMI kernel interface (Linux only).
//
// When the `tongfang-mifs-wmi` driver is loaded, the kernel owns the EC mailbox:
// it serialises the ACPI EC transactions, decodes hotkey events, and publishes
// everything as ordinary sysfs nodes. That is strictly better than poking the
// EC ourselves through `ec_sys`, because concurrent EC access corrupts the
// mailbox — so every subsystem prefers this tier when it exists and only falls
// back to `pal::ec` (raw `ec_sys` / port I/O) when the module is absent.
//
// Nothing here hardcodes a device path: the driver may publish under the WMI bus,
// under the platform bus, or both depending on the kernel revision, so nodes are
// discovered by looking for their attributes.
//
// Reference ABI (from the `platform/x86: tongfang-mifs-wmi` patch series):
//   hwmon chip name .......... "tongfang_mifs"
//   temp1_input .............. CPU package temperature, millidegrees Celsius
//   fan1_input/fan2_input .... CPU / GPU fan RPM, fan3_input = system fan
//   fanN_label ............... "CPU" / "GPU" / "SYS"
//   fan_boost ................ write-only, 1 = max fan override
//   gpu_mode ................. read/write, "hybrid" | "discrete" | "uma"
//   kb_mode .................. read/write, RGB effect id
//   platform_profile ......... the standard ACPI platform profile knob
//   leds/*kbd_backlight/ ..... monochrome keyboard backlight, max 3

use std::path::{Path, PathBuf};

const HWMON_NAME: &str = "tongfang_mifs";

/// Attribute files the kernel driver exposes on its device node. Used only to
/// locate the node; presence of any one of them identifies the driver.
const DRIVER_ATTRS: &[&str] = &["fan_boost", "gpu_mode", "kb_mode"];

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_u32(path: &Path) -> Option<u32> {
    read_trimmed(path)?.parse::<u32>().ok()
}

fn write_str(path: &Path, value: &str) -> bool {
    std::fs::write(path, value).is_ok()
}

/// Locate the directory that carries the driver's own attributes.
///
/// `walkdir` is not a dependency here on purpose: the search space is two
/// directories, one level deep, so a plain scan is cheaper than the crate.
fn driver_dir() -> Option<PathBuf> {
    for root in ["/sys/bus/wmi/devices", "/sys/devices/platform"] {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if DRIVER_ATTRS.iter().any(|attr| dir.join(attr).exists()) {
                return Some(dir);
            }
        }
    }
    None
}

fn hwmon_dir() -> Option<PathBuf> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if read_trimmed(&dir.join("name")).as_deref() == Some(HWMON_NAME) {
            return Some(dir);
        }
    }
    None
}

fn kbd_backlight_dir() -> Option<PathBuf> {
    let entries = std::fs::read_dir("/sys/class/leds").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.contains("kbd_backlight") && dir.join("brightness").exists() {
            return Some(dir);
        }
    }
    None
}

/// True when the kernel driver is loaded and publishing attributes.
pub fn driver_present() -> bool {
    driver_dir().is_some() || hwmon_dir().is_some()
}

/// One-line description for logs, naming which nodes were found.
pub fn describe() -> String {
    let mut parts = Vec::new();
    if let Some(dir) = driver_dir() {
        parts.push(format!("attrs={}", dir.display()));
    }
    if let Some(dir) = hwmon_dir() {
        parts.push(format!("hwmon={}", dir.display()));
    }
    if let Some(dir) = kbd_backlight_dir() {
        parts.push(format!("leds={}", dir.display()));
    }
    if parts.is_empty() {
        format!("tongfang-mifs-wmi not loaded ({} none)", HWMON_NAME)
    } else {
        parts.join(" ")
    }
}

/// CPU package temperature in Celsius, from the kernel's millidegree value.
///
/// The Tongfang hwmon is preferred when the vendor WMI driver is loaded, but that
/// driver is not installed on most distros — on this chassis (MECHREVO JIGUANG,
/// kernel 7.3.6) `/sys/class/hwmon/*/name` only ever reports `coretemp`, so the
/// vendor node alone made every temperature read `None` and silently disabled the
/// fan-table verdict check. Fall back to the generic CPU sensors instead.
pub fn read_cpu_temp_celsius() -> Option<u8> {
    if let Some(dir) = hwmon_dir() {
        if let Some(milli) = read_u32(&dir.join("temp1_input")) {
            return Some((milli / 1000).min(255) as u8);
        }
    }
    crate::system::cpu_monitor::linux_cpu_temp_celsius()
        .map(|c| c.clamp(0, 255) as u8)
}

/// (CPU, GPU) fan RPM from hwmon. The system fan (fan3) is intentionally
/// dropped: the UI only has two dials, and mixing a chassis fan into the GPU
/// reading would show numbers that do not match the fan curves we program.
pub fn read_fan_rpm() -> Option<(u16, u16)> {
    let dir = hwmon_dir()?;
    let cpu = read_u32(&dir.join("fan1_input"))?;
    let gpu = read_u32(&dir.join("fan2_input"))?;
    Some((cpu.min(u16::MAX as u32) as u16, gpu.min(u16::MAX as u32) as u16))
}

/// Fan boost is a write-only attribute in the driver ABI.
pub fn set_fan_boost(active: bool) -> bool {
    match driver_dir() {
        Some(dir) => write_str(&dir.join("fan_boost"), if active { "1" } else { "0" }),
        None => false,
    }
}

/// GPU MUX mode. Values follow the driver ABI, not the Windows UI vocabulary.
pub fn read_gpu_mode() -> Option<String> {
    read_trimmed(&driver_dir()?.join("gpu_mode"))
}

pub fn set_gpu_mode(mode: &str) -> bool {
    if !matches!(mode, "hybrid" | "discrete" | "uma") {
        return false;
    }
    match driver_dir() {
        Some(dir) => write_str(&dir.join("gpu_mode"), mode),
        None => false,
    }
}

/// RGB keyboard effect id (0 off, 1 cyclic, 2 fixed, 3 custom).
pub fn set_keyboard_effect(effect: u8) -> bool {
    match driver_dir() {
        Some(dir) => write_str(&dir.join("kb_mode"), &effect.to_string()),
        None => false,
    }
}

/// Monochrome backlight level from the LED class node.
pub fn read_backlight_level() -> Option<u8> {
    let dir = kbd_backlight_dir()?;
    let raw = read_u32(&dir.join("brightness"))?;
    let max = read_u32(&dir.join("max_brightness")).unwrap_or(3).max(1);
    // Normalise to the 0..=3 scale the UI speaks.
    Some(((raw * 3) / max).min(3) as u8)
}

pub fn set_backlight_level(level: u8) -> bool {
    let dir = match kbd_backlight_dir() {
        Some(d) => d,
        None => return false,
    };
    let max = read_u32(&dir.join("max_brightness")).unwrap_or(3).max(1);
    let raw = ((level.min(3) as u32) * max) / 3;
    write_str(&dir.join("brightness"), &raw.to_string())
}

/// Valid values of the ACPI platform profile knob, as the kernel reports them.
pub fn platform_profile_choices() -> Vec<String> {
    read_trimmed(Path::new("/sys/firmware/acpi/platform_profile_choices"))
        .map(|s| s.split_whitespace().map(|v| v.to_string()).collect())
        .unwrap_or_default()
}

/// Last accepted profile, or None when the knob is absent.
pub fn read_platform_profile() -> Option<String> {
    read_trimmed(Path::new("/sys/firmware/acpi/platform_profile"))
}

/// Write the first choice that the kernel advertises.
///
/// The kernel rejects values outside its table, and the table differs per
/// chassis (`quiet` on some Tongfang boards, `low-power` on others), so callers
/// pass a preference list rather than a single value.
pub fn set_platform_profile(preferred: &[&str]) -> Option<String> {
    let choices = platform_profile_choices();
    if choices.is_empty() {
        return None;
    }
    for candidate in preferred {
        if choices.iter().any(|c| c == candidate) {
            if write_str(Path::new("/sys/firmware/acpi/platform_profile"), candidate) {
                return Some((*candidate).to_string());
            }
            return None;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Non-Linux twins
//
// The module is a kernel interface, so on Windows every function reports
// "absent" instead of being gated at every call site. Callers then fall through
// to the EC/WMI path without knowing which OS they are on.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "linux"))]
pub fn driver_present() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn describe() -> String {
    "tongfang-mifs-wmi is a Linux kernel interface (absent on this platform)".to_string()
}

#[cfg(not(target_os = "linux"))]
pub fn read_cpu_temp_celsius() -> Option<u8> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn read_fan_rpm() -> Option<(u16, u16)> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn set_fan_boost(_active: bool) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn read_gpu_mode() -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn set_gpu_mode(_mode: &str) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn set_keyboard_effect(_effect: u8) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn read_backlight_level() -> Option<u8> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn set_backlight_level(_level: u8) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn platform_profile_choices() -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn read_platform_profile() -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn set_platform_profile(_preferred: &[&str]) -> Option<String> {
    None
}
