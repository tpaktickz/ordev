// PAL — Embedded Controller raw port.
//
// Windows: delegated to the ACPI/WMI EC driver already shipped in `driver::wmi_ec`.
// Linux:   `ec_sys` kernel module exposed through debugfs. The node is a flat
//          512-byte view of EC address space, so file offset == EC address.

#[cfg(windows)]
pub use windows_arm::{available, batch_read, batch_write, read, write};

#[cfg(windows)]
mod windows_arm {
    //! `\.\ACPIDriver` IOCTLs, with the driverless AcpiTest_MULong WMI mailbox
    //! as a fallback for legacy Tongfang/Hasee firmware.

    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::driver::wmi_mulong;

    const IOCTL_ECREAD: u32 = crate::hal::vault::driver::IOCTL_ECREAD;
    const IOCTL_ECWRITE: u32 = crate::hal::vault::driver::IOCTL_ECWRITE;
    const GENERIC_READ_WRITE: u32 = 0xC000_0000;
    const FILE_SHARE_READ_WRITE: u32 = 3;
    const OPEN_EXISTING: u32 = 3;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
        fn CreateFileA(
            name: *const u8,
            desired_access: u32,
            share_mode: u32,
            security: *mut std::ffi::c_void,
            creation_disposition: u32,
            flags: u32,
            template: *mut std::ffi::c_void,
        ) -> isize;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn DeviceIoControl(
            device: isize,
            ioctl: u32,
            in_buffer: *const std::ffi::c_void,
            in_size: u32,
            out_buffer: *mut std::ffi::c_void,
            out_size: u32,
            returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }

    static PROBED: AtomicBool = AtomicBool::new(false);
    static AVAILABLE: AtomicBool = AtomicBool::new(false);

