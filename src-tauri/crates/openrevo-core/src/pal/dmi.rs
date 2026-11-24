// Machine identity from the firmware tables (DMI / SMBIOS).
//
// On Linux the kernel already decoded SMBIOS into `/sys/class/dmi/id`; on Windows
// the same strings land in the registry under `HARDWARE\DESCRIPTION\System\BIOS`
// when firmware hands them to the OS loader. Both are read through the PAL so
// model detection never guesses from PCI ids alone.

fn trim_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Firmware that was never provisioned by the OEM ships these placeholders.
    let placeholder = matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "none"
            | "default string"
            | "to be filled by o.e.m."
            | "to be filled by o.e.m"
            | "system manufacturer"
            | "system product name"
            | "system version"
            | "not applicable"
            | "not available"
            | "unknown"
            | "o.e.m."
    );
    if placeholder {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(target_os = "linux")]
fn read_dmi_file(name: &str) -> Option<String> {
    let path = format!("/sys/class/dmi/id/{}", name);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| trim_value(&raw))
}

#[cfg(target_os = "linux")]
pub fn sys_vendor() -> Option<String> {
    read_dmi_file("sys_vendor")
}

#[cfg(target_os = "linux")]
pub fn product_name() -> Option<String> {
    read_dmi_file("product_name")
}

#[cfg(target_os = "linux")]
pub fn product_version() -> Option<String> {
    read_dmi_file("product_version")
}

#[cfg(target_os = "linux")]
pub fn board_name() -> Option<String> {
    read_dmi_file("board_name")
}

#[cfg(target_os = "linux")]
pub fn bios_version() -> Option<String> {
    read_dmi_file("bios_version")
}

#[cfg(windows)]
fn read_bios_registry(value_name: &str) -> Option<String> {
    crate::pal::reg::read_string(
        crate::pal::reg::RegRoot::Machine,
        r"HARDWARE\DESCRIPTION\System\BIOS",
        value_name,
    )
    .as_deref()
    .and_then(trim_value)
}

#[cfg(windows)]
pub fn sys_vendor() -> Option<String> {
    read_bios_registry("SystemManufacturer")
}

#[cfg(windows)]
pub fn product_name() -> Option<String> {
    read_bios_registry("SystemProductName")
}

#[cfg(windows)]
pub fn product_version() -> Option<String> {
    read_bios_registry("SystemVersion")
}

#[cfg(windows)]
pub fn board_name() -> Option<String> {
    read_bios_registry("BaseBoardProduct")
}

#[cfg(windows)]
pub fn bios_version() -> Option<String> {
    read_bios_registry("BIOSVersion")
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn sys_vendor() -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn product_name() -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn product_version() -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn board_name() -> Option<String> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn bios_version() -> Option<String> {
    None
}

/// OEM names that ship Tongfang / Uniwill barebones.
///
/// These are rebadgers of the same ODM platform: MechRevo, Machenike, Hasee and
/// Thunderobot sell it in China; XMG, Tuxedo, Slimbook, Eluktronics, Aftershock,
/// Infinity and PC Specialist sell the same boards elsewhere.
const TONGFANG_BRANDS: &[&str] = &[
    "tongfang",
    "mechrevo",
    "machenike",
    "uniwill",
    "hasee",
    "thunderobot",
    "xmg",
    "tuxedo",
    "slimbook",
    "eluktronics",
    "aftershock",
    "infinity",
    "pcspecialist",
    "monster",
    "schenker",
    "bestware",
];

/// True when the firmware identifies one of the Tongfang / Uniwill rebadges.
///
/// This only says "the chassis is ours to drive"; whether an EC channel is
/// actually reachable is a separate probe, because a Tongfang board with the
/// `ec_sys` module unloaded has identity but no transport.
pub fn is_tongfang_family() -> bool {
    let haystack = [sys_vendor(), product_name(), board_name()]
        .into_iter()
        .flatten()
        .collect::<Vec<String>>()
        .join(" ")
        .to_ascii_lowercase();
    if haystack.is_empty() {
        return false;
    }
    TONGFANG_BRANDS
        .iter()
        .any(|brand| haystack.contains(brand))
}

/// One-line identity for logs: `vendor / product (board)`.
pub fn identity_summary() -> String {
    let vendor = sys_vendor().unwrap_or_else(|| "unknown-vendor".to_string());
    let product = product_name().unwrap_or_else(|| "unknown-product".to_string());
    match board_name() {
        Some(board) => format!("{} / {} ({})", vendor, product, board),
        None => format!("{} / {}", vendor, product),
    }
}
