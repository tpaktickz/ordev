//! Gamma ramps and ICC profile association — portable surface.
//!
//! Two backends, one API:
//! - **Windows**: GDI device contexts plus the WCS `ICM\ProfileAssociations`
//!   registry keys, which is exactly what the MMC color panel writes.
//! - **Linux**: the DRM connector `gamma` attribute for the ramp, and an XDG
//!   data directory plus a sidecar association file for profiles. colord is the
//!   freedesktop-standard owner of these operations and the intended P3
//!   replacement; nothing here is KDE- or GNOME-specific, so the shim keeps
//!   working on any compositor until then.

use std::path::PathBuf;

#[repr(C)]
pub struct GammaRamp {
    pub red: [u16; 256],
    pub green: [u16; 256],
    pub blue: [u16; 256],
}

/// A display the color engine can address.
#[derive(Clone, Debug)]
pub struct DisplayDevice {
    /// GDI device name (`\\.\DISPLAY1`) or DRM connector name (`card0-DP-1`).
    pub name: String,
    /// Windows monitor class index (`0002`); `None` on Linux.
    pub class_index: Option<String>,
    /// Currently attached to the desktop / connected.
    pub attached: bool,
}

/// Where user-installed profiles live.
pub fn profiles_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("OpenRevo")
            .join("profiles")
    }
    #[cfg(not(windows))]
    {
        crate::pal::xdg::data_dir().join("profiles")
    }
}

