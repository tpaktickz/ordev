// Linux autostart via the XDG autostart spec.
//
// Drop-in replacement for the Windows scheduled-task backend: same public
// surface, so callers stay untouched. A `.desktop` file in
// `$XDG_CONFIG_HOME/autostart` is what KDE/GNOME systemd-xdg-autostart consume.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::system::query_cache::ExpiringCache;

const DESKTOP_FILE: &str = "openrevo.desktop";
const EXEC_ARG: &str = "--minimized";

/// Same role and TTL as the Windows cache: the file only changes when this app
/// or the user touches it, and both paths invalidate explicitly.
static AUTOSTART_CACHE: LazyLock<ExpiringCache<bool>> =
    LazyLock::new(|| ExpiringCache::new(Duration::from_secs(5)));

fn autostart_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("autostart"))
}

fn desktop_path() -> Option<PathBuf> {
    autostart_dir().map(|d| d.join(DESKTOP_FILE))
}

pub fn invalidate_autostart_cache() {
    AUTOSTART_CACHE.invalidate();
}

pub fn is_autostart_enabled() -> bool {
    desktop_path().map(|p| p.is_file()).unwrap_or(false)
}

pub fn is_autostart_enabled_cached() -> bool {
    AUTOSTART_CACHE.get_or_compute(is_autostart_enabled)
}

/// Parse the executable out of `Exec=` in the autostart entry, if present.
pub fn get_scheduled_task_exe_path() -> Option<PathBuf> {
    let text = std::fs::read_to_string(desktop_path()?).ok()?;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("Exec=") else {
            continue;
        };
        let rest = rest.trim();
        let exe = if let Some(inner) = rest.strip_prefix('"') {
            inner.split('"').next().unwrap_or(inner)
        } else {
            rest.split_whitespace().next().unwrap_or(rest)
        };
        if exe.is_empty() {
            return None;
        }
        return Some(PathBuf::from(exe));
    }
    None
}

fn normalize_path_str(p: &Path) -> String {
    p.to_string_lossy().trim_matches('"').trim().to_string()
}

/// Create, delete, or repair the autostart entry so it points at this binary.
pub fn set_autostart_enabled(enabled: bool) -> Result<bool, String> {
    let Some(path) = desktop_path() else {
        return Err("Cannot resolve XDG autostart directory (no HOME)".into());
    };

    if !enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("removing {}: {e}", path.display())),
        }
        invalidate_autostart_cache();
        return Ok(false);
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_string_lossy().replace('"', "\\\"");
    let log_arg = if crate::system::logger::is_cli_log_active()
        && crate::system::config::load_config().log_enabled
    {
        " --log"
    } else {
        ""
    };

    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=OpenRevo\n\
         Comment=Mechrevo control center\n\
         Exec=\"{exe_str}\" {EXEC_ARG}{log_arg}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-after=panel\n"
    );

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, body).map_err(|e| format!("writing {}: {e}", path.display()))?;

    invalidate_autostart_cache();
    Ok(true)
}

/// Keep the entry honest: repair a stale target path, or restore a missing entry
/// when config says autostart is on.
pub fn sync_autostart_target_path() {
    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };

    let cfg = crate::system::config::load_config();

    if is_autostart_enabled() {
        if let Some(entry_exe) = get_scheduled_task_exe_path() {
            if normalize_path_str(&entry_exe) != normalize_path_str(&current_exe) {
                crate::log_info!(
                    "AUTOSTART",
                    "Autostart entry points at '{}' but we run from '{}'. Rewriting...",
                    entry_exe.display(),
                    current_exe.display()
                );
                let _ = set_autostart_enabled(true);
            }
        }
    } else if cfg.autostart {
        crate::log_info!(
            "AUTOSTART",
            "Config has autostart=true but no XDG entry exists. Restoring for {:?}...",
            current_exe
        );
        let _ = set_autostart_enabled(true);
    }
}

pub use sync_autostart_target_path as heal_startup_task_if_moved;
