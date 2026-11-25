//! Wayland layer-shell placement for the OSD companion.
//!
//! # Why this exists
//!
//! The OSD is rendered by Slint on the winit backend, and winit's Wayland
//! backend gives the window a plain `xdg_toplevel`. Two consequences on KDE (and
//! on every other compositor): the surface lands wherever the compositor feels
//! like, and it goes *below* a fullscreen game — which is exactly when an OSD is
//! useful. `zwlr_layer_shell_v1` is the escape hatch: a surface in the overlay
//! layer is stacked above everything and positioned by anchoring it to screen
//! edges with margins.
//!
//! # Why not move the Slint window into a layer surface
//!
//! A surface's role is fixed at first commit; winit's surface is already
//! committed as a toplevel, so it cannot be re-parented. And winit 0.30 does not
//! expose the `wl_surface` anyway. So the windowing layer is replaced instead of
//! the rendering layer: this module installs a Slint platform that never creates
//! a real window, renders the component into an in-memory buffer with Slint's
//! software renderer, and blits that buffer into a GTK3 window that *is* in a
//! layer. The `.slint` file, the theme tokens, the animations and every IPC
//! handler stay untouched — only "where do the pixels go" changes.
//!
//! # Why the library is loaded rather than linked
//!
//! `gtk-layer-shell` is not installed everywhere and is useless on X11, so it
//! stays a runtime optional: no library, or a compositor without the protocol,
//! and [`install`] reports failure and the caller keeps the plain winit window.
//!
//! # Transparency
//!
//! The component has a transparent background and animates its card out of view
//! when idle, so an idle OSD costs one fully transparent buffer. The GTK window
//! stays mapped: unmapping a layer surface makes the compositor drop the buffer,
//! and the animation would then start from a blank frame.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_int;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};

/// Design size of the companion window; the card positions itself inside it.
const WIDTH: u32 = 640;
const HEIGHT: u32 = 130;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

// gtk-layer-shell enums
const LAYER_OVERLAY: c_int = 3;
const EDGE_LEFT: c_int = 0;
const EDGE_RIGHT: c_int = 1;
const EDGE_TOP: c_int = 2;
const EDGE_BOTTOM: c_int = 3;
const KEYBOARD_NONE: c_int = 0;

type WindowPtr = *mut std::ffi::c_void;

/// The `GtkWindow*` behind the wrapper.
///
/// Not `&window as *const _ as *mut c_void`: gtk-rs wrappers are values holding the
/// pointer, so that cast yields the address of the *wrapper* — a stack temporary —
/// and every C-side call then works on garbage.
fn window_ptr(window: &gtk::ApplicationWindow) -> WindowPtr {
    use gtk::glib::translate::{Stash, ToGlibPtr};
    let base: &gtk::Window = gtk::prelude::Cast::upcast_ref(window);
    let stash: Stash<'_, *mut gtk::ffi::GtkWindow, gtk::Window> = base.to_glib_none();
    stash.0 as WindowPtr
}

struct Api {
    init_for_window: unsafe extern "C" fn(WindowPtr),
    set_layer: unsafe extern "C" fn(WindowPtr, c_int),
    set_anchor: unsafe extern "C" fn(WindowPtr, c_int, c_int),
    set_margin: unsafe extern "C" fn(WindowPtr, c_int, c_int),
    set_keyboard_mode: unsafe extern "C" fn(WindowPtr, c_int),
    _lib: libloading::Library,
}

impl Api {
    unsafe fn load() -> Option<Self> {
        // SAFETY: the symbols are the documented gtk-layer-shell C API and are
        // matched to their real signatures.
        unsafe {
            let lib = libloading::Library::new("libgtk-layer-shell.so.0").ok()?;
            let is_supported: unsafe extern "C" fn() -> c_int =
                *lib.get(b"gtk_layer_is_supported\0").ok()?;
            if is_supported() == 0 {
                return None;
            }
            Some(Self {
                init_for_window: *lib.get(b"gtk_layer_init_for_window\0").ok()?,
                set_layer: *lib.get(b"gtk_layer_set_layer\0").ok()?,
                set_anchor: *lib.get(b"gtk_layer_set_anchor\0").ok()?,
                set_margin: *lib.get(b"gtk_layer_set_margin\0").ok()?,
                set_keyboard_mode: *lib.get(b"gtk_layer_set_keyboard_mode\0").ok()?,
                _lib: lib,
            })
        }
    }
}

