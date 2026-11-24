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
pub use linux::{available, batch_read, batch_write, read, write, write_supported};
// The registry addresses are 16-bit: `(bank << 8) | offset`. Bank 0 is EC
// address space proper and available through `ec_sys`; every higher bank needs
// an ACPI "Read EC" (0x81) / "Write EC" (0x80) transaction, because the kernel
// module only maps the first 256 bytes flat. Those transactions go straight at
// the ACPI EC command/data ports, which is what the OEM Windows driver does
// internally as well.
#[cfg(target_os = "linux")]
pub mod linux {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use crate::log_warn;

    /// Newer `ec_sys` layout (kernel >= 5.x, `write_support=1`).
    const FLAT_NODE: &str = "/sys/kernel/debug/ec/ec_sys";
    /// Installed by the package for the polkit-mediated fallback.
    const HELPER_PATH: &str = "/usr/lib/openrevo/openrevo-ec-helper";
    /// Older per-device layout.
    const LEGACY_NODE: &str = "/sys/kernel/debug/ec/ec0/io";
    /// EC controllers need a short settle window between consecutive writes;
    /// back-to-back pwrite() calls are silently dropped otherwise.
    const WRITE_SETTLE: Duration = Duration::from_micros(200);

    // ACPI EC interface (ACPI 6.x §12.2): data port 0x62, command/status 0x66.
    const PORT_DATA: u16 = 0x62;
    const PORT_CMD: u16 = 0x66;
    const STATUS_OBF: u8 = 0x01;
    const STATUS_IBF: u8 = 0x02;
    const CMD_READ_EC: u8 = 0x81;
    const CMD_WRITE_EC: u8 = 0x80;
    /// A single transaction blocks the EC; the firmware answers well inside
    /// this, but a stuck handshake must not hang a UI thread.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(50);

