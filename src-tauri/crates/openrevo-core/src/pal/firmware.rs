//! Firmware / platform services.
//!
//! Everything here is "the OS knows about the machine, but only through a
//! platform-specific channel": UEFI variables, PCI rescan, GPU service control,
//! CPU identification, reboot.
//!
//! - **Windows**: `Get/SetFirmwareEnvironmentVariableW` with
//!   `SeSystemEnvironmentPrivilege`, `pnputil /scan-devices`, `net stop|start`,
//!   the `CentralProcessor` registry key.
//! - **Linux**: efivarfs (`/sys/firmware/efi/efivars`), `/sys/bus/pci/rescan`,
//!   `systemctl stop|start nvidia-persistenced` plus module unload,
//!   `/proc/cpuinfo`.
//!
//! efivarfs entries carry a 4-byte little-endian attribute prefix that must be
//! preserved on write, and firmware-set variables are `chattr +i` immutable, so
//! a write first strips the flag. Writes need root; the error text says so
//! instead of failing silently.

/// GUID owning the UniWill / Tongfang motherboard NVRAM variables.
pub const UNIWILL_GUID: &str = "9F33F85C-13CA-4FD1-9C4A-96217722C593";

pub const REBOOT_PENDING_FILE: &str = "openrevo/reboot.pending";

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    const NUL: &str = "\0";

    fn wide(s: &str) -> Vec<u16> {
        format!("{s}{NUL}").encode_utf16().collect()
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFirmwareEnvironmentVariableW(
            name: *const u16,
            guid: *const u16,
            buffer: *mut c_void,
            size: u32,
        ) -> u32;
        fn SetFirmwareEnvironmentVariableW(
            name: *const u16,
            guid: *const u16,
            buffer: *const c_void,
            size: u32,
        ) -> u32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            process: *mut c_void,
            desired_access: u32,
            token: *mut *mut c_void,
        ) -> i32;
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut u64) -> i32;
        fn AdjustTokenPrivileges(
            token: *mut c_void,
            disable_all: i32,
            new_state: *const c_void,
            buffer_length: u32,
            previous_state: *mut c_void,
            return_length: *mut u32,
        ) -> i32;
        fn CloseHandle(object: *mut c_void) -> i32;
    }

    #[repr(C)]
    struct TokenPrivileges {
        count: u32,
        luid: u64,
        attributes: u32,
    }

    /// Enables `SeSystemEnvironmentPrivilege` on the process token.
    pub fn unlock_privilege() -> Result<(), String> {
        unsafe {
            let mut token: *mut c_void = std::ptr::null_mut();
            if OpenProcessToken(
                -1isize as *mut c_void,
                0x0028, // TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY
                &mut token,
            ) == 0
            {
                return Err("OpenProcessToken failed".into());
            }
            let name = wide("SeSystemEnvironmentPrivilege");
            let mut luid = 0u64;
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
                let _ = CloseHandle(token);
                return Err("LookupPrivilegeValueW failed".into());
            }
            let tp = TokenPrivileges {
                count: 1,
                luid,
                attributes: 0x0000_0002, // SE_PRIVILEGE_ENABLED
            };
            let ok = AdjustTokenPrivileges(
                token,
                0,
                &tp as *const _ as *const c_void,
                std::mem::size_of::<TokenPrivileges>() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let _ = CloseHandle(token);
            if ok == 0 {
                return Err("AdjustTokenPrivileges failed".into());
            }
            Ok(())
        }
    }

    pub fn read_var(guid: &str, name: &str, max_len: usize) -> Result<Vec<u8>, String> {
        unlock_privilege()?;
        let guid_w = wide(guid);
        let name_w = wide(name);
        let mut buf = vec![0u8; max_len];
        let read = unsafe {
            GetFirmwareEnvironmentVariableW(
                name_w.as_ptr(),
                guid_w.as_ptr(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
            )
        };
        if read == 0 {
            let err = unsafe { crate::driver::hid_ffi::GetLastError() };
            return Err(format!("firmware variable read failed (Win32 {err})"));
        }
        buf.truncate(read as usize);
        Ok(buf)
    }

    pub fn write_var(guid: &str, name: &str, data: &[u8]) -> Result<(), String> {
        unlock_privilege()?;
        let guid_w = wide(guid);
        let name_w = wide(name);
        let ok = unsafe {
            SetFirmwareEnvironmentVariableW(
                name_w.as_ptr(),
                guid_w.as_ptr(),
                data.as_ptr() as *const c_void,
                data.len() as u32,
            )
        };
        if ok == 0 {
            let err = unsafe { crate::driver::hid_ffi::GetLastError() };
            return Err(format!("firmware variable write failed (Win32 {err})"));
        }
        Ok(())
    }

    pub fn cpu_brand() -> Option<String> {
        crate::pal::reg::read_string(
            crate::pal::reg::RegRoot::Machine,
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        )
    }

    pub fn rescan_pci() -> bool {
        std::process::Command::new("pnputil.exe")
            .arg("/scan-devices")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn stop_gpu_services() {
        for svc in ["NVDisplay.ContainerLocalSystem", "NVContainerLocalSystem"] {
            let _ = std::process::Command::new("net.exe")
                .args(["stop", svc])
                .status();
        }
    }

    pub fn start_gpu_services() {
        for svc in ["NVDisplay.ContainerLocalSystem", "NVContainerLocalSystem"] {
            let _ = std::process::Command::new("net.exe")
                .args(["start", svc])
                .status();
        }
    }

    pub fn reboot_pending() -> bool {
        let file = std::env::temp_dir().join(super::REBOOT_PENDING_FILE);
        file.exists()
    }

    pub fn system_reboot() -> Result<(), String> {
        std::process::Command::new("shutdown.exe")
            .args(["/r", "/t", "3"])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod imp {
    use std::path::{Path, PathBuf};

    const EFIVARS: &str = "/sys/firmware/efi/efivars";

    fn var_path(guid: &str, name: &str) -> PathBuf {
        // efivarfs naming is `<Name>-<lowercase-guid>`.
        Path::new(EFIVARS).join(format!("{}-{}", name, guid.to_lowercase()))
    }

    pub fn unlock_privilege() -> Result<(), String> {
        let root = Path::new(EFIVARS);
        if !root.exists() {
            return Err("efivarfs not mounted (booted in BIOS/CSM mode?)".into());
        }
        Ok(())
    }

    pub fn read_var(guid: &str, name: &str, max_len: usize) -> Result<Vec<u8>, String> {
        unlock_privilege()?;
        let path = var_path(guid, name);
        let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if raw.len() < 4 {
            return Err("efivarfs entry shorter than its attribute header".into());
        }
        let mut data = raw[4..].to_vec();
        data.truncate(max_len);
        Ok(data)
    }

    /// Firmware normally creates these variables with the immutable flag set; a
    /// write has to clear it first or the kernel answers EPERM.
    pub fn write_var(guid: &str, name: &str, data: &[u8]) -> Result<(), String> {
        unlock_privilege()?;
        let path = var_path(guid, name);
        let existing = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if existing.len() < 4 {
            return Err("efivarfs entry shorter than its attribute header".into());
        }
        let mut payload = existing[..4].to_vec();
        payload.extend_from_slice(data);
        // `chattr -i` is the only supported way; ignore failure when the helper
        // is absent and let the write itself report the permission error.
        let _ = std::process::Command::new("chattr")
            .arg("-i")
            .arg(&path)
            .status();
        std::fs::write(&path, payload).map_err(|e| {
            format!(
                "{}: {e} (efivarfs writes need root)",
                path.display()
            )
        })
    }

    pub fn cpu_brand() -> Option<String> {
        let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        let model = text
            .lines()
            .find_map(|l| l.strip_prefix("model name\t:"))
            .map(str::trim)?
            .to_string();
        if model.is_empty() { None } else { Some(model) }
    }

    /// `/sys/bus/pci/rescan` is root-only but the kernel does the work in one
    /// write; there is no userspace equivalent.
    pub fn rescan_pci() -> bool {
        std::fs::write("/sys/bus/pci/rescan", b"1").is_ok()
    }

    /// nvidia-persistenced keeps the eDP pipeline open the same way the Windows
    /// container service does; unloading the modules finishes the job.
    pub fn stop_gpu_services() {
        let _ = std::process::Command::new("systemctl")
            .args(["stop", "nvidia-persistenced.service"])
            .status();
        for module in ["nvidia_drm", "nvidia_modeset", "nvidia"] {
            let _ = std::process::Command::new("modprobe").args(["-r", module]).status();
        }
    }

    pub fn start_gpu_services() {
        for module in ["nvidia", "nvidia_modeset", "nvidia_drm"] {
            let _ = std::process::Command::new("modprobe").arg(module).status();
        }
        let _ = std::process::Command::new("systemctl")
            .args(["start", "nvidia-persistenced.service"])
            .status();
    }

    pub fn reboot_pending() -> bool {
        super::reboot_pending_file().exists()
    }

    pub fn system_reboot() -> Result<(), String> {
        std::process::Command::new("systemctl")
            .arg("reboot")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Path of the reboot-pending marker, honouring `XDG_RUNTIME_DIR`.
pub fn reboot_pending_file() -> std::path::PathBuf {
    crate::pal::xdg::runtime_dir()
        .unwrap_or_else(crate::pal::xdg::cache_home)
        .join(REBOOT_PENDING_FILE)
}

pub use imp::*;
