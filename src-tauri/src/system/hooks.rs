//! Keyboard hook facade.
//!
//! Windows installs a `WH_KEYBOARD_LL` hook on a dedicated message pump to
//! implement the Win-key lock and the Caps/Num/Scroll OSD. Linux has no global
//! low-level keyboard hook that a userspace process can install (Wayland
//! forbids it by design), so the public API is kept and backed by the evdev
//! reader below — same three entry points, no message pump.

#[cfg(windows)]
#[path = "hooks_win.rs"]
mod imp;

#[cfg(windows)]
pub use imp::{init_hooks, is_win_key_locked, set_win_key_locked};

#[cfg(not(windows))]
mod imp_linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::AppHandle;

    /// Win-key lock is meaningless on Linux — but the setting is persisted in the
    /// config and the UI exposes a toggle, so keep the state and report it back.
    static WIN_KEY_LOCKED: AtomicBool = AtomicBool::new(false);

    pub fn is_win_key_locked() -> bool {
        WIN_KEY_LOCKED.load(Ordering::SeqCst)
    }

    pub fn set_win_key_locked(locked: bool) {
        WIN_KEY_LOCKED.store(locked, Ordering::SeqCst);
    }

    /// Registers the shortcut layer and reports what the evdev backend cannot do.
    ///
    /// Caps/Num/Scroll state changes and the Win-key suppression are read from
    /// `/dev/input/event*` once the evdev reader lands; until then the shortcut
    /// dispatcher (XDG portal / compositor bindings) is the only key source.
    pub fn init_hooks(_app: Option<&AppHandle>) {
        crate::system::shortcuts::init_shortcuts();
        crate::log_info!(
            "HOOKS",
            "evdev keyboard layer not active on this build; shortcuts come from the shortcut dispatcher"
        );
    }
}

#[cfg(not(windows))]
pub use imp_linux::{init_hooks, is_win_key_locked, set_win_key_locked};