static API: OnceLock<Api> = OnceLock::new();
// `TARGET` is the Slint window this platform hands out and `LAYER_WINDOW` is the
// GTK window hosting it, so a settings change can re-anchor later. Both are `!Send`
// and both are used only on the main thread.
thread_local! {
    static TARGET: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    static LAYER_WINDOW: RefCell<Option<gtk::ApplicationWindow>> = const { RefCell::new(None) };
}
/// Callbacks posted through `slint::invoke_from_event_loop`, drained on the GTK
/// main loop. A plain mutex rather than a thread-local: the posting thread is
/// usually not the main one, and a thread-local would hand it its own queue.
static PENDING: Mutex<VecDeque<Box<dyn FnOnce() + Send>>> = Mutex::new(VecDeque::new());
/// Set when Slint asks the loop to stop; consumed by the frame callback.
static QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Frames still to be drawn after the last queued event, so an animation that
/// starts on the frame *after* the event still gets painted even if the component
/// only requests one redraw.
/// Set once the layer surface is live. The surface stays mapped while the OSD is
/// idle (the card animates itself out of view), so hide/show must not unmap it.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static TAIL_FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const TAIL_LEN: u32 = 24;

/// Whether layer placement should be attempted at all.
///
/// `OPENREVO_OSD_LAYER=0` forces the plain winit window back (useful when a
/// compositor advertises the protocol but misbehaves); anything else is auto.
pub fn usable() -> bool {
    match std::env::var("OPENREVO_OSD_LAYER").ok().as_deref() {
        Some("0") => return false,
        Some("1") => {}
        _ => {
            if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                return false;
            }
            if std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("x11") {
                return false;
            }
        }
    }
    true
}

/// Whether the layer surface owns the window; callers then leave visibility alone.
pub fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

struct Proxy;

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        // Slint quits the loop as soon as the last window hides, which here can
        // happen before the loop even started, and `gtk_main_quit` panics when no
        // loop is running. The flag is honoured by the frame callback instead, which
        // by construction only runs inside the loop.
        QUIT.store(true, std::sync::atomic::Ordering::Relaxed);
        if gtk::main_level() > 0 {
            gtk::main_quit();
        }
        Ok(())
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), slint::EventLoopError> {
        if let Ok(mut q) = PENDING.lock() {
            q.push_back(event);
        }
        // Not `idle_add_local_once`: that one needs the default main context to be
        // owned by the calling thread, and events arrive from the stdin reader.
        glib::idle_add_once(|| {});
        Ok(())
    }
}

struct LayerPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for LayerPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy))
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        let app = gtk::Application::builder()
            .application_id("io.github.tpaktickz.openrevo.osd")
            .build();
        let window = self.window.clone();
        app.connect_activate(move |app| activate(app, &window));
        // `run()` would forward the process argv, which GApplication interprets as
        // files to open; without a handler that path leaves the application
        // unlaunched and every widget built in `activate` comes out invalid.
        app.run_with_args::<&str>(&[]);
        Ok(())
    }
}

/// Install the layer platform as Slint's platform.
///
/// Must run before the first component is created. Returns `false` when the
/// caller should stay on the winit backend, in which case nothing is installed.
pub fn install() -> bool {
    if !usable() {
        return false;
    }
    // `gtk_layer_is_supported` answers for the *current* display, so GTK has to be
    // up before the library can be asked anything. `gtk::Application::run` calls
    // `gtk_init` again; it is idempotent.
    if gtk::init().is_err() {
        return false;
    }
    let Some(api) = (unsafe { Api::load() }) else {
        return false;
    };
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    if slint::platform::set_platform(Box::new(LayerPlatform {
        window: window.clone(),
    }))
    .is_err()
    {
        return false;
    }
    ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = API.set(api);
    TARGET.with(|t| *t.borrow_mut() = Some(window));
    true
}

/// Anchors for a configured OSD position, as `(edge flags, margins)`.
///
/// The component places its card on the left, right or centre of its own window
/// and vertically centres it, so only the *window's* corner is pinned here.
fn anchor_for(position: &str) -> (c_int, c_int) {
    let horizontal = if position.contains("left") {
        EDGE_LEFT
    } else if position.contains("right") {
        EDGE_RIGHT
    } else {
        -1
    };
    let vertical = if position.starts_with("top") {
        EDGE_TOP
    } else if position.starts_with("bottom") {
        EDGE_BOTTOM
    } else {
        -1
    };
    (horizontal, vertical)
}

/// Re-anchor the surface, e.g. after the position setting changed.
pub fn apply_position(position: &str) {
    let window = LAYER_WINDOW.with(|w| w.borrow().clone());
    if let Some(window) = window {
        anchor_window(&window, position);
    }
}

