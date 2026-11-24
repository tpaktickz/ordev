// OpenRevo - Tongfang Cloud ICC Color Profile Discovery & Retrieval Engine
// Pure ASCII Policy strictly enforced.
// 100% Native Win32 API implementation (Zero PowerShell).
// Probes local hardware identity (EDID PnP ID, Panel SN, Motherboard BIOS SN, Physical MAC)
// and queries Uniwill / Aistone OEM cloud calibration servers to retrieve factory ICC/ICM profiles.

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(windows)]
use windows::core::{w, PCSTR, PCWSTR};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesA, DISPLAY_DEVICEA, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
};
#[cfg(windows)]
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_READ, REG_BINARY, REG_SZ,
};
#[cfg(windows)]
use std::ffi::c_void;

pub const SERVER_UNIWILL: &str = "http://iccprofile.uniwill.com.tw/api/iccprofile/";
pub const SERVER_AISTONE: &str = "http://iccprofile.aistoneglobal.com:8001/api/iccprofile/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudIccMonitorInfo {
    pub pnp: String,
    pub panel_serial: String,
    pub friendly_name: String,
    pub instance_name: String,
    pub is_internal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudIccIdentity {
    pub bios_serial: String,
    pub mac_addresses: Vec<String>,
    pub monitors: Vec<CloudIccMonitorInfo>,
    pub is_50_series: bool,
    pub arch_name: String,
    pub recommended_method: String,
    pub candidates_method1: Vec<String>, // <pnp>_<sn> (50 series)
    pub candidates_method2: Vec<String>, // <pnp>_<mac> (40 series)
    pub candidates_fallback: Vec<String>, // <pnp> / friendly
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedIccItem {
    pub file_name: String,
    pub file_size: usize,
    pub file_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudIccFetchHit {
    pub candidate: String,
    pub server_url: String,
    pub full_url: String,
    pub file_name: String,
    pub is_zip: bool,
    pub file_size: usize,
    pub raw_bytes: Vec<u8>,
    pub icc_profiles: Vec<ExtractedIccItem>,
}

// ----------------------------------------------------------------------------
// Win32 IP Helper API: Query Physical MAC Addresses without PowerShell (0ms)
// ----------------------------------------------------------------------------

#[cfg(windows)]
#[repr(C)]
struct IpAdapterAddressesLh {
    length: u32,
    if_index: u32,
    next: *mut IpAdapterAddressesLh,
    adapter_name: *const i8,
    first_unicast_address: *mut c_void,
    first_anycast_address: *mut c_void,
    first_multicast_address: *mut c_void,
    first_dns_server_address: *mut c_void,
    dns_suffix: *const u16,
    description: *const u16,
    friendly_name: *const u16,
    physical_address: [u8; 8],
    physical_address_length: u32,
    flags: u32,
    mtu: u32,
    if_type: u32,
    oper_status: u32,
}

#[cfg(windows)]
#[link(name = "iphlpapi")]
extern "system" {
    fn GetAdaptersAddresses(
        family: u32,
        flags: u32,
        reserved: *mut c_void,
        adapter_addresses: *mut IpAdapterAddressesLh,
        size_pointer: *mut u32,
    ) -> u32;
}

#[cfg(windows)]
pub fn get_physical_mac_addresses() -> Vec<String> {
    let mut macs = Vec::new();
    let mut buf_len = 0u32;
    unsafe {
        let _ = GetAdaptersAddresses(0, 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut buf_len);
        if buf_len == 0 {
            return macs;
        }
        let mut buffer: Vec<u8> = vec![0; buf_len as usize];
        let res = GetAdaptersAddresses(
            0,
            0,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut IpAdapterAddressesLh,
            &mut buf_len,
        );
        if res == 0 {
            let mut curr = buffer.as_ptr() as *const IpAdapterAddressesLh;
            while !curr.is_null() {
                let item = &*curr;
                // 6 is MIB_IF_TYPE_ETHERNET, 71 is IF_TYPE_IEEE80211
                if item.physical_address_length == 6 && (item.if_type == 6 || item.if_type == 71) {
                    let hex = format!(
                        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                        item.physical_address[0],
                        item.physical_address[1],
                        item.physical_address[2],
                        item.physical_address[3],
                        item.physical_address[4],
                        item.physical_address[5],
                    );
                    if hex != "000000000000" && !macs.contains(&hex) {
                        macs.push(hex);
                    }
                }
                curr = item.next;
            }
        }
    }
    macs
}

// ----------------------------------------------------------------------------
// Win32 Registry API: Read BIOS Serial Number (0ms)
// ----------------------------------------------------------------------------

#[cfg(windows)]
pub fn get_bios_serial_number() -> String {
    let read_reg = |sub: PCWSTR, val: PCWSTR| -> Option<String> {
        unsafe {
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub, 0, KEY_READ, &mut hkey).is_ok() {
                let mut data_type = REG_SZ;
                let mut buf = [0u16; 256];
                let mut data_size = (buf.len() * 2) as u32;
                let status = RegQueryValueExW(
                    hkey,
                    val,
                    None,
                    Some(&mut data_type),
                    Some(buf.as_mut_ptr() as *mut u8),
                    Some(&mut data_size),
                );
                let _ = RegCloseKey(hkey);
                if status.is_ok() {
                    let len = (data_size / 2) as usize;
                    let trimmed_len = if len > 0 && buf[len - 1] == 0 { len - 1 } else { len };
                    let s = String::from_utf16_lossy(&buf[..trimmed_len]).trim().to_string();
                    if !s.is_empty() && s != "None" && s != "Default string" && s != "Standard" {
                        return Some(s);
                    }
                }
            }
        }
        None
    };

    if let Some(sn) = read_reg(w!(r"HARDWARE\DESCRIPTION\System\BIOS"), w!("SystemSerialNumber")) {
        return sn;
    }
    if let Some(sn) = read_reg(w!(r"HARDWARE\DESCRIPTION\System\BIOS"), w!("BaseBoardSerialNumber")) {
        return sn;
    }
    String::new()
}

// ----------------------------------------------------------------------------
// Win32 EDID Parser: Read Panel Serial Number from Registry EDID (0ms)
// ----------------------------------------------------------------------------

#[cfg(windows)]
fn get_edid_serial_number(pnp: &str) -> Option<String> {
    unsafe {
        let pnp_w: Vec<u16> = format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\0", pnp)
            .encode_utf16()
            .collect();
        let mut h_pnp = HKEY::default();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(pnp_w.as_ptr()), 0, KEY_READ, &mut h_pnp).is_err() {
            return None;
        }

        let mut sub_name = [0u16; 256];
        let mut sub_len = sub_name.len() as u32;
        let mut found_sn: Option<String> = None;

        if RegEnumKeyExW(
            h_pnp,
            0,
            windows::core::PWSTR(sub_name.as_mut_ptr()),
            &mut sub_len,
            None,
            windows::core::PWSTR::null(),
            None,
            None,
        )
        .is_ok()
        {
            let sub_str = String::from_utf16_lossy(&sub_name[..sub_len as usize]);
            let params_path = format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\\{}\\Device Parameters\0", pnp, sub_str);
            let params_w: Vec<u16> = params_path.encode_utf16().collect();
            let mut h_params = HKEY::default();

            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(params_w.as_ptr()), 0, KEY_READ, &mut h_params).is_ok() {
                let mut edid_buf = [0u8; 512];
                let mut edid_len = edid_buf.len() as u32;
                let mut data_type = REG_BINARY;
                if RegQueryValueExW(
                    h_params,
                    w!("EDID"),
                    None,
                    Some(&mut data_type),
                    Some(edid_buf.as_mut_ptr()),
                    Some(&mut edid_len),
                )
                .is_ok() && edid_len >= 128
                {
                    // Scan standard EDID descriptor blocks for 0x00, 0x00, 0x00, 0xFF (Serial Number)
                    for off in [54, 72, 90, 108] {
                        if off + 18 <= edid_len as usize
                            && edid_buf[off] == 0
                            && edid_buf[off + 1] == 0
                            && edid_buf[off + 2] == 0
                            && edid_buf[off + 3] == 0xFF
                        {
                            let sn_bytes = &edid_buf[off + 5..off + 18];
                            let sn_str = String::from_utf8_lossy(sn_bytes).trim().to_string();
                            if !sn_str.is_empty() {
                                found_sn = Some(sn_str);
                                break;
                            }
                        }
                    }
                }
                let _ = RegCloseKey(h_params);
            }
        }
        let _ = RegCloseKey(h_pnp);
        found_sn
    }
}

