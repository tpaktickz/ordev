use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use slint::{Color, ComponentHandle, RgbaColor};
pub use slint;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::GetCurrentThreadId;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Controls::MARGINS;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetParent, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, GWL_EXSTYLE, GWL_STYLE, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

pub mod ipc;

#[cfg(target_os = "linux")]
pub mod layer_shell;

#[cfg(target_os = "linux")]
static LAYER_POSITION: Mutex<String> = Mutex::new(String::new());

/// Sticky OSD position, needed by the layer surface: anchoring is applied once
/// the window exists and again on every change, and the value has to survive
/// until then.
#[cfg(target_os = "linux")]
pub fn current_position() -> String {
    let pos = LAYER_POSITION.lock().map(|p| p.clone()).unwrap_or_default();
    if pos.is_empty() { "top-left".to_string() } else { pos }
}

#[cfg(target_os = "linux")]
fn remember_position(position: &str) {
    if let Ok(mut p) = LAYER_POSITION.lock() {
        *p = position.to_string();
    }
}

#[cfg(target_os = "linux")]
fn osd_log(msg: &str) {
    use std::io::Write;
    let dir = openrevo_core::pal::xdg::cache_home().join("openrevo");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("osd.log"))
    {
        let _ = writeln!(
            f,
            "[{}] {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            msg
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn osd_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("C:\\Users\\faint\\.openrevo_osd.log") {
        let _ = writeln!(f, "[{}] {}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(), msg);
    }
}


slint::include_modules!();

#[derive(Debug, Clone)]
pub struct NativeOsdConfig {
    pub enabled: bool,
    pub theme: String,    // "pill" | "fade" | "cinema"
    pub scale: String,    // "80%" | "100%" | "120%" | "140%"
    pub position: String, // "top-left" | "bottom-center" | "center"
}

impl Default for NativeOsdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            theme: "pill".into(),
            scale: "100%".into(),
            position: "top-left".into(),
        }
    }
}

pub fn parse_scale_factor(scale_str: &str) -> f32 {
    match scale_str {
        "80%" => 0.8,
        "120%" => 1.2,
        "140%" => 1.4,
        _ => 1.0,
    }
}

/// Window handle. On Windows this is literally `HWND`; on Linux there is no
/// handle to carry, so an opaque token stands in and every placement call goes
/// through the Slint window instead.
#[cfg(target_os = "windows")]
pub type WinHandle = HWND;

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct WinHandle(pub isize);

/// Send & Sync wrapper around a window handle
#[derive(Debug, Clone, Copy)]
pub struct SafeHwnd(pub isize);

unsafe impl Send for SafeHwnd {}
unsafe impl Sync for SafeHwnd {}

impl SafeHwnd {
    pub fn from_hwnd(hwnd: WinHandle) -> Self {
        #[cfg(target_os = "windows")]
        {
            Self(hwnd.0 as isize)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self(hwnd.0)
        }
    }

