//! Keyboard hook facade.
//!
//! Windows installs a `WH_KEYBOARD_LL` hook on a dedicated message pump to
//! implement the Win-key lock and the Caps/Num/Scroll OSD. Wayland forbids a
//! userspace global hook, so the Linux backend (`hooks_linux.rs`) drives the same
//! behaviours from a passive evdev reader — same entry points, no message pump.

#[cfg(windows)]
#[path = "hooks_win.rs"]
mod imp;

#[cfg(windows)]
pub use imp::{init_hooks, is_win_key_locked, set_win_key_locked};

#[cfg(not(windows))]
#[path = "hooks_linux.rs"]
mod imp_linux;

#[cfg(not(windows))]
pub use imp_linux::{init_hooks, is_key_down, is_win_key_locked, set_win_key_locked};
