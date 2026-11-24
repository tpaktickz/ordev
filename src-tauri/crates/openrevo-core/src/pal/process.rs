// PAL — process enumeration and OEM-installation detection.
//
// Windows: Toolhelp32 snapshot + registry probes (unchanged semantics).
// Linux:   /proc scan; "installation" means an OEM companion package is
//          present on disk or running (there is no registry).

/// Snapshot of running processes as `(pid, lowercased name)`.
pub fn snapshot() -> Vec<(u32, String)> {
    #[cfg(windows)]
    {
        windows_impl::snapshot()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::snapshot()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Terminate a process by PID. Returns whether a kill signal was delivered.
pub fn terminate(pid: u32) -> bool {
    #[cfg(windows)]
    {
        windows_impl::terminate(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::terminate(pid)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

/// Whether the official OEM control-center software exists on this machine.
/// On Linux this gates the takeover UI the same way a clean Windows install does.
pub fn oem_installed() -> bool {
    #[cfg(windows)]
    {
        windows_impl::oem_installed()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::oem_installed()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

/// Names of OEM companion processes, lowercased. Shared by both platforms.
pub const OEM_PROCESS_NAMES: &[&str] = &[
    "gcubridge.exe",
    "gcubridge",
    "gcubridgewmi.exe",
    "gcubridgewmi",
    "gcu.exe",
    "gcu",
    "controlcenter.exe",
    "controlcenter",
    "control_center.exe",
    "control_center",
    "aistoneservice.exe",
    "aistoneservice",
    "unwillservice.exe",
    "uniwillservice.exe",
    "uniwillservice",
    "oemservice.exe",
    "oemservice",
    "gamingcenter.exe",
    "gamingcenter",
];

/// Run a command and capture its exit code plus trimmed stdout/stderr.
/// Windows hides the console window; Linux runs it as-is.
/// Builds a `Command` that will not flash a console window on Windows and is a
/// plain process everywhere else. Use this instead of reaching for
/// `CommandExt::creation_flags`, which only exists on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Run a command and capture its exit code plus trimmed stdout/stderr.
/// Windows hides the console window.
#[cfg(windows)]
pub fn run_quiet(cmd: &str, args: &[&str]) -> (Option<i32>, String, String) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new(cmd)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ),
        Err(e) => (None, String::new(), e.to_string()),
    }
}

/// See the Windows variant.
#[cfg(not(windows))]
pub fn run_quiet(cmd: &str, args: &[&str]) -> (Option<i32>, String, String) {
    match std::process::Command::new(cmd).args(args).output() {
        Ok(out) => (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ),
        Err(e) => (None, String::new(), e.to_string()),
    }
}

#[cfg(windows)]
mod windows_impl {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    fn name_of(entry: &PROCESSENTRY32W) -> String {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase()
    }

    pub fn snapshot() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        unsafe {
            let Ok(handle) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            if handle == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(handle, &mut entry).is_ok() {
                loop {
                    out.push((entry.th32ProcessID, name_of(&entry)));
                    if Process32NextW(handle, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(handle);
        }
        out
    }

    pub fn terminate(pid: u32) -> bool {
        unsafe {
            match OpenProcess(PROCESS_TERMINATE, false, pid) {
                Ok(handle) => {
                    let ok = TerminateProcess(handle, 0).is_ok();
                    let _ = CloseHandle(handle);
                    ok
                }
                Err(_) => false,
            }
        }
    }

    pub fn oem_installed() -> bool {
        use super::OEM_PROCESS_NAMES;
        use std::path::Path;
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
        };

        fn key_exists(root: HKEY, path: &str) -> bool {
            let hkey = HSTRING::from(path);
            let mut subkey = HKEY::default();
            let res = unsafe {
                RegOpenKeyExW(root, windows::core::PCWSTR(hkey.as_ptr()), 0, KEY_READ, &mut subkey)
            };
            if res.is_ok() {
                let _ = unsafe { RegCloseKey(subkey) };
                return true;
            }
            false
        }

        for key in [
            r"SYSTEM\CurrentControlSet\Services\GCUBridge",
            r"SYSTEM\CurrentControlSet\Services\GCUService",
            r"SYSTEM\CurrentControlSet\Services\AiStoneService",
            r"SYSTEM\CurrentControlSet\Services\UniwillService",
            r"SYSTEM\CurrentControlSet\Services\OEMService",
        ] {
            if key_exists(HKEY_LOCAL_MACHINE, key) {
                return true;
            }
        }

        for path in [
            r"C:\Program Files\OEM\智控中心",
            r"C:\Program Files\OEM\机械革命电竞控制台",
            r"C:\Program Files\OEM\机械革命控制中心",
            r"C:\Program Files\OEM\AiStoneService",
            r"C:\Program Files\OEM\ControlCenter",
            r"C:\Program Files\OEM\GamingCenter",
            r"C:\Program Files (x86)\OEM",
            r"C:\Program Files\Uniwill",
            r"C:\Program Files (x86)\Uniwill",
            r"C:\Program Files\ControlCenter",
            r"C:\Program Files (x86)\ControlCenter",
        ] {
            if Path::new(path).exists() {
                return true;
            }
        }

        if let Ok(entries) = std::fs::read_dir(r"C:\Program Files\OEM") {
            if entries.filter_map(|e| e.ok()).next().is_some() {
                return true;
            }
        }

        for key in [
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\CCU.WinUI_wrbgcf7aesyd8",
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\ControlCenter3_wrbgcf7aesyd8",
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\ControlCenter3_h329z55cwnj8g",
        ] {
            if key_exists(HKEY_CURRENT_USER, key) {
                return true;
            }
        }

        snapshot()
            .iter()
            .any(|(_, name)| OEM_PROCESS_NAMES.contains(&name.as_str()))
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::OEM_PROCESS_NAMES;
    use std::path::Path;

    /// `/proc/<pid>/comm` is truncated to 15 chars by the kernel, which is fine
    /// for every OEM companion name above.
    pub fn snapshot() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return out;
        };
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
                continue;
            };
            out.push((pid, comm.trim().to_lowercase()));
        }
        out
    }

    pub fn terminate(pid: u32) -> bool {
        std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// No registry on Linux: an OEM control center is "installed" when its
    /// package left files behind or a companion process is running. Paths cover
    /// the third-party native ports and Wine prefixes alike.
    pub fn oem_installed() -> bool {
        for path in [
            "/opt/mechrevo",
            "/opt/uniwill",
            "/opt/openrevo-oem",
            "/usr/lib/mechrevo",
            "/usr/share/mechrevo",
        ] {
            if Path::new(path).exists() {
                return true;
            }
        }
        snapshot()
            .iter()
            .any(|(_, name)| OEM_PROCESS_NAMES.contains(&name.as_str()))
    }
}