fn anchor_window(window: &gtk::ApplicationWindow, position: &str) {
    let Some(api) = API.get() else { return };
    let ptr = window_ptr(window);
    let (horizontal, vertical) = anchor_for(position);
    for (edge, anchored) in [
        (EDGE_LEFT, horizontal == EDGE_LEFT),
        (EDGE_RIGHT, horizontal == EDGE_RIGHT),
        (EDGE_TOP, vertical == EDGE_TOP),
        (EDGE_BOTTOM, vertical == EDGE_BOTTOM),
    ] {
        let anchored = if anchored { 1 } else { 0 };
        unsafe {
            (api.set_anchor)(ptr, edge, anchored);
            // Flush against the edge: the component already insets its own card, so
            // an extra compositor margin would double the gap.
            if anchored == 1 {
                (api.set_margin)(ptr, edge, 0);
            }
        }
    }
}

fn activate(app: &gtk::Application, slint_window: &Rc<MinimalSoftwareWindow>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .default_width(WIDTH as i32)
        .default_height(HEIGHT as i32)
        .decorated(false)
        .resizable(false)
        .can_focus(false)
        .build();

    let scale = gtk::prelude::WidgetExt::scale_factor(&window).max(1);
    let slint_window_handle = slint_window.window();
    slint_window_handle.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: scale as f32,
    });
    slint_window.set_size(slint::LogicalSize::new(WIDTH as f32, HEIGHT as f32));

    if let Some(screen) = gtk::prelude::WidgetExt::screen(&window) {
        if let Some(visual) = screen.rgba_visual() {
            window.set_visual(Some(&visual));
        }
    }
    window.set_app_paintable(true);

    let ptr = window_ptr(&window);
    if let Some(api) = API.get() {
        unsafe {
            (api.init_for_window)(ptr);
            (api.set_layer)(ptr, LAYER_OVERLAY);
            (api.set_keyboard_mode)(ptr, KEYBOARD_NONE);
        }
        anchor_window(&window, &crate::current_position());
    }

    let width_px = WIDTH * scale as u32;
    let height_px = HEIGHT * scale as u32;

    let area = gtk::DrawingArea::new();
    area.set_size_request(WIDTH as i32, HEIGHT as i32);
    window.add(&area);

    let pixels = Rc::new(RefCell::new(vec![
        PremultipliedRgbaColor::default();
        (width_px * height_px) as usize
    ]));
    let bytes = Rc::new(RefCell::new(vec![0u8; (width_px * height_px * 4) as usize]));

    {
        let bytes = bytes.clone();
        area.connect_draw(move |_, cr| {
            let data = bytes.borrow();
            if let Ok(surface) = cairo::ImageSurface::create_for_data(
                data.clone(),
                cairo::Format::ARgb32,
                width_px as i32,
                height_px as i32,
                (width_px * 4) as i32,
            ) {
                surface.set_device_scale(scale as f64, scale as f64);
                let _ = cr.set_source_surface(&surface, 0.0, 0.0);
                let _ = cr.paint();
            }
            glib::Propagation::Proceed
        });
    }

    let frame_window = slint_window.clone();
    let frame_area = area.clone();
    gtk::glib::timeout_add_local(FRAME_INTERVAL, move || {
        if QUIT.load(std::sync::atomic::Ordering::Relaxed) {
            gtk::main_quit();
            return glib::ControlFlow::Break;
        }
        let mut due = Vec::new();
        if let Ok(mut q) = PENDING.lock() {
            while let Some(cb) = q.pop_front() {
                due.push(cb);
            }
        }
        let had_events = !due.is_empty();
        for cb in due {
            cb();
        }
        slint::platform::update_timers_and_animations();

        if had_events {
            TAIL_FRAMES.store(TAIL_LEN, std::sync::atomic::Ordering::Relaxed);
        } else {
            let tail = TAIL_FRAMES.swap(0, std::sync::atomic::Ordering::Relaxed);
            if tail > 0 {
                TAIL_FRAMES.store(tail - 1, std::sync::atomic::Ordering::Relaxed);
                frame_window.request_redraw();
            }
        }

        let redrawn = frame_window.draw_if_needed(|renderer| {
            let mut pixels = pixels.borrow_mut();
            renderer.render(&mut pixels[..], width_px as usize);
        });
        if redrawn {
            let pixels = pixels.borrow();
            let mut bytes = bytes.borrow_mut();
            for (i, pixel) in pixels.iter().enumerate() {
                // cairo's ARGB32 is premultiplied BGRA on little endian.
                bytes[i * 4] = pixel.blue;
                bytes[i * 4 + 1] = pixel.green;
                bytes[i * 4 + 2] = pixel.red;
                bytes[i * 4 + 3] = pixel.alpha;
            }
            drop(pixels);
            frame_area.queue_draw();
        }
        glib::ControlFlow::Continue
    });

    LAYER_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
    window.show_all();
    window.present();
}
