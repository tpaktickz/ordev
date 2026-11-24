use std::sync::LazyLock;
use std::time::Duration;

use crate::pal::process as pal_proc;
use crate::system::query_cache::ExpiringCache;

/// Both of these are asked on every telemetry heartbeat (1.5 s). A process
/// snapshot and a dozen install probes are cheap individually but pointless at
/// that rate: OEM software only appears or disappears when the user installs,
/// uninstalls, takes over or restores it.
static OEM_RUNNING_CACHE: LazyLock<ExpiringCache<bool>> =
    LazyLock::new(|| ExpiringCache::new(Duration::from_secs(5)));
static OEM_INSTALLED_CACHE: LazyLock<ExpiringCache<bool>> =
    LazyLock::new(|| ExpiringCache::new(Duration::from_secs(60)));

/// Drop cached OEM detection results. Call after taking over or restoring the
/// official control center, so the UI reflects the change immediately.
pub fn invalidate_oem_detection_cache() {
    OEM_RUNNING_CACHE.invalidate();
    OEM_INSTALLED_CACHE.invalidate();
}

/// OEM companion process names, without `.exe`. `/proc/<pid>/comm` and the
/// Windows image name are both reduced to this base before comparison.
const OEM_PROCESS_BASES: &[&str] = &[
    "gcubridge",
    "gcuservice",
    "aistoneservice",
    "aistone",
    "mycontrolcenter",
    "controlcenteru",
    "gamingcenteru",
    "controlcenter",
    "gamingcenter",
    "gamingcenter3_cross",
    "systraycomponent",
    "ccuwinui",
    "systemhelper",
    "defaulttool",
    "uniwillservice",
    "unwservice",
    "osdtpdetect",
    "displayutil",
    "sysconutil",
    "bldhotkeyservice",
    "bldfnhotkeyutility",
    "蛟龙游戏控制中心",
    "jiaolongcc",
];

/// Match an image name against the OEM companion list. Accepts the name with or
/// without `.exe`, case-insensitively.
pub fn is_oem_process_name(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    OEM_PROCESS_BASES.contains(&base)
}

/// Force terminate all official OEM background processes.
/// Returns how many were signalled.
pub fn kill_oem_processes_native() -> usize {
    let mut killed = 0;
    for (pid, name) in pal_proc::snapshot() {
        if is_oem_process_name(&name) && pal_proc::terminate(pid) {
            killed += 1;
        }
    }
    killed
}

/// Check if official Mechrevo/Uniwill background processes are actively running.
/// We check every companion process (including GCUService, GCUBridge, AiStoneService, etc.).
/// If even ONE of them is still running (e.g., user disabled GCUService with Huorong but GCUBridge is still active),
/// this returns true so the user is given the option to take over and stop it!
///
/// TTL-cached (see [`OEM_RUNNING_CACHE`]) for hot polling paths; use
/// [`probe_oem_service_running`] when an immediate answer is required.
pub fn is_oem_service_running() -> bool {
    OEM_RUNNING_CACHE.get_or_compute(probe_oem_service_running)
}

/// Uncached process scan for OEM companion processes.
pub fn probe_oem_service_running() -> bool {
    pal_proc::snapshot()
        .iter()
        .any(|(_, name)| is_oem_process_name(name))
}

/// Check if official Mechrevo/Uniwill Control Center is installed on this system.
/// On Windows: 3-tier validation (service registry keys, install directories,
/// running processes). On Linux: package paths plus running processes.
/// If the machine has NEVER had the official software, this returns false and
/// the "接管/恢复官方控制台" button stays hidden.
///
/// TTL-cached (see [`OEM_INSTALLED_CACHE`]) for hot polling paths; use
/// [`probe_oem_installed`] when an immediate answer is required.
pub fn is_oem_installed() -> bool {
    OEM_INSTALLED_CACHE.get_or_compute(probe_oem_installed)
}

/// Uncached OEM installation probe.
pub fn probe_oem_installed() -> bool {
    pal_proc::oem_installed()
}

/// Check if a specific process is actively running (case-insensitive, with or
/// without `.exe`), returning its PID if found.
pub fn is_process_running(target_name: &str) -> Option<u32> {
    pal_proc::snapshot()
        .into_iter()
        .find(|(_, name)| is_oem_process_name_match(name, target_name))
        .map(|(pid, _)| pid)
}

fn is_oem_process_name_match(candidate: &str, target: &str) -> bool {
    let c = candidate.trim().to_lowercase();
    let t = target.trim().to_lowercase();
    let c_base = c.strip_suffix(".exe").unwrap_or(&c);
    let t_base = t.strip_suffix(".exe").unwrap_or(&t);
    c_base == t_base
}

/// Retrieve all currently running OEM processes with their names and PIDs.
pub fn get_running_oem_processes() -> Vec<(String, u32)> {
    pal_proc::snapshot()
        .into_iter()
        .filter(|(_, name)| is_oem_process_name(name))
        .map(|(pid, name)| (name, pid))
        .collect()
}

/// Trims the calling process working set, reclaiming unused physical memory and
/// cold DLL pages.
pub fn trim_process_working_set() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
    #[cfg(target_os = "linux")]
    {
        // Only trims the anonymous pages of the caller; the knobs that matter
        // for the whole box are `vm.drop_caches`, which needs root.
        if let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .open("/proc/self/clear_refs")
        {
            use std::io::Write;
            let mut file = file;
            let _ = file.write_all(b"1");
        }
    }
}
