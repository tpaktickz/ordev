//! Passive evdev input reader.
//!
//! Wayland exposes no global keyboard state to a regular client — no
//! `WH_KEYBOARD_LL`, no `XQueryKeymap`. Reading `/dev/input/event*` directly is
//! the compositor-independent way to get system-wide key edges and a real idle
//! clock, and it works identically under KWin, Mutter, Hyprland and wlroots.
//!
//! The reader is passive: it never calls `EVIOCGRAB`, so the compositor keeps
//! receiving every key. Devices are discovered through `/sys/class/input`, kept
//! for their lifetime, and re-scanned periodically so hot-plugged keyboards are
//! picked up.
//!
//! Membership in the `input` group is required (`/dev/input/event*` is `root:input
//! 0660`); without it the reader logs once and everything reports unknown.

/// A keyboard key press with the modifier state that was active at that moment.
#[derive(Clone, Copy, Debug)]
pub struct KeyPress {
    pub vk: u16,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// Hint printed when `/dev/input` cannot be opened.
pub const INPUT_GROUP_HINT: &str = "add the user to the 'input' group (usermod -aG input $USER, then re-login)";

#[cfg(target_os = "linux")]
mod linux {
    use super::{KeyPress, INPUT_GROUP_HINT};
    use crate::pal::keymap::{self, Modifier};
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// `struct input_event` on 64-bit Linux: timeval(16) + type(2) + code(2) + value(4).
    #[cfg(test)]
    #[repr(C)]
    struct LayoutProbe {
        tv_sec: i64,
        tv_usec: i64,
        type_: u16,
        code: u16,
        value: i32,
    }

    #[cfg(test)]
    mod layout_tests {
        #[test]
        fn input_event_matches_kernel_layout() {
            assert_eq!(std::mem::size_of::<super::LayoutProbe>(), super::EVENT_SIZE);
        }
    }

    const EVENT_SIZE: usize = 24;
    const EV_SYN: u16 = 0;
    const EV_KEY: u16 = 1;
    const EV_REL: u16 = 2;
    const EV_ABS: u16 = 3;

    const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

    struct State {
        start: Instant,
        last_input_ms: AtomicU64,
        ctrl: AtomicBool,
        alt: AtomicBool,
        shift: AtomicBool,
        win: AtomicBool,
        pressed: Mutex<Vec<u16>>,
        running: AtomicBool,
        denied: AtomicBool,
    }

    static STATE: OnceLock<State> = OnceLock::new();

    fn state() -> &'static State {
        STATE.get_or_init(|| State {
            start: Instant::now(),
            last_input_ms: AtomicU64::new(0),
            ctrl: AtomicBool::new(false),
            alt: AtomicBool::new(false),
            shift: AtomicBool::new(false),
            win: AtomicBool::new(false),
            pressed: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            denied: AtomicBool::new(false),
        })
    }