#[cfg(windows)]
fn cstr_to_string(bytes: &[i8]) -> String {
    let u8_slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, bytes.len()) };
    let len = u8_slice.iter().position(|&c| c == 0).unwrap_or(u8_slice.len());
    String::from_utf8_lossy(&u8_slice[..len]).trim().to_string()
}

#[cfg(windows)]
pub fn get_monitors_for_cloud_icc() -> Vec<CloudIccMonitorInfo> {
    let mut list = Vec::new();
    unsafe {
        let mut dev_num = 0u32;
        let mut adapter: DISPLAY_DEVICEA = std::mem::zeroed();
        adapter.cb = std::mem::size_of::<DISPLAY_DEVICEA>() as u32;

        while EnumDisplayDevicesA(None, dev_num, &mut adapter, 0).as_bool() {
            if (adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP) != 0 {
                let name = cstr_to_string(&adapter.DeviceName);
                if let Ok(c_name) = CString::new(name.clone()) {
                    let pc_name = PCSTR(c_name.as_ptr() as *const u8);
                    let mut mon_dev: DISPLAY_DEVICEA = std::mem::zeroed();
                    mon_dev.cb = std::mem::size_of::<DISPLAY_DEVICEA>() as u32;
                    let mut mon_idx = 0u32;
                    while mon_idx < 4 {
                        if EnumDisplayDevicesA(pc_name, mon_idx, &mut mon_dev, 0).as_bool() {
                            let m_id = cstr_to_string(&mon_dev.DeviceID);
                            let m_str = cstr_to_string(&mon_dev.DeviceString);
                            if !m_id.is_empty() {
                                // Extract PnP ID from DeviceID: "MONITOR\BOE0B87\{...}\..."
                                let parts: Vec<&str> = m_id.split('\\').collect();
                                let pnp = if parts.len() > 1 {
                                    parts[1].to_string()
                                } else {
                                    m_id.clone()
                                };
                                let is_internal = m_id.contains("UID4353")
                                    || m_id.contains("INTERNAL")
                                    || list.is_empty();

                                let panel_serial = get_edid_serial_number(&pnp).unwrap_or_default();

                                list.push(CloudIccMonitorInfo {
                                    pnp,
                                    panel_serial,
                                    friendly_name: if !m_str.is_empty() {
                                        m_str
                                    } else {
                                        name.clone()
                                    },
                                    instance_name: m_id,
                                    is_internal,
                                });
                            }
                        }
                        mon_idx += 1;
                    }
                }
            }
            adapter = std::mem::zeroed();
            adapter.cb = std::mem::size_of::<DISPLAY_DEVICEA>() as u32;
            dev_num += 1;
        }
    }
    list
}

