//! Embedding the plugin webview into the dashboard.
//!
//! On Windows the plugin WebView2 is re-parented with `SetParent` and becomes a
//! real child of the Slint window, so it can be clipped to the plugin area and
//! follow every move of the shell for free.
//!
//! Wayland has no cross-process child windows: the protocol gives an application
//! its own surfaces only. The Linux implementation therefore keeps the plugin
//! host as a separate always-on-top-free toplevel and places it at the plugin
//! area's offset, computed from the same dashboard proportions. Move and resize
//! of the dashboard re-apply that position through the same entry point.

#[cfg(windows)]
#[path = "webview_dock_win.rs"]
mod win;
#[cfg(windows)]
pub use win::*;

pub const EMBEDDED_LABEL: &str = "embedded_plugin_host";

/// Design-space geometry of the plugin area inside the dashboard: sidebar 200px,
/// header + action strip 79px, 1px seam margin.
const PLUGIN_AREA_X: f64 = 200.0;
const PLUGIN_AREA_Y: f64 = 79.0;
const PLUGIN_AREA_RADIUS: f64 = 9.0;
const DASHBOARD_W: f64 = 1120.0;
const DASHBOARD_H: f64 = 760.0;

#[cfg(not(windows))]
pub type NativeHandle = isize;

/// Handle of the Slint dashboard window, if it is up.
#[cfg(windows)]
pub fn parent_handle() -> Option<NativeHandle> {
    let raw = super::dialog::get_openrevo_main_hwnd();
    if raw == 0 {
        None
    } else {
        Some(windows::Win32::Foundation::HWND(raw as *mut _))
    }
}

/// Handle of the Slint dashboard window, if it is up.
#[cfg(not(windows))]
pub fn parent_handle() -> Option<NativeHandle> {
    // Nothing on Wayland identifies the Slint toplevel; a live marker is enough
    // for every caller to know docking is possible.
    Some(1)
}

/// Raw integer form of a handle, for arg passing and the embedder.
#[cfg(windows)]
pub fn handle_raw(handle: NativeHandle) -> isize {
    handle.0 as isize
}

/// Raw integer form of a handle, for arg passing and the embedder.
#[cfg(not(windows))]
pub fn handle_raw(handle: NativeHandle) -> isize {
    handle
}

#[cfg(not(windows))]
pub fn attach_or_switch_embedded_webview(app: &tauri::AppHandle, plugin_id: &str) {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    crate::log_info!(
        "EMBEDDED_WEBVIEW",
        "attach_or_switch_embedded_webview requested for plugin: '{}'",
        plugin_id
    );

    let (target_x, target_y, target_w, target_h, _) = calc_embedded_geometry(1);

    let init_script = format!(
        "window.__OPENREVO_WIDGET_ID__ = '{0}';",
        plugin_id
    );

    if let Some(win) = app.get_webview_window(EMBEDDED_LABEL) {
        let script = format!(
            "window.__OPENREVO_WIDGET_ID__ = '{0}'; window.location.hash = '#/widget/{0}'; window.dispatchEvent(new CustomEvent('openrevo-switch-widget', {{ detail: {{ pluginId: '{0}' }} }}));",
            plugin_id
        );
        let _ = win.eval(&script);
        let _ = win.set_size(tauri::PhysicalSize::new(target_w as u32, target_h as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(target_x, target_y));
        let _ = win.show();
        return;
    }

    let builder = WebviewWindowBuilder::new(app, EMBEDDED_LABEL, WebviewUrl::App("index.html".into()))
        .title("OpenRevo Embedded Plugin")
        .inner_size(target_w as f64, target_h as f64)
        .position(target_x as f64, target_y as f64)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .skip_taskbar(true)
        .initialization_script(&init_script)
        .visible(false);

    match builder.build() {
        Ok(win) => {
            let _ = win.set_size(tauri::PhysicalSize::new(target_w as u32, target_h as u32));
            let _ = win.show();
            crate::log_info!(
                "EMBEDDED_WEBVIEW",
                "Embedded webview placed at {},{},{}x{} for '{}'",
                target_x,
                target_y,
                target_w,
                target_h,
                plugin_id
            );
        }
        Err(err) => {
            crate::log_error!(
                "EMBEDDED_WEBVIEW",
                "Failed to build embedded WebviewWindow for '{}': {:?}",
                plugin_id,
                err
            );
        }
    }
}

#[cfg(not(windows))]
pub fn hide_embedded_webview(app: &tauri::AppHandle) {
    use tauri::Manager;

    crate::log_info!("EMBEDDED_WEBVIEW", "hide_embedded_webview called");
    if let Some(win) = app.get_webview_window(EMBEDDED_LABEL) {
        let _ = win.hide();
    }
}

/// Plugin area rectangle for a dashboard of `scale` (x, y, w, h, corner radius).
///
/// The proportions are the dashboard's own design grid; a scaled dashboard keeps
/// the area attached to the same grid cell.
#[cfg(not(windows))]
pub fn calc_embedded_geometry(_parent: NativeHandle) -> (i32, i32, i32, i32, i32) {
    let scale = 1.0;
    let total_w = (DASHBOARD_W * scale).round() as i32;
    let total_h = (DASHBOARD_H * scale).round() as i32;
    let target_x = (PLUGIN_AREA_X * scale).round() as i32;
    let target_y = (PLUGIN_AREA_Y * scale).round() as i32;
    let target_w = (total_w - target_x - 1).max(100);
    let target_h = (total_h - target_y - 1).max(100);
    let radius = (PLUGIN_AREA_RADIUS * scale).round() as i32;
    (target_x, target_y, target_w, target_h, radius)
}
