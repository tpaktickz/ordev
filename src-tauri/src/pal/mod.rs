//! Shell-side platform helpers.
//!
//! The core crate owns the hardware-side platform layer; this module re-exports
//! it for the Tauri shell and adds the pieces only the shell needs (native
//! window handles for the webview windows).

pub use openrevo_core::pal::*;

pub mod window;
