//! Power-management platform layer.
//!
//! Two primitives live here because they are the only pieces of the "keep the
//! machine awake" story that differ per OS:
//!
//! * [`set_execution_state`] — pin / release a system-wide power lock.
//! * [`last_input_tick`] — monotonic tick of the last user input, used to detect
//!   peripheral interaction while the panel is powered down.
//!
//! Windows maps both onto `SetThreadExecutionState` / `GetLastInputInfo`.
//! Linux maps the lock onto a logind inhibitor (via `systemd-inhibit`, which is
//! the freedesktop-standard way to ask logind for one and works on every
//! desktop) and reads the idle tick from the evdev reader (`pal::evdev`), since
//! neither Wayland nor logind exposes a cross-desktop input timestamp.

/// What the watcher thread wants the machine to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExecutionState {
    /// Release every lock: normal sleep and display idle timers apply again.
    Normal,
    /// Keep display and system awake (user pressed "keep awake").
    KeepAwake,
    /// Keep the system awake but allow the panel to stay off.
    ScreenOffAway,
}

impl ExecutionState {
    /// Human readable label for logs.
    pub fn label(self) -> &'static str {
        match self {
            ExecutionState::Normal => "normal",
            ExecutionState::KeepAwake => "keep-awake",
            ExecutionState::ScreenOffAway => "screen-off-away",
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::ExecutionState;

    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_AWAYMODE_REQUIRED: u32 = 0x0000_0040;

    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
        fn GetTickCount() -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    }

    pub fn set_execution_state(state: ExecutionState) {
        let flags = match state {
            ExecutionState::Normal => ES_CONTINUOUS,
            ExecutionState::KeepAwake => {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
            }
            ExecutionState::ScreenOffAway => {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED
            }
        };
        unsafe {
            SetThreadExecutionState(flags);
        }
    }

    pub fn last_input_tick() -> Option<u32> {
        let mut lii = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        let ok = unsafe { GetLastInputInfo(&mut lii) };
        if ok == 0 {
            return None;
        }
        let now = unsafe { GetTickCount() };
        Some(now.wrapping_sub(lii.dw_time))
    }
}

#[cfg(not(windows))]
mod portable_impl {
    use super::ExecutionState;
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;
    use std::time::Instant;

    /// The inhibitor child (`systemd-inhibit ... sleep infinity`) currently held.
    struct Inhibitor {
        child: Child,
        state: ExecutionState,
    }

    static INHIBITOR: Mutex<Option<Inhibitor>> = Mutex::new(None);

    /// Kept as the idle reference origin for callers that need one.
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

    fn what_for(state: ExecutionState) -> &'static str {
        match state {
            ExecutionState::Normal => "idle",
            ExecutionState::KeepAwake => "idle:sleep",
            ExecutionState::ScreenOffAway => "idle:sleep:handle-lid-switch",
        }
    }

    fn systemd_inhibit_available() -> bool {
        Command::new("systemd-inhibit")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn release_locked(slot: &mut Option<Inhibitor>) {
        if let Some(mut held) = slot.take() {
            let _ = held.child.kill();
            let _ = held.child.wait();
        }
    }

    pub fn set_execution_state(state: ExecutionState) {
        let mut slot = INHIBITOR.lock().unwrap_or_else(|e| e.into_inner());
        if state == ExecutionState::Normal {
            release_locked(&mut slot);
            return;
        }
        if slot.as_ref().map(|held| held.state) == Some(state) {
            return;
        }
        release_locked(&mut slot);
        if !systemd_inhibit_available() {
            crate::log_warn!(
                "POWER_STATE",
                "systemd-inhibit not available; cannot hold '{}' lock",
                state.label()
            );
            return;
        }
        let child = Command::new("systemd-inhibit")
            .args([
                "--what",
                what_for(state),
                "--who",
                "OpenRevo",
                "--why",
                "Keep-awake requested from the control center",
                "--mode",
                "block",
                "sleep",
                "infinity",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => {
                crate::log_info!(
                    "POWER_STATE",
                    "Power lock acquired: {} ({}), inhibited via logind",
                    state.label(),
                    what_for(state)
                );
                *slot = Some(Inhibitor { child, state });
            }
            Err(err) => crate::log_warn!(
                "POWER_STATE",
                "Failed to spawn systemd-inhibit for '{}': {}",
                state.label(),
                err
            ),
        }
    }

    pub fn last_input_tick() -> Option<u32> {
        // Fed by the evdev reader (`pal::evdev`), which pumps /dev/input/event*
        // and stamps every key, button and motion event. No compositor or
        // display server is consulted, so it behaves the same on Wayland and
        // X11. Returns "unknown" only while no keyboard was opened (wrong
        // group, or no keyboard attached).
        let start = START.get_or_init(Instant::now);
        let _ = start;
        crate::pal::evdev::idle_ms().map(|idle| idle.min(u32::MAX as u64) as u32)
    }
}

#[cfg(windows)]
pub use windows_impl::{last_input_tick, set_execution_state};

#[cfg(not(windows))]
pub use portable_impl::{last_input_tick, set_execution_state};
