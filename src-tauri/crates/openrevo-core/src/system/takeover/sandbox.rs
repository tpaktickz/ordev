//! Autostart, registry, and service isolation sandbox.
//!
//! Level 1.1 of the takeover spec, expressed portably:
//! - OEM autostart entries neutralized and restorable: Windows Run keys (backed
//!   up under `OpenRevoBackup_`) and Scheduled Tasks; on Linux, XDG autostart
//!   entries and OEM systemd units.
//! - Modern startup-task toggles: UWP AppModel `State` on Windows, no-op on
//!   Linux (there is no AppModel).
//! - Shell notification-area refresh: `Shell_TrayWnd` poking on Windows; the
//!   StatusNotifierItem host has no equivalent, so Linux is a no-op.
//!
//! All registry access goes through [`crate::pal::reg`], which is a no-op on
//! Linux — the Windows branches below therefore stay readable.

use crate::pal::reg::{self, RegRoot};

/// OEM startup task/unit names shared by every Uniwill/Mechrevo variant.
pub const OEM_TASKS: &[&str] = &[
    "ControlCenter",
    "ControlCenterTray",
    "GamingCenter",
    "GCUService",
    "GCUBridge",
    "AiStone",
    "AiStoneService",
    "OSDTpDetect",
    "UniwillService",
    "CCU",
    "CCU.WinUI",
    "OEMService",
];

/// Substrings that identify an OEM value name in Run keys / StartupApproved.
const OEM_NAME_PATTERNS: &[&str] = &[
    "controlcenter",
    "gamingcenter",
    "gcuservice",
    "gcubridge",
    "aistone",
    "mycontrolcenter",
    "uniwill",
    "osdtp",
    "systray",
    "ccu",
];

/// Run-key locations the OEM software uses, per root.
const RUN_KEYS: &[(RegRoot, &str)] = &[
    (RegRoot::Machine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run"),
    (RegRoot::Machine, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run"),
    (RegRoot::User, r"Software\Microsoft\Windows\CurrentVersion\Run"),
];

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
fn run_cmd_quiet(cmd: &str, args: &[&str]) -> (Option<i32>, String, String) {
    use std::os::windows::process::CommandExt;
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

/// Fast check whether a registry subkey exists. Linux: always `false`.
pub fn reg_key_exists(root: RegRoot, subkey: &str) -> bool {
    reg::key_exists(root, subkey)
}

/// Write a DWORD value. Linux: no-op.
pub fn set_reg_dword(root: RegRoot, subkey: &str, value_name: &str, val: u32) {
    reg::set_dword(root, subkey, value_name, val);
}

/// Neutralize OEM autostart entries by moving them aside (reversible).
pub fn neutralize_run_keys() {
    for (root, subkey) in RUN_KEYS {
        let moved = reg::backup_matching_values(*root, subkey, OEM_NAME_PATTERNS);
        if moved > 0 {
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "Run key '{}': backed up and disabled {} entr{}",
                subkey,
                moved,
                if moved == 1 { "y" } else { "ies" }
            );
        }
    }
}

/// Restore OEM autostart entries previously moved aside.
pub fn restore_run_keys() {
    for (root, subkey) in RUN_KEYS {
        let restored = reg::restore_backup_values(*root, subkey);
        if restored > 0 {
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "Run key '{}': restored {} entr{}",
                subkey,
                restored,
                if restored == 1 { "y" } else { "ies" }
            );
        }
    }
}

#[cfg(windows)]
fn oem_task_registered(task: &str) -> bool {
    let key_path = format!(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree\{}",
        task
    );
    reg_key_exists(RegRoot::Machine, &key_path)
}

/// Disable every OEM startup task that exists on this machine.
pub fn disable_scheduled_tasks() {
    #[cfg(windows)]
    for task in OEM_TASKS {
        if oem_task_registered(task) {
            let (code, _, _) = run_cmd_quiet("schtasks.exe", &["/change", "/tn", task, "/disable"]);
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "Task '{}': disabled (code: {:?})",
                task,
                code
            );
        }
    }

    #[cfg(target_os = "linux")]
    for unit in oem_systemd_units() {
        let (code, _, err) = crate::pal::process::run_quiet(
            "systemctl",
            &["--user", "mask", "--now", &unit],
        );
        crate::log_info!(
            "TAKEOVER_SANDBOX",
            "Unit '{}': masked (code: {:?}{})",
            unit,
            code,
            if err.is_empty() {
                String::new()
            } else {
                format!(", {err}")
            }
        );
    }
}

/// Re-enable every OEM startup task that exists on this machine.
pub fn enable_scheduled_tasks() {
    #[cfg(windows)]
    for task in OEM_TASKS {
        if oem_task_registered(task) {
            let (code, _, _) = run_cmd_quiet("schtasks.exe", &["/change", "/tn", task, "/enable"]);
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "Task '{}': enabled (code: {:?})",
                task,
                code
            );
        }
    }

    #[cfg(target_os = "linux")]
    for unit in oem_systemd_units() {
        let (code, _, _) =
            crate::pal::process::run_quiet("systemctl", &["--user", "unmask", &unit]);
        crate::log_info!(
            "TAKEOVER_SANDBOX",
            "Unit '{}': unmasked (code: {:?})",
            unit,
            code
        );
    }
}

/// OEM systemd units that actually exist for this user, derived from
/// [`OEM_TASKS`] by lowercasing and probing both `.service` and the bare name.
#[cfg(target_os = "linux")]
fn oem_systemd_units() -> Vec<String> {
    let mut units = Vec::new();
    for task in OEM_TASKS {
        let base = task.to_lowercase();
        for candidate in [format!("{base}.service"), format!("openrevo-oem-{base}.service")] {
            let (code, _, _) = crate::pal::process::run_quiet(
                "systemctl",
                &["--user", "cat", &candidate],
            );
            if code == Some(0) && !units.contains(&candidate) {
                units.push(candidate);
            }
        }
    }
    units
}

