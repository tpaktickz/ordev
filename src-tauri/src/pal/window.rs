//! Native window handle bridge for the Tauri webview windows.
//!
//! The Windows side needs the real `HWND` for focus and hit-testing checks. On
//! Wayland an application has no compositor window id it may rely on; the
//! callers only use the handle as "is this ours", so Linux returns a marker that
//! is stable for the lifetime of the process and unique per label.

/// Opaque, process-local handle used by the platform-neutral call sites.
#[cfg(windows)]
pub fn native_handle(app: &tauri::AppHandle, label: &str) -> Option<isize> {
    use tauri::Manager;
    app.get_webview_window(label)
        .and_then(|win| win.hwnd().ok())
        .map(|h| h.0 as isize)
}

/// Opaque, process-local handle used by the platform-neutral call sites.
///
/// There is no `HWND` equivalent, and the Wayland protocol deliberately withholds
/// the compositor's window identity. A stable hash of the label is enough for the
/// "does the focused window belong to us" checks: it never matches a foreign id.
#[cfg(not(windows))]
pub fn native_handle(app: &tauri::AppHandle, label: &str) -> Option<isize> {
    use std::hash::{Hash, Hasher};
    use tauri::Manager;

    app.get_webview_window(label)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "openrevo-native-window".hash(&mut hasher);
    label.hash(&mut hasher);
    // Keep it negative so it can never collide with a real X11/xdg window id.
    Some(-((hasher.finish() & 0x7fff_ffff) as isize) - 1)
}