    pub fn as_hwnd(&self) -> WinHandle {
        #[cfg(target_os = "windows")]
        {
            HWND(self.0 as *mut _)
        }
        #[cfg(not(target_os = "windows"))]
        {
            WinHandle(self.0)
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_thread_wnd_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if let Ok(parent) = GetParent(hwnd) {
        if !parent.0.is_null() {
            return BOOL(1);
        }
    }

    let mut rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rect);
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;

    osd_log(&format!("enum_thread_wnd: hwnd={:?}, w={}, h={}", hwnd.0, w, h));

    // Top-level Slint OsdWindow has non-zero dimensions
    if w >= 100 && h >= 10 {
        let target = lparam.0 as *mut HWND;
        *target = hwnd;
        return BOOL(0); // Found target window, stop enumeration
    }
    BOOL(1) // Continue enumeration
}

#[cfg(target_os = "windows")]
fn find_thread_hwnd() -> Option<HWND> {
    let mut found_hwnd = HWND(std::ptr::null_mut());
    unsafe {
        let thread_id = GetCurrentThreadId();
        let _ = EnumThreadWindows(
            thread_id,
            Some(enum_thread_wnd_proc),
            LPARAM(&mut found_hwnd as *mut HWND as isize),
        );
    }
    if found_hwnd.0.is_null() {
        None
    } else {
        Some(found_hwnd)
    }
}

#[cfg(target_os = "windows")]
fn get_window_dpi_scale(hwnd: HWND) -> f32 {
    #[link(name = "user32")]
    extern "system" {
        fn GetDpiForWindow(hwnd: HWND) -> u32;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi > 0 {
        dpi as f32 / 96.0
    } else {
        1.0
    }
}

#[cfg(target_os = "windows")]
fn get_monitor_bounds(hwnd: HWND) -> (i32, i32, i32, i32) {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if !hmon.is_invalid() {
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                let r = mi.rcMonitor;
                return (r.left, r.top, r.right - r.left, r.bottom - r.top);
            }
        }
        (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
    }
}

fn calculate_osd_position(hwnd: WinHandle, position_str: &str, win_width: i32, win_height: i32) -> (i32, i32) {
    let dpi_scale = get_window_dpi_scale(hwnd);
    let (mon_x, mon_y, screen_w, screen_h) = get_monitor_bounds(hwnd);

    let w = if win_width > 0 { win_width } else { (640.0 * dpi_scale).round() as i32 };
    let h = if win_height > 0 { win_height } else { (130.0 * dpi_scale).round() as i32 };

    let margin_x = (36.0 * dpi_scale).round() as i32;
    let margin_top = (36.0 * dpi_scale).round() as i32;
    let margin_bottom = (70.0 * dpi_scale).round() as i32;

    let (rel_x, rel_y) = match position_str {
        "top-center" => ((screen_w - w) / 2, margin_top),
        "top-right" | "right" => (screen_w - w - margin_x, margin_top),
        "bottom-center" | "bottom" => ((screen_w - w) / 2, screen_h - h - margin_bottom),
        "bottom-left" => (margin_x, screen_h - h - margin_bottom),
        "center" => ((screen_w - w) / 2, (screen_h - h) / 2),
        _ => (margin_x, margin_top), // Default: "top-left"
    };

    (mon_x + rel_x, mon_y + rel_y)
}

#[cfg(target_os = "windows")]
fn reposition_osd_window(hwnd: HWND, position_str: &str) {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let win_width = rect.right - rect.left;
        let win_height = rect.bottom - rect.top;

        let (x, y) = calculate_osd_position(hwnd, position_str, win_width, win_height);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(target_os = "windows")]
pub fn apply_osd_window_styles(hwnd: HWND, position_str: &str) {
    unsafe {
        // Guarantee window remains hidden while undergoing style mutations
        let _ = ShowWindow(hwnd, SW_HIDE);

        // Clear title string so Windows DWM titlebar renderer has nothing to display
        let empty_title: [u16; 1] = [0];
        let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(empty_title.as_ptr()));

        // Remove caption/border decorations without WS_VISIBLE to prevent flash
        SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize);

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_ex_style = (ex_style
            | (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize)
            & !(0x00040000);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex_style);

        let corner_pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const _,
            std::mem::size_of_val(&corner_pref) as u32,
        );

        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let win_width = rect.right - rect.left;
        let win_height = rect.bottom - rect.top;

        let (x, y) = calculate_osd_position(hwnd, position_str, win_width, win_height);

        // Crucial: SWP_FRAMECHANGED forces Windows DWM to discard and recalculate non-client caption bar!
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );

        let _ = ShowWindow(hwnd, SW_HIDE);
        osd_log(&format!("apply_osd_window_styles applied at ({}, {}) for hwnd={:?}", x, y, hwnd.0));
    }
}


#[cfg(not(target_os = "windows"))]
fn with_osd_ui<R>(f: impl FnOnce(&OsdWindow) -> R) -> Option<R> {
    let state = OSD_STATE.get()?;
    let ui = state.window.upgrade()?;
    Some(f(&ui))
}