// ----------------------------------------------------------------------------
// Linux sources: sysfs/DMI/DRM instead of IP Helper, the registry and GDI
// ----------------------------------------------------------------------------

/// Physical MAC addresses, upper-case, no separators.
///
/// Only real link-layer devices: `lo` and the common virtual prefixes are
/// skipped, and the Uniwill servers key on physical NICs.
#[cfg(not(windows))]
pub fn get_physical_mac_addresses() -> Vec<String> {
    let mut macs = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return macs;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for name in names {
        if name == "lo"
            || name.starts_with("docker")
            || name.starts_with("veth")
            || name.starts_with("virbr")
            || name.starts_with("br-")
        {
            continue;
        }
        let Ok(addr) = fs::read_to_string(format!("/sys/class/net/{}/address", name)) else {
            continue;
        };
        let hex: String = addr.trim().replace(':', "").to_uppercase();
        if hex.len() == 12 && hex != "000000000000" && !macs.contains(&hex) {
            macs.push(hex);
        }
    }
    macs
}

/// BIOS serial number from DMI.
#[cfg(not(windows))]
pub fn get_bios_serial_number() -> String {
    for path in [
        "/sys/class/dmi/id/board_serial",
        "/sys/class/dmi/id/product_serial",
    ] {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty() && trimmed != "None" && trimmed != "Default string" && trimmed != "Standard" {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// Manufacturer PnP ID (three letters, e.g. `BOE`) encoded in EDID bytes 8..10.
#[cfg(not(windows))]
fn edid_pnp(edid: &[u8]) -> Option<String> {
    if edid.len() < 16 {
        return None;
    }
    let word = ((edid[8] as u16) << 8) | edid[9] as u16;
    let mut out = String::with_capacity(3);
    for shift in [10u16, 5, 0] {
        let value = ((word >> shift) & 0x1f) as u8;
        if !(1..=26).contains(&value) {
            return None;
        }
        out.push((b'A' + value - 1) as char);
    }
    Some(out)
}

/// Panel serial number for the panel whose EDID carries `pnp`.
///
/// The 32-bit serial lives at EDID offset 0x0C; when a vendor ships the serial
/// as a text descriptor (0xFF) that is used instead.
#[cfg(not(windows))]
fn get_edid_serial_number(pnp: &str) -> Option<String> {
    for (connector, edid) in read_all_edids() {
        if edid_pnp(&edid).as_deref() != Some(pnp) {
            continue;
        }
        let numeric = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
        if numeric != 0 {
            return Some(format!("{:08X}", numeric));
        }
        for offset in (0..edid.len().saturating_sub(4)).step_by(18) {
            if edid[offset..offset + 4] == [0x00, 0x00, 0x00, 0xFF] {
                let text: String = edid[offset + 5..offset + 18]
                    .iter()
                    .take_while(|&&b| b != 0x0A && b != 0)
                    .map(|&b| b as char)
                    .collect();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        log_debug_sample(&connector);
    }
    None
}

#[cfg(not(windows))]
fn log_debug_sample(connector: &str) {
    crate::log_debug!("CLOUD_ICC", "Panel {} has no serial descriptor", connector);
}

#[cfg(not(windows))]
fn read_all_edids() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return out;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains('-'))
        .collect();
    names.sort();
    for name in names {
        let dir = format!("/sys/class/drm/{}", name);
        if fs::read_to_string(format!("{}/status", dir))
            .map(|s| s.trim() != "connected")
            .unwrap_or(true)
        {
            continue;
        }
        if let Ok(edid) = fs::read(format!("{}/edid", dir)) {
            if edid.len() >= 128 {
                out.push((name, edid));
            }
        }
    }
    out
}

#[cfg(not(windows))]
pub fn get_monitors_for_cloud_icc() -> Vec<CloudIccMonitorInfo> {
    let mut list = Vec::new();
    for (connector, edid) in read_all_edids() {
        let Some(pnp) = edid_pnp(&edid) else {
            continue;
        };
        let is_internal = connector.contains("eDP") || connector.contains("LVDS") || list.is_empty();
        let panel_serial = get_edid_serial_number(&pnp).unwrap_or_default();
        list.push(CloudIccMonitorInfo {
            pnp,
            panel_serial,
            friendly_name: connector.clone(),
            instance_name: connector,
            is_internal,
        });
    }
    list
}

/// Retrieve machine identity for cloud ICC lookup using 100% native Win32 APIs (0ms, 0 PowerShell)
pub fn get_cloud_icc_identity() -> Result<CloudIccIdentity, String> {
    let bios_serial = get_bios_serial_number();
    let macs = get_physical_mac_addresses();
    let monitors = get_monitors_for_cloud_icc();

    // Build candidate arrays
    let mut c1 = Vec::new();
    let mut c2 = Vec::new();
    let mut c3 = Vec::new();

    for mon in &monitors {
        let id = &mon.pnp;
        if id.is_empty() {
            continue;
        }

        // Method 1 (50 series): <pnp>_<sn>
        if !mon.panel_serial.is_empty() {
            c1.push(format!("{}_{}", id, mon.panel_serial));
        }
        if !bios_serial.is_empty() {
            c1.push(format!("{}_{}", id, bios_serial));
        }

        // Method 2 (40 series): <pnp>_<mac>
        for mac in &macs {
            c2.push(format!("{}_{}", id, mac));
        }

        // Fallback: <pnp> / <friendly>
        c3.push(id.clone());
        if !mon.friendly_name.is_empty() && &mon.friendly_name != id {
            c3.push(mon.friendly_name.clone());
        }
    }

    // Deduplicate while preserving order
    let dedup = |vec: Vec<String>| -> Vec<String> {
        let mut out = Vec::new();
        for item in vec {
            if !out.contains(&item) {
                out.push(item);
            }
        }
        out
    };

    // Query declarative hardware ground truth (OemMagicVariable vs UniWillVariable)
    let is_50 = crate::driver::wmi_ec::is_nextgen_oem_magic_ec();
    let arch_name = if is_50 {
        "OemMagicVariable (50 series)".to_string()
    } else {
        "UniWillVariable (40 series)".to_string()
    };
    let recommended_method = if is_50 { "50".to_string() } else { "40".to_string() };

    Ok(CloudIccIdentity {
        bios_serial,
        mac_addresses: macs,
        monitors,
        is_50_series: is_50,
        arch_name,
        recommended_method,
        candidates_method1: dedup(c1),
        candidates_method2: dedup(c2),
        candidates_fallback: dedup(c3),
    })
}

/// Unpack a retrieved archive.
///
/// No zip crate in the tree: shell out, preferring `bsdtar` (libarchive handles
/// both zip and 7z) and falling back to `unzip`.
fn unpack_zip_native(archive: &Path, dest: &Path) -> Result<(), String> {
    let archive = archive.to_string_lossy().to_string();
    let dest = dest.to_string_lossy().to_string();

    #[cfg(windows)]
    let candidates: [(&str, Vec<&str>); 1] = [("tar.exe", vec!["-xf", &archive, "-C", &dest])];
    #[cfg(not(windows))]
    let candidates: [(&str, Vec<&str>); 2] = [
        ("bsdtar", vec!["-xf", &archive, "-C", &dest]),
        ("unzip", vec!["-o", "-q", &archive, "-d", &dest]),
    ];

    let mut last_err = String::from("no archive tool available");
    for (program, args) in candidates {
        match crate::pal::process::command(program).args(&args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => last_err = format!("{} exited with {}", program, status),
            Err(err) => last_err = format!("{}: {}", program, err),
        }
    }
    Err(last_err)
}

/// Collect all .icm and .icc files recursively from a directory
fn collect_icc_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(collect_icc_files_recursive(&path));
            } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                let e = ext.to_lowercase();
                if e == "icm" || e == "icc" {
                    results.push(path);
                }
            }
        }
    }
    results
}

