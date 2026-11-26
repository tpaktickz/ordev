//! Linux system tray: a freedesktop StatusNotifierItem host.
//!
//! The `win32_tray` facade owns the menu model and the actions; this file only
//! renders that model as an SNI menu so it works on any shell that implements
//! `org.kde.StatusNotifierItem` (KDE, GNOME with the appindicator extension,
//! XFCE, Cinnamon, sway/wlroots bars, …).
//!
//! Two rules make this host responsive, and both are easy to get wrong:
//!
//! 1. Menu handlers (`activate`, `select`) must never touch the tray model.
//!    ksni calls them inside `update_immediately`, which already holds the
//!    model lock, so `Handle::update` from a handler deadlocks a `std::sync`
//!    mutex on itself. Plasma then times out on `GetLayout` (25 s) and the
//!    right-click menu looks frozen. Work is therefore handed to a thread.
//! 2. `menu()` and `icon_pixmap()` run on the DBus thread and must not wait for
//!    the EC poller: one mailbox write is 60 ms and would stall the shell.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem};
use ksni::TrayService;
use ksni::{Category, Icon, MenuItem, Tray};

use super::{TrayAction, GLOBAL_EC_BRIDGE};
use crate::driver::ec_bridge::SharedEcBridge;

/// Handle of the published item, so background refreshes can reach it.
static HANDLE: OnceLock<ksni::Handle<OpenRevoTray>> = OnceLock::new();

/// Last rendered state. `menu()` reads only these atomics.
static SNAP_MODE: AtomicU8 = AtomicU8::new(2);
static SNAP_FAN: AtomicBool = AtomicBool::new(false);
static SNAP_EXPANDED: AtomicBool = AtomicBool::new(true);

/// Application icon shipped with the package, embedded so the tray still shows
/// the product icon on a machine where `make install` never ran.
const ICON_32: &[u8] = include_bytes!("../../icons/32x32.png");
const ICON_64: &[u8] = include_bytes!("../../icons/64x64.png");

pub struct OpenRevoTray;

impl OpenRevoTray {
    fn new() -> Self {
        Self
    }
}

/// Runs one action off the DBus thread, then republishes the menu state.
fn spawn_action(action: TrayAction) {
    std::thread::spawn(move || {
        super::perform(action);
        if action == TrayAction::Quit {
            // `perform` shuts the subsystems down; the process exits after it.
            std::process::exit(0);
        }
        refresh_snapshot();
        publish();
    });
}

/// Re-reads the state the menu renders. Never blocks: a contended EC lock or a
/// poller mid-write only leaves the previous snapshot in place.
fn refresh_snapshot() {
    if let Some(ec) = GLOBAL_EC_BRIDGE.get() {
        if let Some(guard) = ec.try_lock() {
            if let Some((mode, _)) = guard.try_menu_state() {
                if mode > 0 {
                    SNAP_MODE.store(mode, Ordering::Relaxed);
                }
            }
        }
    }
    let cfg = crate::system::config::load_config();
    SNAP_FAN.store(cfg.fan_boost, Ordering::Relaxed);
    SNAP_EXPANDED.store(cfg.last_window_expanded, Ordering::Relaxed);
}

/// Repaints the menu if any rendered value changed.
fn publish() {
    let Some(handle) = HANDLE.get() else { return };
    let _ = handle.update(|_| {});
}

/// Repaint request from the rest of the app (Slint panel, hotkeys, physical
/// Q-key). Cheap to call from any thread, including a menu handler.
pub(super) fn refresh() {
    std::thread::spawn(|| {
        refresh_snapshot();
        publish();
    });
}

fn argb_icons() -> &'static [(i32, i32, Vec<u8>)] {
    static ICONS: OnceLock<Vec<(i32, i32, Vec<u8>)>> = OnceLock::new();
    ICONS.get_or_init(|| {
        let mut out = Vec::new();
        for png in [ICON_32, ICON_64] {
            // A decode failure only costs the picture, never the tray item.
            let Ok(image) = image::load_from_memory(png) else {
                continue;
            };
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            let mut data = Vec::with_capacity(rgba.len());
            // StatusNotifierItem wants ARGB32, network byte order.
            for pixel in rgba.pixels() {
                data.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
            }
            out.push((width as i32, height as i32, data));
        }
        out
    })
}

/// Theme name of the icon, but only when the package actually installed it —
/// otherwise an unknown name makes some shells draw nothing at all.
fn installed_icon_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        for dir in openrevo_core::pal::xdg::data_dirs() {
            for size in ["48x48", "64x64", "128x128", "scalable"] {
                if dir.join("icons/hicolor").join(size).join("apps/open-revo.png").exists()
                    || dir.join("icons/hicolor").join(size).join("apps/open-revo.svg").exists()
                {
                    return "open-revo";
                }
            }
        }
        ""
    })
}

fn item(label: impl Into<String>, enabled: bool, action: TrayAction) -> MenuItem<OpenRevoTray> {
    MenuItem::Standard(StandardItem {
        label: label.into(),
        enabled,
        activate: Box::new(move |_tray: &mut OpenRevoTray| spawn_action(action)),
        ..Default::default()
    })
}

fn check(label: impl Into<String>, checked: bool, action: TrayAction) -> MenuItem<OpenRevoTray> {
    MenuItem::Checkmark(CheckmarkItem {
        label: label.into(),
        checked,
        activate: Box::new(move |_tray: &mut OpenRevoTray| spawn_action(action)),
        ..Default::default()
    })
}