#[cfg(not(target_os = "windows"))]
fn find_thread_hwnd() -> Option<WinHandle> {
    with_osd_ui(|_| ()).map(|_| WinHandle(0))
}

#[cfg(not(target_os = "windows"))]
fn get_window_dpi_scale(_hwnd: WinHandle) -> f32 {
    with_osd_ui(|ui| ui.window().scale_factor())
        .filter(|s| *s > 0.1)
        .unwrap_or(1.0)
}

/// Primary screen extent. Work area is not exposed by a cross-desktop API.
#[cfg(not(target_os = "windows"))]
fn get_monitor_bounds(_hwnd: WinHandle) -> (i32, i32, i32, i32) {
    let mut best = (0i32, 0i32);
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.join("status").exists() {
                continue;
            }
            if std::fs::read_to_string(path.join("status")).unwrap_or_default().trim() != "connected" {
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
    let (w, h) = if best.0 == 0 || best.1 == 0 { (1920, 1080) } else { best };
    (0, 0, w, h)
}

/// Wayland compositors may ignore this for tiled surfaces; X11 obeys it.
#[cfg(not(target_os = "windows"))]
fn reposition_osd_window(_hwnd: WinHandle, position_str: &str) {
    let size = with_osd_ui(|ui| ui.window().size()).unwrap_or_default();
    let (x, y) = calculate_osd_position(
        WinHandle(0),
        position_str,
        size.width as i32,
        size.height as i32,
    );
    let _ = with_osd_ui(|ui| {
        ui.window()
            .set_position(slint::PhysicalPosition::new(x.max(0), y.max(0)));
    });
}

/// Linux has no DWM/WM styles to mutate: the OSD is a frameless Slint window and
/// the compositor owns decoration and stacking. Placement is still applied.
#[cfg(not(target_os = "windows"))]
pub fn apply_osd_window_styles(hwnd: WinHandle, position_str: &str) {
    reposition_osd_window(hwnd, position_str);
    osd_log(&format!("apply_osd_window_styles (linux) at pos={}", position_str));
}

fn hide_window(hwnd: SafeHwnd) {
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = ShowWindow(hwnd.as_hwnd(), SW_HIDE);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        // A layer surface that is hidden loses its buffer, and the next reveal would
        // start from a blank frame. The card animates itself out of view instead, so
        // the surface stays mapped and simply shows nothing.
        if layer_shell::active() {
            osd_log("hide_window: layer surface stays mapped");
            return;
        }
        let _ = with_osd_ui(|ui| ui.window().hide());
    }
}

fn show_window_no_activate(hwnd: SafeHwnd) {
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = ShowWindow(hwnd.as_hwnd(), SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd.as_hwnd(),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        if layer_shell::active() {
            let _ = with_osd_ui(|ui| ui.window().show());
            return;
        }
        let _ = with_osd_ui(|ui| ui.window().show());
    }
}

#[derive(Debug, Clone)]
pub struct OsdPalette {
    pub office: Color,
    pub balance: Color,
    pub beast: Color,
    pub geek: Color,
    pub fan: Color,
    pub enabled: Color,
    pub disabled: Color,
    pub neutral: Color,
}

impl Default for OsdPalette {
    fn default() -> Self {
        Self {
            office: Color::from(RgbaColor { red: 16, green: 185, blue: 129, alpha: 255 }),
            balance: Color::from(RgbaColor { red: 6, green: 182, blue: 212, alpha: 255 }),
            beast: Color::from(RgbaColor { red: 239, green: 68, blue: 68, alpha: 255 }),
            geek: Color::from(RgbaColor { red: 168, green: 85, blue: 247, alpha: 255 }),
            fan: Color::from(RgbaColor { red: 56, green: 189, blue: 248, alpha: 255 }),
            enabled: Color::from(RgbaColor { red: 16, green: 185, blue: 129, alpha: 255 }),
            disabled: Color::from(RgbaColor { red: 239, green: 68, blue: 68, alpha: 255 }),
            neutral: Color::from(RgbaColor { red: 148, green: 163, blue: 184, alpha: 255 }),
        }
    }
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    let s = hex.trim().strip_prefix('#')?;
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::from_rgb_u8(r, g, b))
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = u8::from_str_radix(&s[6..8], 16).ok()?;
        Some(Color::from_argb_u8(a, r, g, b))
    } else {
        None
    }
}