/// Verify whether byte array is a valid ICC color profile (signature 'acsp' at offset 36..40)
pub fn is_valid_icc_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 128 {
        return false;
    }
    // ICC Specification: bytes 36..40 contains ASCII signature 'acsp'
    &bytes[36..40] == b"acsp"
}

/// Probe a single candidate against a specific server URL across 3 extensions ('', '.zip', '.icm').
pub fn probe_single_candidate(
    server_base: &str,
    candidate: &str,
    timeout_secs: u64,
) -> Option<CloudIccFetchHit> {
    let base = if server_base.ends_with('/') {
        server_base.to_string()
    } else {
        format!("{}/", server_base)
    };

    let suffixes = ["", ".zip", ".icm"];
    for suffix in suffixes {
        let full_url = format!("{}{}{}", base, candidate, suffix);
        let resp = match ureq::get(&full_url)
            .timeout(Duration::from_secs(timeout_secs))
            .call()
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        if resp.status() != 200 {
            continue;
        }

        let mut bytes = Vec::new();
        if resp.into_reader().read_to_end(&mut bytes).is_err() || bytes.is_empty() {
            continue;
        }

        let is_zip = bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4B;
        let is_icc = is_valid_icc_bytes(&bytes);

        if is_zip {
            // Write to temporary zip and extract
            let temp_dir = std::env::temp_dir().join(format!("icc_extract_{}_{}", std::process::id(), fastrand()));
            let temp_zip = temp_dir.join("archive.zip");
            let _ = fs::create_dir_all(&temp_dir);
            if fs::write(&temp_zip, &bytes).is_err() {
                let _ = fs::remove_dir_all(&temp_dir);
                continue;
            }

            let extract_dir = temp_dir.join("out");
            let _ = fs::create_dir_all(&extract_dir);
            let mut extracted_items = Vec::new();

            if unpack_zip_native(&temp_zip, &extract_dir).is_ok() {
                let icc_paths = collect_icc_files_recursive(&extract_dir);
                for path in icc_paths {
                    if path.metadata().map(|m| m.len()).unwrap_or(0) > 128 {
                        if let Ok(content) = fs::read(&path) {
                            if is_valid_icc_bytes(&content) {
                                let fname = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("profile.icm")
                                    .to_string();
                                let size = content.len();
                                extracted_items.push(ExtractedIccItem {
                                    file_name: fname,
                                    file_size: size,
                                    file_bytes: content,
                                });
                            }
                        }
                    }
                }
            }
            let _ = fs::remove_dir_all(&temp_dir);

            if !extracted_items.is_empty() {
                let safe_zip_name = format!("{}.zip", candidate.trim_end_matches(".zip"));
                let total_size = bytes.len();
                return Some(CloudIccFetchHit {
                    candidate: candidate.to_string(),
                    server_url: server_base.to_string(),
                    full_url,
                    file_name: safe_zip_name,
                    is_zip: true,
                    file_size: total_size,
                    raw_bytes: bytes,
                    icc_profiles: extracted_items,
                });
            }
        } else if is_icc {
            let size = bytes.len();
            let safe_name = format!("{}.icm", candidate.trim_end_matches(".icm"));
            let single_item = ExtractedIccItem {
                file_name: safe_name.clone(),
                file_size: size,
                file_bytes: bytes.clone(),
            };
            return Some(CloudIccFetchHit {
                candidate: candidate.to_string(),
                server_url: server_base.to_string(),
                full_url,
                file_name: safe_name,
                is_zip: false,
                file_size: size,
                raw_bytes: bytes,
                icc_profiles: vec![single_item],
            });
        }
    }
    None
}

fn fastrand() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
