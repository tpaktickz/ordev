//! Linux window plumbing.
//!
//! There is no HWND here: the compositor owns geometry. Calls into `slint::Window`
//! are best effort — a Wayland compositor may ignore `set_position` for tiled
//! surfaces, and X11 honours it. The handle type stays an opaque token so the
//! shared call sites (`hwnd.0`, `SafeHwnd::to_hwnd`) compile unchanged.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use crate::MainWindow;

/// Opaque window token. Linux has no pointer to carry; 0 means "not resolved".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinHandle(pub isize);

#[derive(Clone, Copy, Debug)]
pub struct SafeHwnd(pub isize);

impl SafeHwnd {
    pub fn to_hwnd(&self) -> WinHandle {
        WinHandle(self.0)
    }
}

pub(crate) static WAS_MINIMIZED: AtomicBool = AtomicBool::new(false);
pub(crate) static SLINT_UI_WEAK: Mutex<Option<slint::Weak<MainWindow>>> = Mutex::new(None);
pub static HWND_HOLDER: Mutex<Option<SafeHwnd>> = Mutex::new(None);
pub static SAVED_FULL_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);
pub(crate) static LAST_REDRAW_INSTANT: Mutex<Option<Instant>> = Mutex::new(None);

pub static FOCUS_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static LAST_WAKE_TIME: Mutex<Option<Instant>> = Mutex::new(None);
pub static LAST_MOVE_TIME: Mutex<Option<Instant>> = Mutex::new(None);
pub static HOST_PID: AtomicU32 = AtomicU32::new(0);

pub fn set_host_pid(pid: u32) {
    HOST_PID.store(pid, Ordering::SeqCst);
}

pub fn mark_window_woken() {
    FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst);
    *LAST_WAKE_TIME.lock().unwrap() = Some(Instant::now());
}

pub fn mark_window_focused() {
    FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst);
}

pub fn mark_window_moved() {
    *LAST_MOVE_TIME.lock().unwrap() = Some(Instant::now());
}

pub fn set_slint_ui_weak(weak: slint::Weak<MainWindow>) {
    *SLINT_UI_WEAK.lock().unwrap() = Some(weak);
}

fn with_ui<R>(f: impl FnOnce(&MainWindow) -> R) -> Option<R> {
    let ui = SLINT_UI_WEAK.lock().ok()?.as_ref()?.upgrade()?;
    Some(f(&ui))
}

/// Resolve the main window. The window exists as soon as Slint created it.
pub fn find_thread_hwnd() -> Option<WinHandle> {
    let alive = SLINT_UI_WEAK
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|w| w.upgrade()))
        .is_some();
    if alive {
        Some(WinHandle(0))
    } else {
        None
    }
}

/// No DWM/EXSTYLE layer on Linux; the compositor draws decorations.
pub fn apply_window_decorations(_hwnd: WinHandle) {}

pub fn hide_window(_hwnd: WinHandle) {
    let _ = with_ui(|ui| ui.window().hide());
}

pub fn maximize_window(_hwnd: WinHandle) {
    let _ = with_ui(|ui| ui.window().set_maximized(true));
}

pub fn minimize_window(_hwnd: WinHandle) {
    WAS_MINIMIZED.store(true, Ordering::SeqCst);
    let _ = with_ui(|ui| ui.window().set_minimized(true));
}

pub fn restore_window(_hwnd: WinHandle) {
    let _ = with_ui(|ui| {
        ui.window().set_minimized(false);
        ui.window().set_maximized(false);
    });
}

/// Wayland cannot raise a window from the client side; mapping is all we get.
pub fn present_window(hwnd: WinHandle) {
    show_window(hwnd);
    trigger_full_redraw(hwnd);
}

pub fn is_window_present(_hwnd: WinHandle) -> bool {
    with_ui(|ui| ui.window().is_visible() && !ui.window().is_minimized()).unwrap_or(false)
}

pub fn window_position(_hwnd: WinHandle) -> Option<(i32, i32)> {
    with_ui(|ui| ui.window().position()).map(|p| (p.x, p.y))
}

pub fn show_window(_hwnd: WinHandle) {
    let _ = with_ui(|ui| ui.window().show());
}

pub fn get_window_scale_factor(_hwnd: WinHandle) -> f64 {
    if let Some(scale) = with_ui(|ui| ui.window().scale_factor()) {
        if scale > 0.1 {
            return scale as f64;
        }
    }
    1.0
}

