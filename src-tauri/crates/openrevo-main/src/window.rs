//! Native window plumbing.
//!
//! Windows: Win32 subclassing, DWM decorations, work-area placement.
//! Linux: the Slint/winit window is driven directly; the handle is an opaque
//! token so callers keep the same call shape on both platforms.

#[cfg(windows)]
#[path = "window_win.rs"]
mod imp;
#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
#[path = "window_linux.rs"]
mod linux;
#[cfg(not(windows))]
pub use linux::*;