fn with_alpha(c: Color, alpha: u8) -> Color {
    let rgba = c.to_argb_u8();
    Color::from(RgbaColor { red: rgba.red, green: rgba.green, blue: rgba.blue, alpha })
}

fn current_palette() -> OsdPalette {
    OSD_STATE
        .get()
        .and_then(|s| s.palette.lock().ok().map(|p| p.clone()))
        .unwrap_or_default()
}

#[allow(dead_code)]
struct OsdGlobalState {
    window: slint::Weak<OsdWindow>,
    hwnd: Arc<Mutex<Option<SafeHwnd>>>,
    config: Arc<Mutex<NativeOsdConfig>>,
    palette: Arc<Mutex<OsdPalette>>,
}

static OSD_STATE: OnceLock<OsdGlobalState> = OnceLock::new();

static LAST_ACTIVITY: Mutex<Option<std::time::Instant>> = Mutex::new(None);

pub fn touch_activity() {
    if let Ok(mut g) = LAST_ACTIVITY.lock() {
        *g = Some(std::time::Instant::now());
    }
}

fn reveal_osd_window(hwnd_opt: Option<SafeHwnd>) {
    let safe_hwnd = hwnd_opt.or_else(|| {
        if let Some(hwnd) = find_thread_hwnd() {
            let pos = OSD_STATE
                .get()
                .and_then(|s| s.config.lock().ok().map(|c| c.position.clone()))
                .unwrap_or_else(|| "top-left".to_string());
            apply_osd_window_styles(hwnd, &pos);
            let s = SafeHwnd::from_hwnd(hwnd);
            if let Some(state) = OSD_STATE.get() {
                if let Ok(mut g) = state.hwnd.lock() {
                    *g = Some(s);
                }
            }
            osd_log(&format!("reveal_osd_window: captured and primed HWND on demand: {:?}", hwnd.0));
            Some(s)
        } else {
            None
        }
    });

    if let Some(safe_hwnd) = safe_hwnd {
        let pos = OSD_STATE
            .get()
            .and_then(|s| s.config.lock().ok().map(|c| c.position.clone()))
            .unwrap_or_else(|| "top-left".to_string());
        reposition_osd_window(safe_hwnd.as_hwnd(), &pos);
        show_window_no_activate(safe_hwnd);
        osd_log(&format!("reveal_osd_window: Shown HWND {:?}", safe_hwnd.0));
    } else {
        osd_log("reveal_osd_window: WARNING safe_hwnd is None");
    }
}