/// Primary screen extent from DRM, used to place the drawer bottom-right.
/// Work area (panel/taskbar subtraction) is not exposed through a cross-desktop
/// API, so a fixed margin stands in for it.
fn primary_screen_size() -> (i32, i32) {
    let mut best = (0i32, 0i32);
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.join("status").exists() {
                continue;
            }
            let status = std::fs::read_to_string(path.join("status")).unwrap_or_default();
            if status.trim() != "connected" {
                continue;
            }
            if let Ok(modes) = std::fs::read_to_string(path.join("modes")) {
                for line in modes.lines() {
                    let mut it = line.split('x');
                    let w = it.next().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
                    let h = it.next().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
                    if w * h > best.0 * best.1 {
                        best = (w, h);
                    }
                }
            }
        }
    }
    if best.0 == 0 || best.1 == 0 {
        (1920, 1080)
    } else {
        best
    }
}

pub fn align_mini_drawer_to_tray(_hwnd: WinHandle, target_logical_w: i32, target_logical_h: i32) {
    let scale = get_window_scale_factor(_hwnd);
    let w = (target_logical_w as f64 * scale).round() as i32;
    let h = (target_logical_h as f64 * scale).round() as i32;
    let (screen_w, screen_h) = primary_screen_size();
    let margin = (16.0 * scale).round() as i32;

    let _ = with_ui(|ui| {
        let window = ui.window();
        window.set_size(slint::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
        window.set_position(slint::PhysicalPosition::new(
            (screen_w - w - margin).max(0),
            (screen_h - h - margin).max(0),
        ));
    });
}

pub fn expand_native_window(
    _hwnd: WinHandle,
    target_logical_w: i32,
    target_logical_h: i32,
    saved_pos: Option<(i32, i32)>,
) {
    let scale = get_window_scale_factor(_hwnd);
    let w = (target_logical_w as f64 * scale).round() as i32;
    let h = (target_logical_h as f64 * scale).round() as i32;
    let (screen_w, screen_h) = primary_screen_size();

    let (x, y) = match saved_pos {
        Some((sx, sy)) => {
            let overlap_x = (sx + w).min(screen_w) - sx.max(0);
            let overlap_y = (sy + h).min(screen_h) - sy.max(0);
            if overlap_x >= 200 && overlap_y >= 100 {
                (sx, sy)
            } else {
                ((screen_w - w) / 2, (screen_h - h) / 2)
            }
        }
        None => ((screen_w - w) / 2, (screen_h - h) / 2),
    };

    let _ = with_ui(|ui| {
        let window = ui.window();
        window.set_size(slint::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
        window.set_position(slint::PhysicalPosition::new(x.max(0), y.max(0)));
    });
}

fn do_redraw_pass(_hwnd: WinHandle) {
    let _ = with_ui(|ui| {
        ui.set_redraw_tick(ui.get_redraw_tick().wrapping_add(1));
        ui.window().request_redraw();
    });
}

pub fn trigger_full_redraw(hwnd: WinHandle) {
    {
        let mut last = LAST_REDRAW_INSTANT.lock().unwrap();
        let now = Instant::now();
        if let Some(prev) = *last {
            if now.duration_since(prev) < Duration::from_millis(200) {
                return;
            }
        }
        *last = Some(now);
    }
    do_redraw_pass(hwnd);
}

/// Auto-collapse hook. The Win32 version fires from the subclassed window
/// procedure; here the caller decides when blur happened (see `ipc::system`),
/// so the debounce runs unchanged.
pub fn handle_window_blur(_hwnd: WinHandle) {
    let (auto_hide, is_expanded) = with_ui(|ui| (ui.get_mini_auto_hide(), ui.get_is_expanded()))
        .unwrap_or((false, false));

    if is_expanded || !auto_hide {
        return;
    }

    let current_gen = FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(160));

        if FOCUS_GENERATION.load(Ordering::SeqCst) != current_gen {
            return;
        }
        if LAST_WAKE_TIME.lock().unwrap().map_or(false, |t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        if LAST_MOVE_TIME.lock().unwrap().map_or(false, |t| t.elapsed() < Duration::from_millis(400)) {
            return;
        }
        let is_expanded_now = with_ui(|ui| ui.get_is_expanded()).unwrap_or(false);
        if is_expanded_now {
            return;
        }
        let visible = with_ui(|ui| ui.window().is_visible()).unwrap_or(false);
        if !visible {
            return;
        }

        hide_window(WinHandle(0));
        crate::ipc::send_action_to_host("record_window_mode", serde_json::json!({ "is_expanded": false }));
        crate::ipc::send_action_to_host("slint_window_hidden", serde_json::json!({}));
        std::thread::sleep(Duration::from_millis(30));
        std::process::exit(0);
    });
}
