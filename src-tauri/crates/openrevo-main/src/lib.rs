use std::sync::{Arc, Mutex};
use std::time::Duration;
use slint::ComponentHandle;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

pub use slint;

pub mod callbacks;
pub mod curve_helpers;
pub mod ipc;
pub mod window;

pub use window::SafeHwnd;

slint::include_modules!();

pub fn run_main_standalone() {
    #[cfg(target_os = "windows")]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        }
        const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let window = match MainWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[OpenRevo Slint] Failed to create MainWindow: {}", e);
            return;
        }
    };

    // Dynamically inject the package version from CARGO_PKG_VERSION at compile time
    window.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());

    let hwnd_holder: Arc<Mutex<Option<SafeHwnd>>> = Arc::new(Mutex::new(None));
    let ui_weak = window.as_weak();
    window::set_slint_ui_weak(ui_weak.clone());

    for arg in std::env::args() {
        if let Some(pid_str) = arg.strip_prefix("--host-pid=") {
            if let Ok(pid) = pid_str.parse::<u32>() {
                window::set_host_pid(pid);
            }
        }
    }

    // Start stdin IPC listener from host process
    ipc::start_ipc_listener(ui_weak.clone());

    let start_expanded = std::env::args().any(|a| a == "--expanded");
    if start_expanded {
        window.set_is_expanded(true);
    }

    // Register all callbacks by domain
    callbacks::register_all(&window, ui_weak.clone(), hwnd_holder.clone());

    // Frameless toplevel with no size binding in the markup: give it a sane size
    // before the first frame so the compositor never sees the default 800x600.
    // Later mode snaps go through window_linux.rs / window_win.rs, which also
    // only ever call `set_size` — so a user resize survives until the next snap.
    window.window().set_size(slint::LogicalSize::new(
        if start_expanded { 1120.0 } else { 395.0 },
        if start_expanded { 760.0 } else { 632.0 },
    ));

    let _ = window.show();

    // Synchronously capture HWND and configure visibility/position immediately
    let hwnd_setup = hwnd_holder.clone();
    let mut initial_hwnd_val = 0;
    if let Some(hwnd) = window::find_thread_hwnd() {
        window::apply_window_decorations(hwnd);
        let safe = SafeHwnd(hwnd.0 as isize);
        *hwnd_setup.lock().unwrap() = Some(safe);
        *window::HWND_HOLDER.lock().unwrap() = Some(safe);
        initial_hwnd_val = hwnd.0 as isize;
        if start_expanded {
            let saved = *window::SAVED_FULL_POS.lock().unwrap();
            window::expand_native_window(hwnd, 1120, 760, saved);
        } else {
            window::align_mini_drawer_to_tray(hwnd, 395, 632);
        }
        if !std::env::args().any(|a| a == "--visible") {
            window::hide_window(hwnd);
        } else {
            window::mark_window_woken();
        }
    }

    ipc::send_action_to_host("slint_ready", serde_json::json!({ "hwnd": initial_hwnd_val }));

    // Setup window handles and DWM styles fallback timer
    let timer = slint::Timer::default();
    let applied = Arc::new(Mutex::new(false));
    let applied_clone = applied.clone();

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let mut done = applied_clone.lock().unwrap();
            if !*done {
                if let Some(hwnd) = window::find_thread_hwnd() {
                    window::apply_window_decorations(hwnd);
                    let safe = SafeHwnd(hwnd.0 as isize);
                    *hwnd_setup.lock().unwrap() = Some(safe);
                    *window::HWND_HOLDER.lock().unwrap() = Some(safe);
                    if start_expanded {
                        let saved = *window::SAVED_FULL_POS.lock().unwrap();
                        window::expand_native_window(hwnd, 1120, 760, saved);
                    } else {
                        window::align_mini_drawer_to_tray(hwnd, 395, 632);
                    }
                    if !std::env::args().any(|a| a == "--visible") {
                        window::hide_window(hwnd);
                    }
                    ipc::send_action_to_host("sync_slint_hwnd", serde_json::json!({ "hwnd": hwnd.0 as isize }));
                    *done = true;
                }
            }
        },
    );

    // `Window::run()` quits as soon as the last window is hidden, because hiding
    // releases Slint's per-window keepalive (`i-slint-core/src/window.rs:1946`).
    // The companion is a daemon that starts hidden and is shown by the host over
    // stdin, so on Linux it must use the until-quit entry point instead; the host
    // ends it explicitly with `{"action":"quit"}`.
    #[cfg(windows)]
    {
        let _ = window.run();
    }
    #[cfg(not(windows))]
    {
        // The component itself must outlive the loop: dropping it tears the window
        // down and every later IPC `show` would act on a dead weak handle. Hiding
        // it only releases the loop keepalive, which `until_quit` ignores.
        let _keep_alive = window;
        let _ = slint::run_event_loop_until_quit();
    }
}