/// Radio options carry their action through the group's `select` index, so a
/// bare label is all an option needs.
fn radio(label: &str) -> RadioItem {
    RadioItem {
        label: label.into(),
        ..Default::default()
    }
}

/// A radio group: exactly one option stays marked, which a list of independent
/// check marks cannot express.
fn radio_group(
    labels: &[&str],
    selected: usize,
    actions: Vec<TrayAction>,
) -> MenuItem<OpenRevoTray> {
    MenuItem::RadioGroup(RadioGroup {
        selected,
        select: Box::new(move |_tray: &mut OpenRevoTray, index: usize| {
            if let Some(action) = actions.get(index) {
                spawn_action(*action);
            }
        }),
        options: labels.iter().map(|label| radio(label)).collect(),
    })
}

impl Tray for OpenRevoTray {
    fn id(&self) -> String {
        "open-revo".into()
    }

    fn title(&self) -> String {
        "OpenRevo 控制中枢".into()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn icon_name(&self) -> String {
        installed_icon_name().to_string()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        argb_icons()
            .iter()
            .map(|(width, height, data)| Icon {
                width: *width,
                height: *height,
                data: data.clone(),
            })
            .collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // ksni runs this inside `update_immediately` with the model locked, and
        // waking the panel spawns the Slint companion process (seconds on a
        // cold start). Hand it off so the shell never waits on us.
        std::thread::spawn(crate::system::slint_window::show_slint_window);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mode = SNAP_MODE.load(Ordering::Relaxed);
        let fan = SNAP_FAN.load(Ordering::Relaxed);
        let expanded = SNAP_EXPANDED.load(Ordering::Relaxed);
        let mode_index = match mode {
            1 => 0,
            3 => 2,
            4 => 3,
            _ => 1,
        };
        vec![
            radio_group(
                &["🖥️ 完整面板", "🪟 迷你抽屉"],
                if expanded { 0 } else { 1 },
                vec![TrayAction::ShowFullPanel, TrayAction::ShowMiniDrawer],
            ),
            item("🌙 关闭屏幕 (3秒防误触)", true, TrayAction::TurnOffScreen),
            MenuItem::Separator,
            radio_group(
                &[
                    "办公模式 (Office)",
                    "均衡模式 (Balance)",
                    "狂暴模式 (Beast)",
                    "极客调校 (Custom)",
                ],
                mode_index,
                vec![
                    TrayAction::SetPowerMode(1),
                    TrayAction::SetPowerMode(2),
                    TrayAction::SetPowerMode(3),
                    TrayAction::SetPowerMode(4),
                ],
            ),
            MenuItem::Separator,
            check("一键强冷 (Fan Boost)", fan, TrayAction::ToggleFanBoost),
            item(
                "🔄 接管/恢复 官方后台服务",
                true,
                TrayAction::ToggleOemTakeover,
            ),
            MenuItem::Separator,
            item(
                format!("🛡️ PRO v{}", env!("CARGO_PKG_VERSION")),
                false,
                TrayAction::ShowFullPanel,
            ),
            item("退出 OpenRevo", true, TrayAction::Quit),
        ]
    }
}

/// Runs the StatusNotifierItem host on the calling thread (blocking until the
/// service stops, mirroring the Win32 message loop contract).
pub fn run_tray_loop(ec_instance: SharedEcBridge) {
    let _ = GLOBAL_EC_BRIDGE.set(ec_instance);

    // Prime the snapshot before the first `menu()` so the marks are right from
    // the start instead of after the first poll.
    refresh_snapshot();
    SNAP_MODE.store(
        GLOBAL_EC_BRIDGE
            .get()
            .and_then(|ec| ec.try_lock().and_then(|guard| guard.try_menu_state()))
            .map(|(mode, _)| mode)
            .filter(|mode| *mode > 0)
            .unwrap_or(2),
        Ordering::Relaxed,
    );

    let service = TrayService::new(OpenRevoTray::new());
    let handle = service.handle();
    let _ = HANDLE.set(handle.clone());

    // Hardware state also changes behind our back (a physical Q-key, the Slint
    // panel, the EC poller). Poll the snapshot instead of blocking `menu()`.
    std::thread::spawn(|| {
        let mut last = (0u8, false, false);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            refresh_snapshot();
            let now = (
                SNAP_MODE.load(Ordering::Relaxed),
                SNAP_FAN.load(Ordering::Relaxed),
                SNAP_EXPANDED.load(Ordering::Relaxed),
            );
            if now != last {
                last = now;
                publish();
            }
        }
    });

    // StatusNotifierItem is the freedesktop standard, but the host side is not:
    // KDE and XFCE ship a watcher, GNOME only shows items through an extension.
    // Say so once instead of letting the user wonder where the icon went.
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop.to_ascii_uppercase().contains("GNOME") {
        crate::log_warn!(
            "TRAY",
            "GNOME has no StatusNotifierItem host by default; install the AppIndicator extension or start OpenRevo with --visible"
        );
    }
    crate::log_info!("TRAY", "Publishing StatusNotifierItem (desktop={})", desktop);

    match service.run() {
        Ok(()) => crate::log_info!("TRAY", "StatusNotifierItem host stopped"),
        Err(err) => crate::log_error!("TRAY", "StatusNotifierItem host failed: {}", err),
    }
    crate::remove_core_pid_file();
}