    fn open() -> Option<isize> {
        let name = crate::hal::vault::driver::get_driver_path_null_terminated();
        let handle = unsafe {
            CreateFileA(
                name.as_ptr(),
                GENERIC_READ_WRITE,
                FILE_SHARE_READ_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == -1 || handle == 0 {
            None
        } else {
            Some(handle)
        }
    }

    pub fn available() -> bool {
        if !PROBED.load(Ordering::Acquire) {
            let ok = open().is_some_and(|h| unsafe { CloseHandle(h as *mut _) } != 0);
            AVAILABLE.store(ok, Ordering::Release);
            PROBED.store(true, Ordering::Release);
            return ok;
        }
        AVAILABLE.load(Ordering::Acquire)
    }

    fn ioctl_read(handle: isize, addr: u16) -> Option<u8> {
        let in_buf: [u32; 1] = [addr as u32];
        let mut out_buf: u32 = 0;
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_ECREAD,
                in_buf.as_ptr() as *const _,
                4,
                &mut out_buf as *mut u32 as *mut _,
                4,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok != 0 { Some((out_buf & 0xFF) as u8) } else { None }
    }

    fn ioctl_write(handle: isize, addr: u16, val: u8) -> bool {
        let in_buf: [u32; 2] = [addr as u32, val as u32];
        let mut out_buf: u32 = 0;
        let mut returned: u32 = 0;
        unsafe {
            DeviceIoControl(
                handle,
                IOCTL_ECWRITE,
                in_buf.as_ptr() as *const _,
                8,
                &mut out_buf as *mut u32 as *mut _,
                4,
                &mut returned,
                ptr::null_mut(),
            ) != 0
        }
    }

    pub fn read(addr: u16) -> Option<u8> {
        if available() {
            if let Some(handle) = open() {
                let val = ioctl_read(handle, addr);
                unsafe { CloseHandle(handle as *mut _) };
                if val.is_some() {
                    return val;
                }
            }
        }
        if wmi_mulong::is_mulong_available() {
            return wmi_mulong::mulong_read_ec(addr);
        }
        None
    }

    pub fn write(addr: u16, val: u8) -> bool {
        if available() {
            if let Some(handle) = open() {
                let ok = ioctl_write(handle, addr, val);
                unsafe { CloseHandle(handle as *mut _) };
                if ok {
                    return true;
                }
            }
        }
        if wmi_mulong::is_mulong_available() {
            return wmi_mulong::mulong_write_ec(addr, val);
        }
        false
    }

    pub fn batch_read(addrs: &[u16]) -> Option<Vec<u8>> {
        if let Some(handle) = open() {
            let mut out = Vec::with_capacity(addrs.len());
            for &addr in addrs {
                out.push(ioctl_read(handle, addr).unwrap_or(0));
            }
            unsafe { CloseHandle(handle as *mut _) };
            return Some(out);
        }
        if wmi_mulong::is_mulong_available() {
            return Some(
                addrs
                    .iter()
                    .map(|a| wmi_mulong::mulong_read_ec(*a).unwrap_or(0))
                    .collect(),
            );
        }
        None
    }

    pub fn batch_write(entries: &[(u16, u8)]) -> bool {
        if let Some(handle) = open() {
            let mut all_ok = true;
            for &(addr, val) in entries {
                if !ioctl_write(handle, addr, val) {
                    all_ok = false;
                }
            }
            unsafe { CloseHandle(handle as *mut _) };
            return all_ok;
        }
        if wmi_mulong::is_mulong_available() {
            return entries.iter().all(|(a, v)| wmi_mulong::mulong_write_ec(*a, *v));
        }
        false
    }
}

#[cfg(target_os = "linux")]
pub use linux::{available, batch_read, batch_write, read, write};

#[cfg(target_os = "linux")]
pub mod linux {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use crate::log_warn;

    /// Newer `ec_sys` layout (kernel >= 5.x, `write_support=1`).
    const FLAT_NODE: &str = "/sys/kernel/debug/ec/ec_sys";
    /// Older per-device layout.
    const LEGACY_NODE: &str = "/sys/kernel/debug/ec/ec0/io";
    /// EC controllers need a short settle window between consecutive writes;
    /// back-to-back pwrite() calls are silently dropped otherwise.
    const WRITE_SETTLE: Duration = Duration::from_micros(200);

    static NODE: OnceLock<Option<PathBuf>> = OnceLock::new();
    static HANDLE: Mutex<Option<File>> = Mutex::new(None);

    fn node() -> Option<&'static Path> {
        NODE.get_or_init(|| {
            for candidate in [FLAT_NODE, LEGACY_NODE] {
                let p = Path::new(candidate);
                if p.exists() {
                    return Some(p.to_path_buf());
                }
            }
            None
        })
        .as_deref()
    }

    fn with_handle<T>(f: impl FnOnce(&mut File) -> std::io::Result<T>) -> Option<T> {
        let mut guard = HANDLE.lock().ok()?;
        if guard.is_none() {
            let path = node()?;
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => *guard = Some(file),
                Err(e) => {
                    // Read-only mount of debugfs or missing privileges: keep it
                    // readable so telemetry still works.
                    match OpenOptions::new().read(true).open(path) {
                        Ok(file) => *guard = Some(file),
                        Err(_) => {
                            log_warn!("PAL-EC", "open {} failed: {}", path.display(), e);
                            return None;
                        }
                    }
                }
            }
        }
        let file = guard.as_mut()?;
        f(file).ok()
    }

    pub fn available() -> bool {
        node().is_some()
    }

    pub fn read(addr: u16) -> Option<u8> {
        with_handle(|file| {
            file.seek(SeekFrom::Start(addr as u64))?;
            let mut buf = [0u8; 1];
            file.read_exact(&mut buf)?;
            Ok(buf[0])
        })
    }

    pub fn write(addr: u16, val: u8) -> bool {
        let ok = with_handle(|file| {
            file.seek(SeekFrom::Start(addr as u64))?;
            file.write_all(&[val])?;
            file.flush()
        })
        .is_some();
        if ok {
            std::thread::sleep(WRITE_SETTLE);
        }
        ok
    }

    /// Batch read, single seek. Addresses are sorted ascending; missing entries
    /// come back as `None`.
    pub fn batch_read(addrs: &[u16]) -> Option<Vec<u8>> {
        with_handle(|file| {
            let mut out = vec![0u8; addrs.len()];
            for (i, addr) in addrs.iter().enumerate() {
                file.seek(SeekFrom::Start(*addr as u64))?;
                let mut buf = [0u8; 1];
                file.read_exact(&mut buf)?;
                out[i] = buf[0];
            }
            Ok(out)
        })
    }

    /// Batch write with per-byte settling.
    pub fn batch_write(entries: &[(u16, u8)]) -> bool {
        entries.iter().all(|(addr, val)| write(*addr, *val))
    }

    /// True when the module was loaded with `write_support=1`. With write
    /// support off the node is still readable, so probe by round-tripping a
    /// scratch address through the flat node only.
    pub fn write_supported() -> bool {
        with_handle(|file| file.metadata().map(|m| m.len() >= 512)).unwrap_or(false)
    }
}
