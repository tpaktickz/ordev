//! Linux keyboard layer.
//!
//! Windows installs a `WH_KEYBOARD_LL` hook and can swallow keystrokes. Wayland
//! has no such hook, so this module drives the same three behaviours from the
//! passive evdev reader in `openrevo_core::pal::evdev`:
//!
//! * shortcut bus — every key edge is fed to `check_match_and_trigger`;
//! * Caps/Num/Scroll OSD — the lock state is read from the kernel LED class, the
//!   same way `GetKeyState` reports it on Windows;
//! * Win-key lock — state is kept and exposed; suppressing the Super press
//!   itself needs a `/dev/uinput` remap, which the kernel-side layer does.
//!
//! Nothing here grabs a device (`EVIOCGRAB`), so the compositor keeps receiving
//! every key while the panel is open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use openrevo_core::pal::evdev::{self, KeyPress};

/// The config-driven Win-key lock flag (see `is_win_key_locked`).
static WIN_KEY_LOCKED: AtomicBool = AtomicBool::new(false);
/// Latest known physical state per lock key, seeded at startup.
static CAPS_STATE: AtomicBool = AtomicBool::new(false);
static NUM_STATE: AtomicBool = AtomicBool::new(false);
static SCROLL_STATE: AtomicBool = AtomicBool::new(false);

const LOCK_DEBOUNCE: Duration = Duration::from_millis(180);

struct LockKey {
    name: &'static str,
    state: &'static AtomicBool,
    last: std::sync::Mutex<Option<Instant>>,
}

static LOCK_KEYS: [LockKey; 3] = [
    LockKey { name: "caps", state: &CAPS_STATE, last: std::sync::Mutex::new(None) },
    LockKey { name: "num", state: &NUM_STATE, last: std::sync::Mutex::new(None) },
    LockKey { name: "scroll", state: &SCROLL_STATE, last: std::sync::Mutex::new(None) },
];

static APP: OnceLock<AppHandle> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn is_win_key_locked() -> bool {
    WIN_KEY_LOCKED.load(Ordering::SeqCst)
}

pub fn set_win_key_locked(locked: bool) {
    WIN_KEY_LOCKED.store(locked, Ordering::SeqCst);
}

/// Reads the kernel LED state for a lock key, e.g.
/// `/sys/class/leds/input3::capslock/brightness`.
fn read_led_state(suffix: &str) -> Option<bool> {
    let entries = std::fs::read_dir("/sys/class/leds").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(suffix) {
            continue;
        }
        let raw = std::fs::read_to_string(entry.path().join("brightness")).ok()?;
        return Some(raw.trim() != "0");
    }
    None
}

/// Index into [`LOCK_KEYS`] for a lock-key virtual code.
fn lock_index(vk: u16) -> Option<usize> {
    match vk {
        0x14 => Some(0),
        0x90 => Some(1),
        0x91 => Some(2),
        _ => None,
    }
}

/// Kernel LED class suffix for a lock key.
const LOCK_SUFFIX: [&str; 3] = ["::capslock", "::numlock", "::scrolllock"];

fn notify_lock(index: usize, locked: bool, app: Option<&AppHandle>) {
    let key = &LOCK_KEYS[index];
    crate::log_info!("HOOKS", "{} Lock toggled to {}", key.name, locked);
    crate::system::osd_window::show_key_lock_toast(key.name, locked);
    if let Some(app) = app {
        crate::system::osd_window::wake_osd_if_enabled(app);
        let event = format!("{}-lock-changed", key.name);
        let _ = app.emit(&event, locked);
        let _ = app.emit_to("main", &event, locked);
        let _ = app.emit_to("osd", &event, locked);
    }
}

/// Handles a Caps/Num/Scroll press with the same 180 ms debounce as the Windows
/// hook (filters typematic repeats), then reports the *new* LED state.
fn handle_lock_key(press: KeyPress) {
    let Some(index) = lock_index(press.vk) else {
        return;
    };

    let key = &LOCK_KEYS[index];
    let should_fire = {
        let mut last = key.last.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            Some(previous) => {
                if previous.elapsed() >= LOCK_DEBOUNCE {
                    *last = Some(Instant::now());
                    true
                } else {
                    false
                }
            }
            None => {
                *last = Some(Instant::now());
                true
            }
        }
    };
    if !should_fire {
        return;
    }

    if let Some(locked) = read_led_state(LOCK_SUFFIX[index]) {
        key.state.store(locked, Ordering::SeqCst);
        notify_lock(index, locked, APP.get());
    }
}

fn on_key(press: KeyPress) {
    let app = APP.get();
    if let Some(app) = app {
        if crate::system::shortcuts::check_match_and_trigger(
            app,
            press.vk,
            press.ctrl,
            press.alt,
            press.shift,
            press.win,
        ) {
            return;
        }
    }
    if lock_index(press.vk).is_some() {
        handle_lock_key(press);
    }
}

/// Seeds the lock states from the kernel and starts the shortcut/OSD path.
pub fn init_hooks(app: Option<&AppHandle>) {
    if let Some(app) = app {
        let _ = APP.set(app.clone());
    }
    crate::system::shortcuts::init_shortcuts(app);

    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    if let Some(locked) = read_led_state("::capslock") {
        CAPS_STATE.store(locked, Ordering::SeqCst);
    }
    if let Some(locked) = read_led_state("::numlock") {
        NUM_STATE.store(locked, Ordering::SeqCst);
    }
    if let Some(locked) = read_led_state("::scrolllock") {
        SCROLL_STATE.store(locked, Ordering::SeqCst);
    }

    if evdev::spawn_monitor(on_key) {
        crate::log_info!(
            "HOOKS",
            "evdev keyboard layer active; Win-key suppression still owned by the compositor"
        );
    } else {
        crate::log_warn!(
            "HOOKS",
            "no readable keyboard under /dev/input; {}",
            evdev::INPUT_GROUP_HINT
        );
    }
}

/// Live key state for a virtual-key code, backed by the evdev reader.
pub fn is_key_down(vk: u16) -> bool {
    evdev::is_key_down(vk)
}