// --------------------------------------------------------------------------
// Windows backend
// --------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use super::{DisplayDevice, GammaRamp};
    use std::path::Path;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::{BOOL, ERROR_SUCCESS};
    use windows::Win32::Graphics::Gdi::HDC;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_MULTI_SZ,
        REG_OPEN_CREATE_OPTIONS, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExA,
        RegOpenKeyExA, RegOpenKeyExW, RegQueryValueExA, RegQueryValueExW, RegSetValueExW,
    };

    const PROFILE_ASSOC: &str = r"Software\Microsoft\Windows NT\CurrentVersion\ICM\ProfileAssociations\Display\{4d36e96e-e325-11ce-bfc1-08002be10318}";

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateDCW(
            pwsz_driver: *const u16,
            pwsz_device: *const u16,
            pwsz_port: *const u16,
            p_data: *const std::ffi::c_void,
        ) -> HDC;
        fn DeleteDC(hdc: HDC) -> BOOL;
        fn SetDeviceGammaRamp(hdc: HDC, lp_ramp: *const std::ffi::c_void) -> BOOL;
        fn GetICMProfileW(hdc: HDC, p_buf_size: *mut u32, psz_filename: *mut u16) -> BOOL;
        fn SetICMProfileW(hdc: HDC, psz_filename: *const u16) -> BOOL;
    }

    #[link(name = "mscms")]
    extern "system" {
        fn InstallColorProfileW(p_machine_name: *const u16, p_profile_name: *const u16) -> i32;
    }

    #[repr(C)]
    struct DISPLAY_DEVICEW {
        cb: u32,
        device_name: [u16; 32],
        device_string: [u16; 128],
        state_flags: u32,
        device_id: [u16; 128],
        device_key: [u16; 128],
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplayDevicesW(
            lp_device: *const u16,
            i_dev_num: u32,
            lp_display_device: *mut DISPLAY_DEVICEW,
            dw_flags: u32,
        ) -> BOOL;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    fn empty_dev() -> DISPLAY_DEVICEW {
        DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            device_name: [0; 32],
            device_string: [0; 128],
            state_flags: 0,
            device_id: [0; 128],
            device_key: [0; 128],
        }
    }

    fn trailing_index(key: &[u16]) -> Option<String> {
        let trimmed = String::from_utf16_lossy(key);
        let trimmed = trimmed.trim_matches('\0');
        let sub = trimmed.split('\\').next_back()?;
        if sub.is_empty() || !sub.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(sub.to_string())
    }

    fn open(path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
        let w = to_wide(path);
        let mut hkey = HKEY::default();
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), 0, access, &mut hkey) };
        if rc == ERROR_SUCCESS { Some(hkey) } else { None }
    }

    fn create(path: &str) -> Option<HKEY> {
        let w = to_wide(path);
        let mut hkey = HKEY::default();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(w.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPEN_CREATE_OPTIONS(0),
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if rc == ERROR_SUCCESS { Some(hkey) } else { None }
    }

    fn close(hkey: HKEY) {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
    }

    fn write_string_value(path: &str, name: &str, ty: windows::Win32::System::Registry::REG_VALUE_TYPE, data: &[u8]) {
        let Some(hkey) = create(path) else { return };
        let name_w = to_wide(name);
        unsafe {
            let _ = RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), 0, ty, Some(data));
        }
        close(hkey);
    }

    fn multi_sz_bytes(strings: &[&str]) -> Vec<u8> {
        let mut w = Vec::new();
        for s in strings {
            w.extend(s.encode_utf16());
            w.push(0);
        }
        w.push(0);
        unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2).to_vec() }
    }

    pub fn apply_ramp(device_name: &str, ramp: &GammaRamp) -> bool {
        let wide = to_wide(device_name);
        unsafe {
            let hdc = CreateDCW(
                std::ptr::null(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            );
            if hdc.0.is_null() {
                return false;
            }
            let ok = SetDeviceGammaRamp(hdc, ramp as *const _ as *const _);
            let _ = DeleteDC(hdc);
            ok.as_bool()
        }
    }

    pub fn active_profile(device_name: &str) -> Option<String> {
        let sub = active_class_index(device_name)?;
        let path = format!(r"{PROFILE_ASSOC}\{sub}");
        let hkey = open(&path, KEY_READ)?;
        let mut buf = [0u16; 512];
        let mut size = 1024u32;
        let name_w = to_wide("ICMProfileSnapshot");
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                None,
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut size),
            )
        };
        close(hkey);
        if rc != ERROR_SUCCESS || size <= 2 {
            return None;
        }
        let name = String::from_utf16_lossy(&buf[..(size as usize / 2)])
            .trim_matches('\0')
            .to_string();
        if name.is_empty() { None } else { Some(name) }
    }

    pub fn set_profile(device_name: &str, file_name: &str) -> bool {
        if file_name.is_empty() {
            return false;
        }
        let wide_dev = to_wide(device_name);
        unsafe {
            let hdc = CreateDCW(
                std::ptr::null(),
                wide_dev.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            );
            if !hdc.0.is_null() {
                let wide_file = to_wide(file_name);
                let _ = SetICMProfileW(hdc, wide_file.as_ptr());
                let _ = DeleteDC(hdc);
            }
        }

        let spool = Path::new(r"C:\Windows\System32\spool\drivers\color").join(file_name);
        if spool.exists() {
            let w = to_wide(&spool.to_string_lossy());
            unsafe {
                let _ = InstallColorProfileW(std::ptr::null(), w.as_ptr());
            }
        }

        let Some(sub) = active_class_index(device_name) else {
            return false;
        };
        let path = format!(r"{PROFILE_ASSOC}\{sub}");
        // SINGLE SLOT: Windows Calibration Loader falls back to the OEM profile
        // unless exactly one entry exists, and `UsePerUserProfiles` pins it.
        let slot = multi_sz_bytes(&[file_name]);
        write_string_value(&path, "ICMProfile", REG_MULTI_SZ, &slot);
        write_string_value(&path, "ICMProfileSnapshot", REG_MULTI_SZ, &slot);
        write_string_value(&path, "UsePerUserProfiles", REG_DWORD, &1u32.to_ne_bytes());
        true
    }

    pub fn clear_profile(device_name: &str) {
        // An explicit empty list with UsePerUserProfiles=1 tells Windows the user
        // chose "no profile", which blocks OEM fallback on reboot.
        let empty = multi_sz_bytes(&[]);
        for sub in class_indices(device_name) {
            let path = format!(r"{PROFILE_ASSOC}\{sub}");
            write_string_value(&path, "ICMProfile", REG_MULTI_SZ, &empty);
            write_string_value(&path, "UsePerUserProfiles", REG_DWORD, &1u32.to_ne_bytes());
            if let Some(hkey) = open(&path, KEY_WRITE) {
                let name_w = to_wide("ICMProfileSnapshot");
                unsafe {
                    let _ = RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr()));
                }
                close(hkey);
            }
        }
    }

    /// Every monitor class index belonging to a device, attached or not.
    pub fn class_indices(device_name: &str) -> Vec<String> {
        let wide_name = to_wide(device_name);
        let mut subs = Vec::new();
        for mon_idx in 0..8 {
            let mut dd = empty_dev();
            let ok = unsafe { EnumDisplayDevicesW(wide_name.as_ptr(), mon_idx, &mut dd, 0) };
            if !ok.as_bool() {
                break;
            }
            if let Some(sub) = trailing_index(&dd.device_key) {
                if !subs.contains(&sub) {
                    subs.push(sub);
                }
            }
        }
        subs
    }

    fn active_class_index(device_name: &str) -> Option<String> {
        let wide_name = to_wide(device_name);
        let mut fallback = None;
        for mon_idx in 0..8 {
            let mut dd = empty_dev();
            let ok = unsafe { EnumDisplayDevicesW(wide_name.as_ptr(), mon_idx, &mut dd, 0) };
            if !ok.as_bool() {
                break;
            }
            let Some(sub) = trailing_index(&dd.device_key) else {
                continue;
            };
            if dd.state_flags & 0x1 != 0 {
                return Some(sub);
            }
            fallback.get_or_insert(sub);
        }
        fallback
    }

    pub fn active_devices() -> Vec<DisplayDevice> {
        let mut out = Vec::new();
        for index in 0..8u32 {
            let mut dd = empty_dev();
            let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut dd, 0) };
            if !ok.as_bool() {
                break;
            }
            let name = String::from_utf16_lossy(&dd.device_name)
                .trim_matches('\0')
                .to_string();
            if name.is_empty() {
                continue;
            }
            out.push(DisplayDevice {
                class_index: trailing_index(&dd.device_key),
                attached: dd.state_flags & 0x1 != 0,
                name,
            });
        }
        out
    }

    pub fn install_profile(path: &Path) -> bool {
        let spool = Path::new(r"C:\Windows\System32\spool\drivers\color");
        let Some(file) = path.file_name() else {
            return false;
        };
        let target = spool.join(file);
        let copied = if spool.exists() {
            std::fs::copy(path, &target).is_ok()
        } else {
            false
        };
        if copied {
            let w = to_wide(&target.to_string_lossy());
            unsafe {
                let _ = InstallColorProfileW(std::ptr::null(), w.as_ptr());
            }
        }
        copied
    }

    /// OEM factory profile shipped in the driver spool, as `(file name, label)`.
    pub fn factory_profile_hint() -> Option<(String, String)> {
        let spool = Path::new(r"C:\Windows\System32\spool\drivers\color");
        if !spool.exists() {
            return None;
        }
        let vendor_prefixes = ["BOE", "CSO", "AUO", "INN", "XMI"];
        if let Ok(entries) = std::fs::read_dir(spool) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let upper = fname.to_uppercase();
                let is_icc = upper.ends_with(".ICM") || upper.ends_with(".ICC");
                if is_icc && vendor_prefixes.iter().any(|p| upper.starts_with(p)) {
                    let label = fname.split('_').next().unwrap_or("factory").to_string();
                    return Some((fname, label));
                }
            }
        }
        None
    }

    pub fn hdr_active() -> bool {
        let base = b"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers\\MonitorDataStore\0";
        let mut hbase = HKEY::default();
        unsafe {
            if RegOpenKeyExA(
                HKEY_LOCAL_MACHINE,
                PCSTR(base.as_ptr()),
                0,
                KEY_READ,
                &mut hbase,
            )
            .is_err()
            {
                return false;
            }
            let mut idx = 0u32;
            let mut name_buf = [0u8; 128];
            let mut name_len = 128u32;
            loop {
                if idx >= 4
                    || RegEnumKeyExA(
                        hbase,
                        idx,
                        windows::core::PSTR(name_buf.as_mut_ptr()),
                        &mut name_len,
                        None,
                        windows::core::PSTR(std::ptr::null_mut()),
                        None,
                        None,
                    )
                    .is_err()
                {
                    break;
                }
                let name = std::str::from_utf8(&name_buf[..name_len as usize]).unwrap_or("");
                let sub = format!("SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers\\MonitorDataStore\\{}\0", name);
                let mut hsub = HKEY::default();
                if RegOpenKeyExA(
                    HKEY_LOCAL_MACHINE,
                    PCSTR(sub.as_ptr()),
                    0,
                    KEY_READ,
                    &mut hsub,
                )
                .is_ok()
                {
                    let mut ty = REG_DWORD;
                    let mut data = 0u32;
                    let mut len = 4u32;
                    let value = b"HDREnabled\0";
                    let rc = RegQueryValueExA(
                        hsub,
                        PCSTR(value.as_ptr()),
                        None,
                        Some(&mut ty),
                        Some(&mut data as *mut u32 as *mut u8),
                        Some(&mut len),
                    );
                    let _ = RegCloseKey(hsub);
                    if rc == ERROR_SUCCESS && data == 1 {
                        let _ = RegCloseKey(hbase);
                        return true;
                    }
                }
                idx += 1;
                name_len = 128;
            }
            let _ = RegCloseKey(hbase);
        }
        false
    }
}

