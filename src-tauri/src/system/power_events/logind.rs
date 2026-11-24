//! Linux power-event source.
//!
//! Two logind interfaces drive the shared pipelines:
//!
//! * `org.freedesktop.login1.Manager.PrepareForSleep(bool)` — `true` before the
//!   system suspends, `false` when it comes back. systemd-logind broadcasts it
//!   to every interested process and it is desktop independent.
//! * `org.freedesktop.login1.Session.LidClosed` — the lid switch, the same
//!   information `WM_POWERBROADCAST` delivers on Windows. logind exposes it as
//!   a property, so it is polled at a low rate instead of pushed.
//!
//! Shutdown comes from `PrepareForShutdown`.

use std::time::Duration;

use dbus::blocking::Connection;
use dbus::message::MatchRule;

const LOGIND_SERVICE: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
const LID_POLL_TICKS: u32 = 4;
const TICK: Duration = Duration::from_millis(500);

pub(super) fn start() -> bool {
    let conn = match Connection::new_system() {
        Ok(conn) => conn,
        Err(err) => {
            crate::log_warn!(
                "POWER_STATE",
                "logind unavailable ({}); sleep and lid events disabled",
                err
            );
            return false;
        }
    };

    let sleep_rule = MatchRule::new_signal(LOGIND_MANAGER, "PrepareForSleep");
    if let Err(err) = conn.add_match::<(bool,), _>(sleep_rule, |(going_to_sleep,), _conn, _msg| {
        if going_to_sleep {
            super::enter_suspend("PrepareForSleep");
        } else {
            super::resume_pipeline("resumed from Sleep / Hibernate");
        }
        true
    }) {
        crate::log_warn!("POWER_STATE", "Cannot subscribe to PrepareForSleep: {}", err);
    }

    let shutdown_rule = MatchRule::new_signal(LOGIND_MANAGER, "PrepareForShutdown");
    if let Err(err) =
        conn.add_match::<(bool,), _>(shutdown_rule, |(going_down,), _conn, _msg| {
            if going_down {
                super::system_shutdown();
            }
            true
        })
    {
        crate::log_warn!(
            "POWER_STATE",
            "Cannot subscribe to PrepareForShutdown: {}",
            err
        );
    }

    println!("[Power Guard] logind power event listener running.");

    let mut last_lid: Option<bool> = None;
    let mut tick: u32 = 0;
    loop {
        if let Err(err) = conn.process(TICK) {
            crate::log_error!("POWER_STATE", "logind connection dropped: {}", err);
            return false;
        }

        tick = tick.wrapping_add(1);
        if tick % LID_POLL_TICKS != 0 {
            continue;
        }
        let lid = read_lid_closed();
        match (last_lid, lid) {
            (_, None) => {}
            (None, Some(closed)) => {
                // Record the baseline only: never fire a false resume on startup.
                println!(
                    "[Power Guard] Initial lid state registered: {} (startup baseline set)",
                    if closed { "Closed" } else { "Open" }
                );
                last_lid = Some(closed);
            }
            (Some(prev), Some(closed)) if prev != closed => {
                last_lid = Some(closed);
                if closed {
                    super::lid_closed();
                } else {
                    super::resume_pipeline("lid opened");
                }
            }
            _ => {}
        }
    }
}

/// Read the lid switch straight from ACPI.
///
/// `/proc/acpi/button/lid/*/state` is what every desktop reads it from as well;
/// going through logind would need the per-session object path, and there is no
/// guarantee this process is owned by an interactive session.
fn read_lid_closed() -> Option<bool> {
    let entries = std::fs::read_dir("/proc/acpi/button/lid").ok()?;
    for entry in entries.flatten() {
        let state_path = entry.path().join("state");
        let Ok(state) = std::fs::read_to_string(&state_path) else {
            continue;
        };
        let state = state.trim();
        if state.ends_with("closed") {
            return Some(true);
        }
        if state.ends_with("open") {
            return Some(false);
        }
    }
    None
}