static HIDE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn handle_ipc_message(
    raw: &str,
    ui_weak: &slint::Weak<OsdWindow>,
    hwnd_shared: &Arc<Mutex<Option<SafeHwnd>>>,
) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        osd_log(&format!("handle_ipc_message: invalid JSON: {}", raw));
        return;
    };

    let action = val.get("action").and_then(|a| a.as_str()).unwrap_or("");
    osd_log(&format!("handle_ipc_message: action = {}", action));
    touch_activity();
    let weak = ui_weak.clone();
    let hwnd_cloned = hwnd_shared.clone();

    match action {
        "mode" => {
            let mode = val.get("value").and_then(|v| v.as_u64()).unwrap_or(1) as u8;
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_power_mode_internal(&ui, mode);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "fan" => {
            let enabled = val.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_fan_boost_internal(&ui, enabled);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "preview" => {
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_power_mode_internal(&ui, 3);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "key_lock" => {
            let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let locked = val.get("locked").and_then(|l| l.as_bool()).unwrap_or(false);
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_key_lock_toast_internal(&ui, &name, locked);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "device_switch" => {
            let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let enabled = val.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_device_switch_internal(&ui, &name, enabled);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "kb_brightness" => {
            let level = val.get("level").and_then(|l| l.as_u64()).unwrap_or(0) as u8;
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_kb_brightness_internal(&ui, level);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }
        "kb_sleep" => {
            let seconds = val.get("seconds").and_then(|s| s.as_u64()).unwrap_or(0) as u32;
            let gen = HIDE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let hwnd_snap = *hwnd_cloned.lock().unwrap();
                    reveal_osd_window(hwnd_snap);
                    show_kb_sleep_internal(&ui, seconds);
                    schedule_auto_hide_internal(&ui, hwnd_cloned.clone(), gen);
                }
            });
        }

        "config" => {
            let theme = val.get("theme").and_then(|t| t.as_str()).unwrap_or("pill").to_string();
            let scale = val.get("scale").and_then(|s| s.as_str()).unwrap_or("100%").to_string();
            let position = val.get("position").and_then(|p| p.as_str()).unwrap_or("top-left").to_string();
            let scale_factor = parse_scale_factor(&scale);
            osd_log(&format!("handle_ipc_message config: theme={}, scale={}, pos={}", theme, scale, position));
            if let Some(state) = OSD_STATE.get() {
                let mut cfg = state.config.lock().unwrap();
                cfg.theme = theme.clone();
                cfg.scale = scale.clone();
                cfg.position = position.clone();
            }
            #[cfg(target_os = "linux")]
            {
                remember_position(&position);
                layer_shell::apply_position(&position);
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_osd_theme(theme.into());
                    ui.set_osd_scale(scale_factor);
                    ui.set_osd_position(position.clone().into());
                    if let Some(safe_hwnd) = *hwnd_cloned.lock().unwrap() {
                        apply_osd_window_styles(safe_hwnd.as_hwnd(), &position);
                    }
                }
            });
        }
        "theme_tokens" => {
            if let Some(tokens) = val.get("tokens").cloned() {
                if let Some(state) = OSD_STATE.get() {
                    if let Ok(mut pal) = state.palette.lock() {
                        if let Some(c) = tokens.get("mode_office").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.office = c;
                            pal.enabled = c;
                        }
                        if let Some(c) = tokens.get("mode_balance").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.balance = c;
                        }
                        if let Some(c) = tokens.get("mode_beast").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.beast = c;
                            pal.disabled = c;
                        }
                        if let Some(c) = tokens.get("accent_brand").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.geek = c;
                        }
                        if let Some(c) = tokens.get("accent_fan").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.fan = c;
                        }
                        if let Some(c) = tokens.get("text_muted").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            pal.neutral = c;
                        }
                    }
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        let default_font = "Microsoft YaHei UI, Microsoft YaHei, Segoe UI, sans-serif";
                        if let Some(ff) = tokens.get("font_family").and_then(|v| v.as_str()) {
                            let trimmed = ff.trim();
                            if trimmed.is_empty() {
                                ui.set_font_family(default_font.into());
                            } else if !trimmed.contains("Microsoft YaHei") {
                                ui.set_font_family(format!("{}, {}", trimmed, default_font).into());
                            } else {
                                ui.set_font_family(trimmed.into());
                            }
                        } else {
                            ui.set_font_family(default_font.into());
                        }
                        if let Some(c) = tokens.get("surface_elevated").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            let rgba = c.to_argb_u8();
                            let r = rgba.red;
                            let g = rgba.green;
                            let b = rgba.blue;
                            let base_alpha = if rgba.alpha == 255 { 242 } else { rgba.alpha };
                            let mid_alpha = (base_alpha as f32 * 0.45) as u8;

                            ui.set_card_bg_color(Color::from(RgbaColor { red: r, green: g, blue: b, alpha: base_alpha }));
                            ui.set_card_bg_mid(Color::from(RgbaColor { red: r, green: g, blue: b, alpha: mid_alpha }));
                            ui.set_card_bg_end(Color::from(RgbaColor { red: r, green: g, blue: b, alpha: 0 }));
                        }
                        if let Some(c) = tokens.get("text_primary").and_then(|v| v.as_str()).and_then(parse_hex_color) {
                            ui.set_text_primary_color(c);
                        }
                        if let Some(r) = tokens.get("radius_sm").and_then(|v| v.as_f64()) {
                            ui.set_custom_radius_sm(r as f32);
                        }
                        if let Some(r) = tokens.get("radius_md").and_then(|v| v.as_f64()) {
                            ui.set_custom_radius_md(r as f32);
                        }
                    }
                });
            }
        }
        "quit" | "shutdown" | "exit" => {
            std::process::exit(0);
        }
        _ => {}
    }
}



