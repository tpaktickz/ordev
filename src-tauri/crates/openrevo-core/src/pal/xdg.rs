//! XDG Base Directory helpers.
//!
//! Follows the freedesktop.org basedir spec, so the engine never hardcodes
//! `~/.config` or `~/.local/share` and respects a relocated `XDG_*_HOME`.
//! Windows falls back to `%APPDATA%` / `%LOCALAPPDATA%`, which keeps the few
//! callers that are shared across platforms branch-free.

use std::path::PathBuf;

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME` (default `~/.config`).
pub fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .or_else(|| env_path("APPDATA"))
        .or_else(|| home().map(|h| h.join(".config")))
        .unwrap_or_else(std::env::temp_dir)
}

/// `$XDG_DATA_HOME` (default `~/.local/share`).
pub fn data_home() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .or_else(|| env_path("LOCALAPPDATA"))
        .or_else(|| home().map(|h| h.join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir)
}

/// `$XDG_CACHE_HOME` (default `~/.cache`).
pub fn cache_home() -> PathBuf {
    env_path("XDG_CACHE_HOME")
        .or_else(|| own_data())
        .or_else(|| home().map(|h| h.join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
}

fn own_data() -> Option<PathBuf> {
    env_path("LOCALAPPDATA")
}

/// Per-user application config directory: `config_home()/openrevo`.
pub fn config_dir() -> PathBuf {
    config_home().join("openrevo")
}

/// Per-user application data directory: `data_home()/openrevo`.
pub fn data_dir() -> PathBuf {
    data_home().join("openrevo")
}

/// `$XDG_RUNTIME_DIR`, when the session sets one (Linux logind).
pub fn runtime_dir() -> Option<PathBuf> {
    env_path("XDG_RUNTIME_DIR")
}

/// Fully qualified paths of every existing `XDG_DATA_DIRS` entry.
pub fn data_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_DATA_DIRS")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .filter(|v: &Vec<PathBuf>| !v.is_empty())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        })
}
