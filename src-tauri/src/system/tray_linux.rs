//! Linux system tray: a freedesktop StatusNotifierItem host.
//!
//! The `win32_tray` facade owns the menu model and the actions; this file only
//! renders that model as an SNI menu so it works on any shell that implements
//! `org.kde.StatusNotifierItem` (KDE, GNOME with the appindicator extension,
//! XFCE, Cinnamon, sway/wlroots bars, …).

use std::sync::Mutex;

use ksni::{Category, MenuItem, Tray, TrayService};

use super::{TrayAction, GLOBAL_EC_BRIDGE};
use crate::driver::ec_bridge::SharedEcBridge;

pub struct OpenRevoTray {
    handle: Mutex<Option<ksni::Handle<OpenRevoTray>>>,
}

impl OpenRevoTray {
    fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    /// Push the state change produced by an action back into the shell menu.
    fn after(&self, action: TrayAction) {
        super::perform(action);
        if action == TrayAction::Quit {
            std::process::exit(0);
        }
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(handle) = handle {
            handle.update(|_| {});
        }
    }

    fn icon(&self) -> String {
        // Prefer the packaged application icon when it is installed
        // (`p5` installs it into the hicolor tree); otherwise fall back to a
        // stock name that every theme carries so the item is never blank.
        for dir in openrevo_core::pal::xdg::data_dirs() {
            let candidate = dir
                .join("icons/hicolor/48x48/apps/open-revo.png");
            if candidate.exists() {
                return "open-revo".into();
            }
        }
        "computer".into()
    }
}

fn item(label: &str, enabled: bool, action: TrayAction) -> MenuItem<OpenRevoTray> {
    MenuItem::Standard(ksni::menu::StandardItem {
        label: label.into(),
        enabled,
        activate: Box::new(move |tray: &mut OpenRevoTray| tray.after(action)),
        ..Default::default()
    })
}

fn check(label: &str, checked: bool, action: TrayAction) -> MenuItem<OpenRevoTray> {
    MenuItem::Checkmark(ksni::menu::CheckmarkItem {
        label: label.into(),
        checked,
        activate: Box::new(move |tray: &mut OpenRevoTray| tray.after(action)),
        ..Default::default()
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
        self.icon()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        crate::system::slint_window::show_slint_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let (current_mode, current_fan, last_window_expanded) = super::current_menu_state();
        vec![
            check("🖥️ 完整面板", last_window_expanded, TrayAction::ShowFullPanel),
            check("🪟 迷你抽屉", !last_window_expanded, TrayAction::ShowMiniDrawer),
            item("🌙 关闭屏幕 (3秒防误触)", true, TrayAction::TurnOffScreen),
            MenuItem::Separator,
            check("办公模式 (Office)", current_mode == 1, TrayAction::SetPowerMode(1)),
            check("均衡模式 (Balance)", current_mode == 2, TrayAction::SetPowerMode(2)),
            check("狂暴模式 (Beast)", current_mode == 3, TrayAction::SetPowerMode(3)),
            check("极客调校 (Custom)", current_mode == 4, TrayAction::SetPowerMode(4)),
            MenuItem::Separator,
            check("一键强冷 (Fan Boost)", current_fan, TrayAction::ToggleFanBoost),
            item(
                "🔄 接管/恢复 官方后台服务",
                true,
                TrayAction::ToggleOemTakeover,
            ),
            MenuItem::Separator,
            item(
                &format!("🛡️ PRO v{}", env!("CARGO_PKG_VERSION")),
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

    let service = TrayService::new(OpenRevoTray::new());
    let handle = service.handle();
    {
        // Hand the handle to the tray so menu actions can refresh it.
        let for_tray = handle.clone();
        handle.update(move |tray| {
            *tray.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(for_tray.clone());
        });
    }
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