fn show_power_mode_internal(ui: &OsdWindow, mode: u8) {
    let pal = current_palette();
    let (title, icon, color) = match mode {
        1 => ("办公模式", "🌱", pal.office),
        2 => ("均衡模式", "⚖️", pal.balance),
        3 => ("狂暴模式", "⚡", pal.beast),
        _ => ("极客调校", "💧", pal.geek),
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol(icon.into());
    ui.set_accent_color(color);
    ui.set_accent_bg(with_alpha(color, 35));
    ui.set_is_active(true);
}

fn show_fan_boost_internal(ui: &OsdWindow, enabled: bool) {
    let pal = current_palette();
    let (title, icon, color) = if enabled {
        ("强冷模式 开启", "🌪️", pal.fan)
    } else {
        ("强冷模式 关闭", "💨", pal.neutral)
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol(icon.into());
    ui.set_accent_color(color);
    ui.set_accent_bg(with_alpha(color, 35));
    ui.set_is_active(true);
}

fn show_key_lock_toast_internal(ui: &OsdWindow, name: &str, locked: bool) {
    let pal = current_palette();
    let (title, icon, color) = match name {
        "win" => {
            if locked {
                ("Win 键锁定 开启", "🔒", pal.disabled)
            } else {
                ("Win 键锁定 关闭", "🔓", pal.enabled)
            }
        }
        "fn" => {
            if locked {
                ("Fn 键锁定 开启", "⚡", pal.balance)
            } else {
                ("Fn 键锁定 关闭", "⌨️", pal.neutral)
            }
        }
        "touchpad" => {
            if locked {
                ("触摸板 禁用", "🚫", pal.disabled)
            } else {
                ("触摸板 启用", "🖱️", pal.enabled)
            }
        }
        "caps" => {
            if locked {
                ("大写锁定 开启", "🔠", pal.fan)
            } else {
                ("大写锁定 关闭", "🔡", pal.neutral)
            }
        }
        "num" => {
            if locked {
                ("数字锁定 开启", "🔢", pal.fan)
            } else {
                ("数字锁定 关闭", "⌨️", pal.neutral)
            }
        }
        "scroll" => {
            if locked {
                ("滚动锁定 开启", "📜", pal.fan)
            } else {
                ("滚动锁定 关闭", "📄", pal.neutral)
            }
        }
        _ => {
            if locked {
                ("锁定已生效", "🔒", pal.disabled)
            } else {
                ("解锁已生效", "🔓", pal.enabled)
            }
        }
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol(icon.into());
    ui.set_accent_color(color);
    ui.set_accent_bg(with_alpha(color, 30));
    ui.set_is_active(true);
}

fn show_device_switch_internal(ui: &OsdWindow, name: &str, enabled: bool) {
    let pal = current_palette();
    let (title, icon, color) = match name {
        "camera" => {
            if enabled {
                ("摄像头 开启", "📷", pal.enabled)
            } else {
                ("摄像头 关闭", "🚫", pal.disabled)
            }
        }
        "mic" => {
            if enabled {
                ("麦克风 开启", "🎙️", pal.enabled)
            } else {
                ("麦克风 静音", "🔇", pal.disabled)
            }
        }
        "wifi" => {
            if enabled {
                ("Wi-Fi 无线网 开启", "📶", pal.fan)
            } else {
                ("Wi-Fi 无线网 关闭", "✈️", pal.disabled)
            }
        }
        "bluetooth" => {
            if enabled {
                ("蓝牙 开启", "ᛒ", pal.fan)
            } else {
                ("蓝牙 关闭", "📵", pal.disabled)
            }
        }
        "keep_awake" => {
            if enabled {
                ("始终保持唤醒 开启", "☕", pal.fan)
            } else {
                ("始终保持唤醒 关闭", "💤", pal.neutral)
            }
        }
        "screen_flip" => {
            if enabled {
                ("屏幕已翻转 (180°)", "🔄", pal.fan)
            } else {
                ("屏幕方向已恢复", "🖥️", pal.enabled)
            }
        }
        _ => {
            if enabled {
                ("功能已开启", "✅", pal.enabled)
            } else {
                ("功能已关闭", "❌", pal.disabled)
            }
        }
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol(icon.into());
    ui.set_accent_color(color);
    ui.set_accent_bg(with_alpha(color, 30));
    ui.set_is_active(true);
}

fn show_kb_brightness_internal(ui: &OsdWindow, level: u8) {
    let pal = current_palette();
    let (title, icon, color) = if level == 0 {
        ("键盘背光 关闭".to_string(), "💡", pal.neutral)
    } else {
        let pct = match level {
            1 => 25,
            2 => 50,
            3 => 75,
            _ => 100,
        };
        (format!("键盘背光 {}%", pct), "💡", pal.fan)
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol(icon.into());
    ui.set_accent_color(color);
    ui.set_accent_bg(with_alpha(color, 30));
    ui.set_is_active(true);
}

fn show_kb_sleep_internal(ui: &OsdWindow, seconds: u32) {
    let pal = current_palette();
    let title = if seconds == 0 {
        "键盘背光 常亮".to_string()
    } else {
        format!("背光 {}秒熄灭", seconds)
    };

    ui.set_mode_title(title.into());
    ui.set_icon_symbol("⏳".into());
    ui.set_accent_color(pal.fan);
    ui.set_accent_bg(with_alpha(pal.fan, 35));
    ui.set_is_active(true);
}

fn schedule_auto_hide_internal(ui: &OsdWindow, hwnd_shared: Arc<Mutex<Option<SafeHwnd>>>, generation: u64) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1800), move || {
        if HIDE_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation {
            return; // Superseded by a newer OSD event, abort hiding
        }
        if let Some(target) = weak.upgrade() {
            target.set_is_active(false);

            let hide_hwnd = hwnd_shared.clone();
            slint::Timer::single_shot(Duration::from_millis(260), move || {
                if HIDE_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation {
                    return; // Superseded by a newer OSD event, keep shown
                }
                if let Some(safe_hwnd) = *hide_hwnd.lock().unwrap() {
                    hide_window(safe_hwnd);
                    osd_log("schedule_auto_hide: window hidden");
                }
            });
        }
    });
}


/// Standalone entrypoint for --osd-companion process mode.
/// Runs exclusively on the true Main Thread without conflicting with foreign event loops.
pub fn run_osd_standalone() {
    #[cfg(target_os = "windows")]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        }
        const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    std::panic::set_hook(Box::new(|info| {
        osd_log(&format!("FATAL PANIC in OSD Companion: {}", info));
    }));

    osd_log("run_osd_standalone entered");

    // A Wayland layer surface has to exist before the component does: whether the
    // window is a plain toplevel or an overlay is decided by which platform Slint
    // registers, and the platform cannot be swapped afterwards.
    #[cfg(target_os = "linux")]
    if layer_shell::install() {
        osd_log("OSD hosted in a wl_surface layer (overlay)");
    } else {
        osd_log("OSD hosted in a plain toplevel window");
    }

    let osd = match OsdWindow::new() {
        Ok(w) => w,
        Err(e) => {
            osd_log(&format!("FATAL: Failed to create Slint OsdWindow: {}", e));
            return;
        }
    };

    // Same identity as the panel process: without it a fallback X11 toplevel
    // OSD would show a generic taskbar icon.
    #[cfg(target_os = "linux")]
    {
        let _ = slint::set_xdg_app_id("org.openrevo.OpenRevo");
    }

    let osd_weak = osd.as_weak();
    let hwnd_holder = Arc::new(Mutex::new(None));
    let cfg_shared = Arc::new(Mutex::new(NativeOsdConfig::default()));
    let palette_shared = Arc::new(Mutex::new(OsdPalette::default()));

    let _ = OSD_STATE.set(OsdGlobalState {
        window: osd_weak.clone(),
        hwnd: hwnd_holder.clone(),
        config: cfg_shared.clone(),
        palette: palette_shared,
    });

    // Start stdin IPC listener
    ipc::start_ipc_server(osd_weak.clone(), hwnd_holder.clone());

    touch_activity();

    // Show window once so winit actually creates the HWND
    let _ = osd.show();
    osd_log("Called osd.show() once to force HWND creation");

    let (pos, theme, scale) = {
        let c = cfg_shared.lock().unwrap();
        (c.position.clone(), c.theme.clone(), c.scale.clone())
    };
    osd.set_osd_theme(theme.clone().into());
    osd.set_osd_scale(parse_scale_factor(&scale));
    osd.set_osd_position(pos.clone().into());

    let applied_once = Arc::new(Mutex::new(false));

    // Synchronously capture and prime HWND immediately on the main thread
    if let Some(hwnd) = find_thread_hwnd() {
        apply_osd_window_styles(hwnd, &pos);
        hide_window(SafeHwnd::from_hwnd(hwnd));
        *hwnd_holder.lock().unwrap() = Some(SafeHwnd::from_hwnd(hwnd));
        *applied_once.lock().unwrap() = true;
        osd_log(&format!("Synchronously captured & primed HWND {:?}", hwnd.0));
    }

    // Fallback repeated timer just in case winit took an extra event loop tick to register
    let timer = slint::Timer::default();
    let applied_clone = applied_once.clone();
    let hwnd_setter = hwnd_holder.clone();
    let cfg_getter = cfg_shared.clone();
    let osd_init_weak = osd.as_weak();

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(15),
        move || {
            let mut done = applied_clone.lock().unwrap();
            if !*done {
                if let Some(hwnd) = find_thread_hwnd() {
                    osd_log(&format!("Fallback captured target HWND: {:?}", hwnd.0));
                    let (pos, theme, scale) = {
                        let c = cfg_getter.lock().unwrap();
                        (c.position.clone(), c.theme.clone(), c.scale.clone())
                    };
                    if let Some(ui) = osd_init_weak.upgrade() {
                        ui.set_osd_theme(theme.clone().into());
                        ui.set_osd_scale(parse_scale_factor(&scale));
                        ui.set_osd_position(pos.clone().into());
                    }
                    apply_osd_window_styles(hwnd, &pos);
                    *hwnd_setter.lock().unwrap() = Some(SafeHwnd::from_hwnd(hwnd));
                    hide_window(SafeHwnd::from_hwnd(hwnd));
                    *done = true;
                    osd_log(&format!("apply_osd_window_styles done (fallback initial pos={}, theme={}, scale={}), window primed and hidden", pos, theme, scale));
                }
            }
        },
    );

    // 30-second idle self-destruction watchdog
    let idle_watchdog = slint::Timer::default();
    idle_watchdog.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(2),
        move || {
            let should_exit = {
                if let Ok(g) = LAST_ACTIVITY.lock() {
                    if let Some(last) = *g {
                        last.elapsed() >= Duration::from_secs(30)
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if should_exit {
                osd_log("Idle timeout reached (30s without OSD activity). Terminating companion process cleanly.");
                std::process::exit(0);
            }
        },
    );

    osd_log("Entering Slint main event loop (run)");
    let _ = osd.run();
}