    fn touch() {
        let s = state();
        s.last_input_ms
            .store(s.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Milliseconds since the last input event, or `None` when nothing is read.
    pub fn idle_ms() -> Option<u64> {
        let s = state();
        if !s.running.load(Ordering::Relaxed) {
            return None;
        }
        let last = s.last_input_ms.load(Ordering::Relaxed);
        let now = s.start.elapsed().as_millis() as u64;
        Some(now.saturating_sub(last))
    }

    pub fn is_key_down(vk: u16) -> bool {
        state()
            .pressed
            .lock()
            .map(|set| set.contains(&vk))
            .unwrap_or(false)
    }

    pub fn is_running() -> bool {
        state().running.load(Ordering::Relaxed)
    }

    /// True when the device advertises a full keyboard (ESC and 'A' both present).
    fn is_keyboard(event_dir: &Path) -> bool {
        let caps = match fs::read_to_string(event_dir.join("device/capabilities/key")) {
            Ok(text) => text,
            Err(_) => return false,
        };
        let hex: String = caps.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if hex.is_empty() {
            return false;
        }
        // LSB-first: the last hex digit holds bits 0..3.
        let bit = |index: usize| -> bool {
            let nibble_from_end = index / 4;
            if nibble_from_end >= hex.len() {
                return false;
            }
            let digit = hex.as_bytes()[hex.len() - 1 - nibble_from_end];
            let value = (digit as char).to_digit(16).unwrap_or(0);
            value & (1 << (index % 4)) != 0
        };
        bit(1) && bit(30)
    }

    /// Every `/dev/input/event*` node backed by something that looks like a keyboard.
    fn keyboard_nodes() -> Vec<(PathBuf, PathBuf)> {
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir("/sys/class/input") else {
            return found;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("event") {
                continue;
            }
            if !is_keyboard(&entry.path()) {
                continue;
            }
            found.push((entry.path(), PathBuf::from("/dev/input").join(&name)));
        }
        found
    }

    fn with_modifiers(press: &mut KeyPress) {
        let s = state();
        press.ctrl = s.ctrl.load(Ordering::Relaxed);
        press.alt = s.alt.load(Ordering::Relaxed);
        press.shift = s.shift.load(Ordering::Relaxed);
        press.win = s.win.load(Ordering::Relaxed);
    }

    fn set_modifier(modifier: Modifier, down: bool) {
        let s = state();
        let flag = match modifier {
            Modifier::Ctrl => &s.ctrl,
            Modifier::Alt => &s.alt,
            Modifier::Shift => &s.shift,
            Modifier::Win => &s.win,
        };
        flag.store(down, Ordering::Relaxed);
    }

    fn note_pressed(vk: u16, down: bool) {
        if let Ok(mut set) = state().pressed.lock() {
            if down {
                if !set.contains(&vk) {
                    set.push(vk);
                }
            } else {
                set.retain(|held| *held != vk);
            }
        }
    }

    fn handle_event<F: Fn(KeyPress)>(raw_type: u16, code: u16, value: i32, on_press: &F) {
        match raw_type {
            EV_KEY => {
                if let Some(modifier) = keymap::modifier_of(code) {
                    set_modifier(modifier, value != 0);
                    touch();
                    return;
                }
                let Some(vk) = keymap::vk_from_evdev(code) else {
                    touch();
                    return;
                };
                if value == 2 {
                    // Autorepeat: state already held, no edge to report.
                    touch();
                    return;
                }
                let down = value == 1;
                note_pressed(vk, down);
                touch();
                if down {
                    let mut press = KeyPress {
                        vk,
                        ctrl: false,
                        alt: false,
                        shift: false,
                        win: false,
                    };
                    with_modifiers(&mut press);
                    on_press(press);
                }
            }
            EV_REL | EV_ABS | EV_SYN => touch(),
            _ => {}
        }
    }

    fn pump<F: Fn(KeyPress) + Send + Sync + 'static>(
        path: PathBuf,
        on_press: Arc<F>,
        seen: Arc<Mutex<Vec<PathBuf>>>,
    ) {
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(err) => {
                let s = state();
                if err.kind() == std::io::ErrorKind::PermissionDenied
                    && !s.denied.swap(true, Ordering::Relaxed)
                {
                    crate::log_warn!(
                        "EVDEV",
                        "cannot open {} ({}); {}", path.display(), err, INPUT_GROUP_HINT
                    );
                }
                if let Ok(mut set) = seen.lock() {
                    set.retain(|kept| kept != &path);
                }
                return;
            }
        };

        let mut buf = [0u8; EVENT_SIZE];
        loop {
            match file.read(&mut buf) {
                Ok(EVENT_SIZE) => {
                    let raw_type = u16::from_le_bytes([buf[16], buf[17]]);
                    let code = u16::from_le_bytes([buf[18], buf[19]]);
                    let value = i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
                    handle_event(raw_type, code, value, &*on_press);
                }
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }

        if let Ok(mut set) = seen.lock() {
            set.retain(|kept| kept != &path);
        }
    }

    /// Starts the reader. Returns `true` when at least one keyboard was opened.
    pub fn spawn_monitor<F>(on_press: F) -> bool
    where
        F: Fn(KeyPress) + Send + Sync + 'static,
    {
        let s = state();
        if s.running.swap(true, Ordering::SeqCst) {
            return true;
        }
        touch();

        let on_press = Arc::new(on_press);
        let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let first = keyboard_nodes();
        let opened = {
            let mut count = 0usize;
            for (_sys, dev) in &first {
                if let Ok(mut set) = seen.lock() {
                    set.push(dev.clone());
                }
                let path = dev.clone();
                let handler = Arc::clone(&on_press);
                let registry = Arc::clone(&seen);
                std::thread::Builder::new()
                    .name(format!("evdev-{}", dev.display()))
                    .spawn(move || pump(path, handler, registry))
                    .ok();
                count += 1;
            }
            count
        };

        // Supervisor: pick up keyboards plugged in after startup.
        std::thread::Builder::new()
            .name("evdev-rescan".to_string())
            .spawn(move || loop {
                std::thread::sleep(RESCAN_INTERVAL);
                for (_sys, dev) in keyboard_nodes() {
                    let already = seen.lock().map(|set| set.contains(&dev)).unwrap_or(false);
                    if already {
                        continue;
                    }
                    if let Ok(mut set) = seen.lock() {
                        set.push(dev.clone());
                    }
                    let handler = Arc::clone(&on_press);
                    let registry = Arc::clone(&seen);
                    let path = dev.clone();
                    std::thread::Builder::new()
                        .name(format!("evdev-{}", dev.display()))
                        .spawn(move || pump(path, handler, registry))
                        .ok();
                }
            })
            .ok();

        opened > 0
    }
}

#[cfg(target_os = "linux")]
pub use linux::{idle_ms, is_key_down, is_running, spawn_monitor};

#[cfg(not(target_os = "linux"))]
mod portable {
    use super::KeyPress;

    /// Windows answers through `GetAsyncKeyState` / `GetLastInputInfo` instead.
    pub fn spawn_monitor<F>(_on_press: F) -> bool
    where
        F: Fn(KeyPress) + Send + Sync + 'static,
    {
        false
    }

    pub fn idle_ms() -> Option<u64> {
        None
    }

    pub fn is_key_down(_vk: u16) -> bool {
        false
    }

    pub fn is_running() -> bool {
        false
    }
}

#[cfg(not(target_os = "linux"))]
pub use portable::{idle_ms, is_key_down, is_running, spawn_monitor};