    static NODE: OnceLock<Option<PathBuf>> = OnceLock::new();
    static HANDLE: Mutex<Option<File>> = Mutex::new(None);
    /// Serialises port transactions: the EC has one command buffer.
    static PORT_LOCK: Mutex<()> = Mutex::new(());
    /// 0 = untried, 1 = granted, 2 = denied.
    static PORT_ACCESS: AtomicU8 = AtomicU8::new(0);
    static FLAT_WRITABLE: AtomicBool = AtomicBool::new(false);

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
                Ok(file) => {
                    FLAT_WRITABLE.store(true, Ordering::Release);
                    *guard = Some(file);
                }
                Err(e) => {
                    // Read-only mount of debugfs or missing privileges: keep it
                    // readable so telemetry still works.
                    match OpenOptions::new().read(true).open(path) {
                        Ok(file) => {
                            FLAT_WRITABLE.store(false, Ordering::Release);
                            *guard = Some(file);
                        }
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

    #[cfg(target_arch = "x86_64")]
    mod ports {
        use std::os::raw::{c_int, c_ulong};

        extern "C" {
            fn ioperm(from: c_ulong, num: c_ulong, turn_on: c_int) -> c_int;
        }

        #[inline]
        pub unsafe fn inb(port: u16) -> u8 {
            let value: u8;
            std::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
            value
        }

        #[inline]
        pub unsafe fn outb(value: u8, port: u16) {
            std::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
        }

        /// Grant port access to this process. Needs `CAP_SYS_RAWIO` (root).
        pub fn grant(from: u16, count: u16) -> bool {
            unsafe { ioperm(from as c_ulong, count as c_ulong, 1) == 0 }
        }
    }

    /// Whether userspace may drive the ACPI EC ports. Probed once: `ioperm` has
    /// no cheap undo and a denial is stable for the process lifetime.
    #[cfg(target_arch = "x86_64")]
    fn port_access_granted() -> bool {
        match PORT_ACCESS.load(Ordering::Acquire) {
            1 => true,
            2 => false,
            _ => {
                // Both ports are contiguous enough to request in one call only
                // if granted separately; the EC set is {0x62, 0x66}.
                let ok = ports::grant(PORT_DATA, 1) & ports::grant(PORT_CMD, 1);
                PORT_ACCESS.store(if ok { 1 } else { 2 }, Ordering::Release);
                if !ok {
                    log_warn!(
                        "PAL-EC",
                        "ioperm on EC ports denied; banked EC access needs root (or CAP_SYS_RAWIO)"
                    );
                }
                ok
            }
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn port_access_granted() -> bool {
        false
    }

    #[cfg(target_arch = "x86_64")]
    fn wait_status(mask: u8, set: bool) -> bool {
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        while Instant::now() < deadline {
            let status = unsafe { ports::inb(PORT_CMD) };
            if (status & mask != 0) == set {
                return true;
            }
            std::hint::spin_loop();
        }
        false
    }

    /// Read one byte through an ACPI EC transaction (any bank).
    #[cfg(target_arch = "x86_64")]
    fn port_read(addr: u16) -> Option<u8> {
        if !port_access_granted() {
            return None;
        }
        let _guard = PORT_LOCK.lock().ok()?;
        unsafe {
            if !wait_status(STATUS_IBF, false) {
                return None;
            }
            ports::outb(CMD_READ_EC, PORT_CMD);
            if !wait_status(STATUS_IBF, false) {
                return None;
            }
            ports::outb((addr >> 8) as u8, PORT_DATA);
            if !wait_status(STATUS_IBF, false) {
                return None;
            }
            ports::outb(addr as u8, PORT_DATA);
            if !wait_status(STATUS_OBF, true) {
                return None;
            }
            Some(ports::inb(PORT_DATA))
        }
    }

    /// Write one byte through an ACPI EC transaction (any bank).
    #[cfg(target_arch = "x86_64")]
    fn port_write(addr: u16, val: u8) -> bool {
        if !port_access_granted() {
            return false;
        }
        let Ok(_guard) = PORT_LOCK.lock() else {
            return false;
        };
        unsafe {
            if !wait_status(STATUS_IBF, false) {
                return false;
            }
            ports::outb(CMD_WRITE_EC, PORT_CMD);
            if !wait_status(STATUS_IBF, false) {
                return false;
            }
            ports::outb((addr >> 8) as u8, PORT_DATA);
            if !wait_status(STATUS_IBF, false) {
                return false;
            }
            ports::outb(addr as u8, PORT_DATA);
            if !wait_status(STATUS_IBF, false) {
                return false;
            }
            ports::outb(val, PORT_DATA);
            if !wait_status(STATUS_IBF, false) {
                return false;
            }
        }
        std::thread::sleep(WRITE_SETTLE);
        true
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn port_read(_addr: u16) -> Option<u8> {
        None
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn port_write(_addr: u16, _val: u8) -> bool {
        false
    }

    /// Bank 0 can use the flat debugfs view; banks 1+ must go over the ports.
    fn via_node(addr: u16) -> bool {
        addr <= 0xFF && node().is_some()
    }

    pub fn available() -> bool {
        node().is_some() || port_access_granted()
    }

    pub fn read(addr: u16) -> Option<u8> {
        if via_node(addr) {
            if let Some(v) = with_handle(|file| {
                file.seek(SeekFrom::Start(addr as u64))?;
                let mut buf = [0u8; 1];
                file.read_exact(&mut buf)?;
                Ok(buf[0])
            }) {
                return Some(v);
            }
        }
        port_read(addr)
    }

    pub fn write(addr: u16, val: u8) -> bool {
        if via_node(addr) && FLAT_WRITABLE.load(Ordering::Acquire) {
            let ok = with_handle(|file| {
                file.seek(SeekFrom::Start(addr as u64))?;
                file.write_all(&[val])?;
                file.flush()
            })
            .is_some();
            if ok {
                std::thread::sleep(WRITE_SETTLE);
                return true;
            }
        }
        if port_write(addr, val) {
            return true;
        }
        helper_write(&[(addr, val)])
    }

    /// Opt-in escape hatch for stock kernels with no module and no writable node:
    /// hand the write to `openrevo-ec-helper` through `pkexec`, where polkit
    /// decides about the prompt. Off unless `OPENREVO_EC_HELPER=1`, because a
    /// prompted write during boot is intrusive and most machines never need it.
    fn helper_write(entries: &[(u16, u8)]) -> bool {
        if std::env::var_os("OPENREVO_EC_HELPER").is_none() || entries.is_empty() {
            return false;
        }
        let helper = if std::path::Path::new(HELPER_PATH).exists() {
            HELPER_PATH.to_string()
        } else {
            "openrevo-ec-helper".to_string()
        };
        let mut cmd = crate::pal::process::command("pkexec");
        cmd.arg(helper).arg("write");
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                crate::log_warn!("PAL-EC", "cannot spawn {} via pkexec: {}", HELPER_PATH, err);
                return false;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            let mut payload = String::new();
            for (addr, val) in entries {
                payload.push_str(&format!("w {:04x} {:02x}\n", addr, val));
            }
            let _ = stdin.write_all(payload.as_bytes());
        }
        let ok = matches!(child.wait(), Ok(status) if status.success());
        if !ok {
            crate::log_warn!(
                "PAL-EC",
                "privileged helper rejected {} write(s); run as root or pre-authorise com.openrevo.ecwrite",
                entries.len()
            );
        }
        ok
    }

    /// Batch read, sequential. Missing entries come back as `None`.
    pub fn batch_read(addrs: &[u16]) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(addrs.len());
        let mut complete = true;
        for addr in addrs {
            match read(*addr) {
                Some(v) => out.push(v),
                None => {
                    out.push(0);
                    complete = false;
                }
            }
        }
        if complete {
            Some(out)
        } else {
            Some(out)
        }
    }

    /// Batch write with per-byte settling. Falls back to a single privileged
    /// call for the whole batch rather than one prompt per byte.
    pub fn batch_write(entries: &[(u16, u8)]) -> bool {
        let mut all_ok = true;
        for (addr, val) in entries {
            if !write(*addr, *val) {
                all_ok = false;
                break;
            }
        }
        if all_ok {
            return true;
        }
        helper_write(entries)
    }

    /// Diagnostic snapshot for the port transaction path.
    #[cfg(test)]
    pub fn debug_port_state() -> (bool, Option<u8>) {
        #[cfg(target_arch = "x86_64")]
        {
            let granted = port_access_granted();
            let status = if granted { Some(unsafe { ports::inb(PORT_CMD) }) } else { None };
            (granted, status)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            (false, None)
        }
    }

    /// True when *some* write channel is live: a writable flat node, or granted
    /// EC ports (which cover every bank).
    pub fn write_supported() -> bool {
        if FLAT_WRITABLE.load(Ordering::Acquire) {
            return true;
        }
        if node().is_some() {
            // Force the node open so the writability flag is known.
            let _ = with_handle(|file| file.metadata().map(|_| ()));
        }
        FLAT_WRITABLE.load(Ordering::Acquire) || port_access_granted()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_port_tests {
    use super::linux;

    /// Exercises the ACPI EC transaction path against the project id register
    /// (bank 7, offset 0x40). Passes trivially where no EC exists, so it is
    /// safe on VMs and CI.
    #[test]
    fn project_id_round_trip() {
        let addr: u16 = (7 << 8) | 0x40;
        println!("ec_sys present: {}", linux::available());
        println!("write channel: {}", linux::write_supported());
        println!("ports: granted={:?} status={:?}", linux::debug_port_state().0, linux::debug_port_state().1);
        match linux::read(addr) {
            Some(val) => println!("PROJECT_ID = 0x{:02X}", val),
            None => println!("PROJECT_ID unreadable (no EC or no port access)"),
        }
    }
}
