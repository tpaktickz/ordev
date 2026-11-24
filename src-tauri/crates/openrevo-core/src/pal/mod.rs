// OpenRevo Platform Abstraction Layer (PAL)
// Centralizes every OS primitive the engine needs, so business modules never
// touch `windows::` or `/sys` directly. Windows and Linux implementations live
// behind `#[cfg]` in each submodule; call sites are platform-neutral.
//
// Autostart is deliberately not here: it lives in `system::autostart`, which
// swaps the whole module per OS (`autostart.rs` / `autostart_linux.rs`) because
// the two backends share nothing but the API shape.

pub mod display_color;
pub mod ec;
pub mod firmware;
pub mod process;
pub mod reg;
pub mod xdg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Windows,
    Linux,
    Other,
}

pub const fn kind() -> PlatformKind {
    #[cfg(windows)]
    {
        PlatformKind::Windows
    }
    #[cfg(target_os = "linux")]
    {
        PlatformKind::Linux
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        PlatformKind::Other
    }
}

/// Short tag for logs and telemetry payloads.
pub const fn tag() -> &'static str {
    match kind() {
        PlatformKind::Windows => "windows",
        PlatformKind::Linux => "linux",
        PlatformKind::Other => "other",
    }
}
pub mod input;