/// Toggle UWP AppModel startup-task state: `1` disabled, `2` enabled.
/// Linux: no-op (no AppModel), the dynamic scan finds nothing.
pub fn set_uwp_appmodel_startup_state(disable: bool) {
    let target_state: u32 = if disable { 1 } else { 2 };
    const BASE: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

    let known = [
        r"ControlCenter3_wrbgcf7aesyd8\ControlCenterTray",
        r"ControlCenter3_h329z55cwnj8g\ControlCenterTray",
        r"CCU.WinUI_wrbgcf7aesyd8\ControlCenterTray",
        r"CCU.WinUI_wrbgcf7aesyd8\CCU",
        r"CCU.WinUI_wrbgcf7aesyd8\SystrayComponent",
    ];
    for leaf in known {
        let subkey = format!(r"{BASE}\{leaf}");
        if reg_key_exists(RegRoot::User, &subkey) {
            set_reg_dword(RegRoot::User, &subkey, "State", target_state);
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "UWP AppModel task '{}': State set to {}",
                subkey,
                target_state
            );
        }
    }

    // Dynamic scan: package families differ per OEM build, and users may have
    // installed a variant we don't list.
    for family in reg::enum_subkeys(RegRoot::User, BASE) {
        let family_lower = family.to_lowercase();
        if !(family_lower.contains("controlcenter")
            || family_lower.contains("gamingcenter")
            || family_lower.contains("ccu"))
        {
            continue;
        }
        let family_key = format!(r"{BASE}\{family}");
        for task in reg::enum_subkeys(RegRoot::User, &family_key) {
            let task_lower = task.to_lowercase();
            if !(task_lower.contains("tray")
                || task_lower.contains("ccu")
                || task_lower.contains("systray"))
            {
                continue;
            }
            let full = format!(r"{family_key}\{task}");
            set_reg_dword(RegRoot::User, &full, "State", target_state);
            crate::log_info!(
                "TAKEOVER_SANDBOX",
                "UWP AppModel dynamic task '{}': State set to {}",
                full,
                target_state
            );
        }
    }
}

/// Update Explorer `StartupApproved\StartupTask` byte flags
/// (`0x02` enabled, `0x03` disabled). Linux: no-op.
pub fn update_uwp_startup_task(disable: bool) {
    const SUBKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupTask";
    let target_byte = if disable { 0x03 } else { 0x02 };
    let patched = reg::patch_first_byte_of_matching_values(
        RegRoot::User,
        SUBKEY,
        &["controlcenter", "gamingcenter", "ccu"],
        target_byte,
    );
    if patched > 0 {
        crate::log_info!(
            "TAKEOVER_SANDBOX",
            "StartupApproved: set 0x{:02X} on {} entr{}",
            target_byte,
            patched,
            if patched == 1 { "y" } else { "ies" }
        );
    }
}

/// Sweep ghost icons out of the Windows notification area. Linux has no
/// equivalent window — StatusNotifierItem hosts re-query items themselves.
pub fn refresh_tray_notification_area() {
    #[cfg(windows)]
    unsafe {
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetClientRect, SendMessageW, WM_MOUSEMOVE,
        };

        let refresh_toolbar = |h_toolbar: HWND| {
            if h_toolbar.0.is_null() {
                return;
            }
            let mut rect = RECT::default();
            if GetClientRect(h_toolbar, &mut rect).is_ok() {
                let mut x = 0;
                while x < rect.right {
                    let mut y = 0;
                    while y < rect.bottom {
                        let l_param = LPARAM(((y << 16) | (x & 0xFFFF)) as isize);
                        let _ = SendMessageW(h_toolbar, WM_MOUSEMOVE, WPARAM(0), l_param);
                        y += 8;
                    }
                    x += 8;
                }
            }
        };

        if let Ok(h_tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            if let Ok(h_notify) =
                FindWindowExW(h_tray, HWND::default(), w!("TrayNotifyWnd"), PCWSTR::null())
            {
                if let Ok(h_sys_pager) =
                    FindWindowExW(h_notify, HWND::default(), w!("SysPager"), PCWSTR::null())
                {
                    if let Ok(h_tb1) = FindWindowExW(
                        h_sys_pager,
                        HWND::default(),
                        w!("ToolbarWindow32"),
                        PCWSTR::null(),
                    ) {
                        refresh_toolbar(h_tb1);
                    }
                }
                let mut h_tb_direct = HWND::default();
                while let Ok(next) =
                    FindWindowExW(h_notify, h_tb_direct, w!("ToolbarWindow32"), PCWSTR::null())
                {
                    if next.0.is_null() {
                        break;
                    }
                    refresh_toolbar(next);
                    h_tb_direct = next;
                }
            }
        }

        if let Ok(h_overflow) = FindWindowW(w!("NotifyIconOverflowWindow"), PCWSTR::null()) {
            let mut h_tb_overflow = HWND::default();
            while let Ok(next) = FindWindowExW(
                h_overflow,
                h_tb_overflow,
                w!("ToolbarWindow32"),
                PCWSTR::null(),
            ) {
                if next.0.is_null() {
                    break;
                }
                refresh_toolbar(next);
                h_tb_overflow = next;
            }
        }
    }

    #[cfg(target_os = "linux")]
    crate::log_info!(
        "TAKEOVER_SANDBOX",
        "Notification-area sweep is Windows-only; StatusNotifierItem hosts re-query on their own"
    );
}