// --------------------------------------------------------------------------
// Linux backend
// --------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod imp {
    use super::{DisplayDevice, GammaRamp};
    use std::path::{Path, PathBuf};

    const DRM_ROOT: &str = "/sys/class/drm";
    const ASSOC_FILE: &str = "associations.conf";

    fn drm_connector(device_name: &str) -> PathBuf {
        Path::new(DRM_ROOT).join(device_name)
    }

    /// DRM `gamma` takes 3×256 little-endian u16: R then G then B.
    pub fn apply_ramp(device_name: &str, ramp: &GammaRamp) -> bool {
        let node = drm_connector(device_name).join("gamma");
        let mut bytes = Vec::with_capacity(1536);
        for channel in [&ramp.red, &ramp.green, &ramp.blue] {
            for value in channel.iter() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(&node, bytes).is_ok()
    }

    fn assoc_path() -> PathBuf {
        super::super::xdg::data_dir().join(ASSOC_FILE)
    }

    fn read_assoc(device_name: &str) -> Option<String> {
        let text = std::fs::read_to_string(assoc_path()).ok()?;
        text.lines().find_map(|line| {
            let (dev, file) = line.split_once('=')?;
            if dev == device_name && !file.is_empty() {
                Some(file.to_string())
            } else {
                None
            }
        })
    }

    fn write_assoc(device_name: &str, file_name: Option<&str>) {
        let path = assoc_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut kept: Vec<String> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with(&format!("{device_name}=")))
            .map(str::to_string)
            .collect();
        if let Some(file) = file_name {
            kept.push(format!("{device_name}={file}"));
        }
        let _ = std::fs::write(&path, kept.join("\n") + "\n");
    }

    pub fn active_profile(device_name: &str) -> Option<String> {
        read_assoc(device_name)
    }

    /// There is no GDI here: the association is recorded, and colord will own
    /// the binding once the P3 backend lands.
    pub fn set_profile(device_name: &str, file_name: &str) -> bool {
        if file_name.is_empty() {
            return false;
        }
        write_assoc(device_name, Some(file_name));
        true
    }

    pub fn clear_profile(device_name: &str) {
        write_assoc(device_name, None);
    }

    pub fn class_indices(_device_name: &str) -> Vec<String> {
        Vec::new()
    }

    pub fn active_devices() -> Vec<DisplayDevice> {
        let Ok(entries) = std::fs::read_dir(DRM_ROOT) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // card0-DP-1 / card0-eDP-1: connectors, not the card node itself.
            if !name.starts_with("card") || !name.contains('-') {
                continue;
            }
            let status = std::fs::read_to_string(entry.path().join("status"))
                .unwrap_or_default()
                .trim()
                .to_string();
            out.push(DisplayDevice {
                class_index: None,
                attached: status == "connected",
                name,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// `$XDG_DATA_HOME/openrevo/profiles` is user-writable, so installing a
    /// profile is a copy; nothing needs root.
    pub fn install_profile(path: &Path) -> bool {
        let Some(file) = path.file_name() else {
            return false;
        };
        let dir = super::profiles_dir();
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(path, dir.join(file)).is_ok()
    }

    /// Vendor EDID prefixes shipped by system color directories.
    pub fn factory_profile_hint() -> Option<(String, String)> {
        let vendors = ["BOE", "CSO", "AUO", "INN", "XMI"];
        for dir in ["/usr/share/color/icc", "/usr/local/share/color/icc"] {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let upper = fname.to_uppercase();
                let is_icc = upper.ends_with(".ICM") || upper.ends_with(".ICC");
                if is_icc && vendors.iter().any(|p| upper.starts_with(p)) {
                    let label = fname.split('_').next().unwrap_or("factory").to_string();
                    return Some((fname, label));
                }
            }
        }
        None
    }

    /// DRM does not expose HDR state as an attribute; the `Colorspace` property
    /// needs libdrm. Report the colord/kscreen-independent truth we do have.
    pub fn hdr_active() -> bool {
        let Ok(entries) = std::fs::read_dir(DRM_ROOT) else {
            return false;
        };
        for entry in entries.flatten() {
            let colorspace = entry.path().join("colorspace");
            if let Ok(value) = std::fs::read_to_string(&colorspace) {
                if !value.trim().is_empty() && value.trim() != "Default" {
                    return true;
                }
            }
        }
        false
    }
}

pub use imp::*;
