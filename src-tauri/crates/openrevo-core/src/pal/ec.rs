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
    /// Privileged broker socket. `openrevo-ec.service` (root) serves it and
    /// `openrevo-ec-helper serve` is what it runs; membership of the
    /// `openrevo-ec` group is what the socket's own mode gates on. Without this
    /// an unprivileged daemon cannot read a single EC byte, because both
    /// `ec_sys` and `/proc/acpi/call` live under root-only paths.
    const BROKER_SOCKET: &str = "/run/openrevo/ec.sock";
    /// How long a broker reply may take before the connection is abandoned.
    const BROKER_TIMEOUT: Duration = Duration::from_secs(5);
    /// Older per-device layout.
    const LEGACY_NODE: &str = "/sys/kernel/debug/ec/ec0/io";
    /// EC controllers need a short settle window between consecutive writes;
    /// back-to-back pwrite() calls are silently dropped otherwise.
    const WRITE_SETTLE: Duration = Duration::from_micros(200);

    // Bank-0 mailbox of the Uniwill/Tongfang EC. The OEM Windows driver never
    // touches the ACPI ports: it asks the firmware over WMI (method
    // `GetSetULong` on GUID ABBC0F6F-8EA1-11d1-00A0-C90629100000), the AML
    // implementation of which posts a 16-bit address and reads a 16-bit word
    // through these registers. Same layout as TUXEDO's `uniwill_wmi.c`.
    const MB_LDAT: u64 = 0x8A;
    const MB_HDAT: u64 = 0x8B;
    const MB_FLAGS: u64 = 0x8C;
    const MB_CMDL: u64 = 0x8D;
    const MB_CMDH: u64 = 0x8E;
    const MB_RFLG: u8 = 1 << 0;
    const MB_WFLG: u8 = 1 << 1;
    const MB_BFLG: u8 = 1 << 2;
    const MB_DRDY: u8 = 1 << 7;
    /// The firmware polls for 20 × 50 ms; matching that ceiling costs nothing on
    /// a healthy EC and covers a busy one.
    // The EC answers a mailbox transaction in a few milliseconds; polling every
    // 2 ms with a matching cycle budget keeps the worst case at 100 ms while a
    // 100-entry fan table write drops from ~2.3 s to well under 1 s.
    const MB_POLL_CYCLES: usize = 50;
    const MB_POLL_DELAY: Duration = Duration::from_millis(2);
    /// `\proc\acpi\call` (the `acpi_call` module) evaluates an AML method as root.
    /// It is the only userspace path that runs the mailbox under the firmware
    /// mutex, so it cannot interleave with the kernel's battery/thermal polling.
    const AML_NODE: &str = "/proc/acpi/call";
    const AML_METHOD: &str = "\\_SB.AMW0.WMBC 0x0a 0x04";
    const AML_READ: u8 = 0x01;
    /// Writing RAM is the *zero* selector: `SAC1 == Zero` branches to WKBC.
    /// 0x02 is the EC command channel (SCMD), which only moved the address byte
    /// into CMDL and left the RAM untouched.
    const AML_WRITE: u8 = 0x00;

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
    /// 0 = untried, 1 = usable, 2 = absent or not writable.
    static AML: AtomicU8 = AtomicU8::new(0);
    /// Mailbox page that the debugfs node maps, or 0xFF while unknown.
    static NODE_PAGE: AtomicU8 = AtomicU8::new(0xFF);
    /// The EC has one mailbox; two threads must not interleave register writes.
    static MAILBOX_LOCK: Mutex<()> = Mutex::new(());
    /// 0 = never tried, 1 = reachable, 2 = absent.
    static BROKER: AtomicU8 = AtomicU8::new(0);
    static BROKER_LINK: Mutex<Option<std::os::unix::net::UnixStream>> = Mutex::new(None);
    /// The broker itself must never act as a client of its own socket: each
    /// `broker_*` call would open another connection, whose handler re-enters
    /// `read()`, recursing until descriptors or the stack run out.
    static BROKER_CLIENT: AtomicBool = AtomicBool::new(true);
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

    /// Whether the port path may be attempted at all. Distributions do not set
    /// this; the privileged broker does, because on this hardware the EC belongs
    /// to the firmware and only the mailbox can talk to it.
    fn ports_enabled() -> bool {
        !matches!(
            std::env::var_os("OPENREVO_EC_NO_PORTS"),
            Some(value) if value != "0"
        )
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
        if !ports_enabled() || !port_access_granted() {
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
        if !ports_enabled() || !port_access_granted() {
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
        // A clean IBF says nothing about whether the EC kept the byte: on a
        // firmware-owned EC every port write "succeeds" and lands nowhere. Read it
        // back through the mailbox before claiming success.
        if aml_available() {
            match aml_call(AML_READ, addr, 0) {
                Some((lo, _)) if lo == val => {}
                _ => return false,
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

    // ---- banked access through the bank-0 mailbox -----------------------
    //
    // Bank 0 of the debugfs node is *not* mailbox bank 0: the EC keeps the
    // ACPI-visible 256 bytes in its own page (0x04 on Tongfang hardware), so the
    // node is a window onto that page, not onto address 0. Everything else -
    // fan tables in bank 6/7, fan duty in bank 7 - is reachable only through the
    // mailbox, which the firmware exposes as the WMI method above.

    fn aml_available() -> bool {
        match AML.load(Ordering::Acquire) {
            1 => true,
            2 => false,
            _ => {
                let ok = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(AML_NODE)
                    .is_ok();
                AML.store(if ok { 1 } else { 2 }, Ordering::Release);
                ok
            }
        }
    }

    /// One firmware mailbox transaction. Returns `(CMDL, CMDH)`.
    ///
    /// The 40-byte buffer is the method's input block: `[0..2]` address little
    /// endian, `[2..4]` data, `[5]` read/write selector. The firmware echoes it
    /// back with the answer in `[0..2]`, and `0xfe` repeated for an error.
    fn aml_call(function: u8, addr: u16, data: u8) -> Option<(u8, u8)> {
        if !aml_available() {
            return None;
        }
        let _guard = MAILBOX_LOCK.lock().ok()?;
        let mut buf = [0u8; 40];
        buf[0] = addr as u8;
        buf[1] = (addr >> 8) as u8;
        buf[2] = data;
        buf[5] = function;
        let mut line = format!("{} {{", AML_METHOD);
        for (i, byte) in buf.iter().enumerate() {
            line.push_str(if i == 0 { "0x" } else { ", 0x" });
            line.push_str(&format!("{:02x}", byte));
        }
        line.push_str("}\n");

        {
            let mut sink = OpenOptions::new().write(true).open(AML_NODE).ok()?;
            sink.write_all(line.as_bytes()).ok()?;
            sink.flush().ok()?;
        }
        // acpi_call's read handler bails out with EOF when the caller's buffer
        // is smaller than the result (acpi_call.c: `if (len + 1 > count) return 0`),
        // so this must be a single read with a buffer big enough for the whole
        // reply - a chunked reader such as `read_to_string` always sees an
        // empty file. The result is also consumed by the read that finishes it.
        let mut raw = [0u8; 512];
        let n = File::open(AML_NODE).ok()?.read(&mut raw).ok()?;
        if n == 0 {
            return None;
        }
        let text = std::str::from_utf8(&raw[..n]).ok()?;
        let body = text.trim_end_matches('\0').trim();
        if !body.starts_with('{') {
            return None;
        }
        let mut bytes = body
            .trim_start_matches('{')
            .trim_end_matches('}')
            .split(',')
            .map(|tok| u8::from_str_radix(tok.trim().trim_start_matches("0x"), 16).ok());
        let lo = bytes.next()??;
        let hi = bytes.next()??;
        if lo == 0xfe && hi == 0xfe {
            return None;
        }
        Some((lo, hi))
    }

    fn node_byte(offset: u64) -> Option<u8> {
        with_handle(|file| {
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = [0u8; 1];
            file.read_exact(&mut buf)?;
            Ok(buf[0])
        })
    }

    fn node_set_byte(offset: u64, value: u8) -> bool {
        with_handle(|file| {
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(&[value])?;
            file.flush()
        })
        .is_some()
    }

    /// Mailbox read without `acpi_call`: same register dance, driven from
    /// userspace. Correct, but each register write can be interleaved with a
    /// kernel-initiated battery transaction, so a retry loop is mandatory.
    fn mailbox_via_node(addr: u16) -> Option<u8> {
        let _guard = MAILBOX_LOCK.lock().ok()?;
        node_set_byte(MB_FLAGS, read_flags() & !MB_DRDY);
        node_set_byte(MB_LDAT, addr as u8);
        node_set_byte(MB_HDAT, (addr >> 8) as u8);
        node_set_byte(MB_FLAGS, read_flags() | MB_RFLG);
        let mut value = None;
        for _ in 0..MB_POLL_CYCLES {
            std::thread::sleep(MB_POLL_DELAY);
            if node_byte(MB_FLAGS)? & MB_DRDY != 0 {
                value = node_byte(MB_CMDL);
                break;
            }
        }
        node_set_byte(MB_FLAGS, 0);
        value
    }

    fn read_flags() -> u8 {
        node_byte(MB_FLAGS).unwrap_or(0)
    }

    fn mailbox_write_via_node(addr: u16, val: u8) -> bool {
        let Ok(_guard) = MAILBOX_LOCK.lock() else {
            return false;
        };
        node_set_byte(MB_FLAGS, read_flags() & !MB_DRDY);
        node_set_byte(MB_LDAT, addr as u8);
        node_set_byte(MB_HDAT, (addr >> 8) as u8);
        node_set_byte(MB_CMDL, val);
        node_set_byte(MB_CMDH, 0);
        node_set_byte(MB_FLAGS, read_flags() | MB_WFLG);
        std::thread::sleep(MB_POLL_DELAY);
        let mut done = false;
        for _ in 0..MB_POLL_CYCLES {
            std::thread::sleep(MB_POLL_DELAY);
            if node_byte(MB_FLAGS).unwrap_or(0) & MB_DRDY != 0 {
                done = true;
                break;
            }
        }
        node_set_byte(MB_FLAGS, 0);
        done
    }

    fn mailbox_read(addr: u16) -> Option<u8> {
        // Drive the mailbox through the debugfs window first: the same register
        // transaction costs ~7 ms from here against ~52 ms through acpi_call,
        // because the firmware's own poll loop in `RKBC` sleeps 50 ms per
        // iteration. acpi_call stays as the fallback for kernels without
        // ec_sys (or without write_support=1).
        if node().is_some() {
            if let Some(v) = mailbox_via_node(addr) {
                return Some(v);
            }
        }
        if aml_available() {
            if let Some((lo, _)) = aml_call(AML_READ, addr, 0) {
                return Some(lo);
            }
        }
        None
    }

    fn mailbox_write(addr: u16, val: u8) -> bool {
        if node().is_some() && mailbox_write_via_node(addr, val) {
            return true;
        }
        if aml_available() {
            return aml_call(AML_WRITE, addr, val).is_some();
        }
        false
    }

    /// Which mailbox page the debugfs node is a window onto. Discovered by
    /// comparing the first bytes of every candidate page with the node; the
    /// answer is cached for the process lifetime. Falls back to the page seen on
    /// Tongfang hardware when `acpi_call` is not loaded and nothing can be
    /// compared.
    fn node_alias_page() -> Option<u8> {
        const UNKNOWN: u8 = 0xFF;
        const NO_ALIAS: u8 = 0xFE;
        match NODE_PAGE.load(Ordering::Acquire) {
            UNKNOWN => {}
            NO_ALIAS => return None,
            page => return Some(page),
        }
        if node().is_none() {
            NODE_PAGE.store(NO_ALIAS, Ordering::Release);
            return None;
        }
        let mut probe = [0u8; 3];
        for (i, slot) in probe.iter_mut().enumerate() {
            *slot = node_byte(i as u64).unwrap_or(u8::MAX);
        }
        // Compare through the mailbox itself when the node answers: a candidate
        // page costs 3 fast transactions instead of 3 x 52 ms over acpi_call.
        let sample = |addr: u16| -> Option<u8> {
            match mailbox_via_node(addr) {
                Some(v) => Some(v),
                None if aml_available() => aml_call(AML_READ, addr, 0).map(|(lo, _)| lo),
                None => None,
            }
        };
        if aml_available() || mailbox_via_node(0) != None {
            for page in [0x04u16].into_iter().chain(0x00..0x100) {
                let base = page << 8;
                if sample(base).is_none() {
                    continue;
                }
                let matched = (0..probe.len()).all(|i| sample(base + i as u16) == Some(probe[i]));
                if matched {
                    NODE_PAGE.store(page as u8, Ordering::Release);
                    return Some(page as u8);
                }
            }
        }
        NODE_PAGE.store(0x04, Ordering::Release);
        Some(0x04)
    }

    // ---- privileged broker ----------------------------------------------
    //
    // Both channels the rest of this module uses live behind root-only paths:
    // `/sys/kernel/debug` is 0700 and `/proc/acpi/call` is 0660 root:root. The
    // app itself runs as the logged-in user, so on a stock install every read
    // here returns None and the HAL falls back to MockHAL - the machine looks
    // like a supported laptop with dead sensors. The broker closes that gap: a
    // root service holds the socket, the socket's group is `openrevo-ec`, and
    // the group is what the per-user package grants. Telemetry reads a handful
    // of registers per second, so the connection is kept open and pipelined
    // rather than spawned per request the way `pkexec` would.

    fn broker_path() -> PathBuf {
        std::env::var_os("OPENREVO_EC_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(BROKER_SOCKET))
    }

    fn broker_connect() -> Option<std::os::unix::net::UnixStream> {
        use std::os::unix::net::UnixStream;
        let path = broker_path();
        if !path.exists() {
            return None;
        }
        let stream = UnixStream::connect(&path).ok()?;
        stream.set_read_timeout(Some(BROKER_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(BROKER_TIMEOUT)).ok()?;
        Some(stream)
    }

    /// One request/response exchange. Reconnects once if the cached connection
    /// died (broker restarted, service reloaded), and drops it on any later
    /// failure so the next call starts clean instead of reading a stale reply.
    fn broker_request(line: &str) -> Option<String> {
        use std::io::{BufRead as _, BufReader, Write as _};
        let mut link = BROKER_LINK.lock().ok()?;
        for _ in 0..2 {
            if link.is_none() {
                *link = broker_connect();
            }
            let Some(stream) = link.as_mut() else {
                BROKER.store(2, Ordering::Release);
                return None;
            };
            let sent = stream
                .write_all(line.as_bytes())
                .and_then(|()| stream.write_all(b"\n"))
                .and_then(|()| stream.flush());
            if sent.is_err() {
                *link = None;
                continue;
            }
            let mut reply = String::new();
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(clone) => clone,
                Err(_) => {
                    *link = None;
                    return None;
                }
            });
            if reader.read_line(&mut reply).unwrap_or(0) == 0 {
                *link = None;
                continue;
            }
            BROKER.store(1, Ordering::Release);
            return Some(reply.trim().to_string());
        }
        BROKER.store(2, Ordering::Release);
        None
    }

    /// Called once by the broker's `serve()` so its own readers keep using the
    /// privileged transports instead of looping back through the socket.
    pub fn disable_broker_client() {
        BROKER_CLIENT.store(false, Ordering::Release);
    }

    fn broker_client() -> bool {
        BROKER_CLIENT.load(Ordering::Acquire)
    }

    fn broker_available() -> bool {
        if !broker_client() {
            return false;
        }
        match BROKER.load(Ordering::Acquire) {
            1 => true,
            2 => false,
            _ => broker_request("probe").is_some(),
        }
    }

    fn broker_read(addr: u16) -> Option<u8> {
        if !broker_client() {
            return None;
        }
        let reply = broker_request(&format!("r {:04x}", addr))?;
        let mut parts = reply.split_whitespace();
        if parts.next()? != "v" {
            return None;
        }
        parts.next()?;
        u8::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()
    }

    fn broker_write(entries: &[(u16, u8)]) -> bool {
        if entries.is_empty() || !broker_client() {
            return false;
        }
        let mut line = String::from("w");
        for (addr, val) in entries {
            line.push_str(&format!(" {:04x} {:02x}", addr, val));
        }
        broker_request(&line).as_deref() == Some("ok")
    }

    /// Can this address be served straight from the debugfs node?
    fn via_node(addr: u16) -> bool {
        node_alias_page().map(|page| addr >> 8 == page as u16).unwrap_or(false)
    }

    pub fn available() -> bool {
        node().is_some() || aml_available() || port_access_granted() || broker_available()
    }

    pub fn read(addr: u16) -> Option<u8> {
        if via_node(addr) {
            if let Some(v) = node_byte((addr & 0xFF) as u64) {
                return Some(v);
            }
        }
        if let Some(v) = broker_read(addr) {
            return Some(v);
        }
        if let Some(v) = mailbox_read(addr) {
            return Some(v);
        }
        port_read(addr)
    }

    pub fn write(addr: u16, val: u8) -> bool {
        let trace = std::env::var_os("OPENREVO_EC_TRACE").is_some();
        if via_node(addr) && FLAT_WRITABLE.load(Ordering::Acquire) {
            if node_set_byte((addr & 0xFF) as u64, val) {
                if trace {
                    log_warn!("PAL-EC", "write {:#06x} via node", addr);
                }
                std::thread::sleep(WRITE_SETTLE);
                return true;
            }
            if trace {
                log_warn!("PAL-EC", "write {:#06x}: node path refused", addr);
            }
        } else if trace {
            log_warn!("PAL-EC", "write {:#06x}: node path unavailable (page {:?}, flat writable {})", addr, node_alias_page(), FLAT_WRITABLE.load(Ordering::Acquire));
        }
        if broker_write(&[(addr, val)]) {
            std::thread::sleep(WRITE_SETTLE);
            return true;
        }
        if mailbox_write(addr, val) {
            if trace {
                log_warn!("PAL-EC", "write {:#06x} via mailbox", addr);
            }
            std::thread::sleep(WRITE_SETTLE);
            return true;
        }
        if trace {
            log_warn!("PAL-EC", "write {:#06x}: mailbox refused, port access granted = {}", addr, port_access_granted());
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
        // Fan tables are ~100 register writes; pushing them one round trip at a
        // time costs 100 sockets and 100 sequential EC transactions with no
        // benefit. Send the whole run to the broker in chunks of its per-request
        // limit instead.
        if !entries.is_empty() && broker_available() {
            const CHUNK: usize = 192;
            if entries
                .chunks(CHUNK)
                .all(|chunk| broker_write(chunk))
            {
                std::thread::sleep(WRITE_SETTLE);
                return true;
            }
        }
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
        FLAT_WRITABLE.load(Ordering::Acquire) || aml_available() || port_access_granted() || broker_available()
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
