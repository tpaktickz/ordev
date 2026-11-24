//! Display topology and mode control.
//!
//! Windows uses the GDI display APIs (see `display_win.rs`). Linux reads DRM
//! sysfs for topology and delegates the *changes* to whichever standard tool the
//! running session exposes (`wlr-randr`, `kscreen-doctor`, `xrandr`), because
//! there is no freedesktop API for mode switching.

#[cfg(windows)]
#[path = "display_win.rs"]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod imp {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeSet;
    use std::sync::LazyLock;
    use std::time::Duration;

    use crate::system::query_cache::KeyedExpiringCache;

    /// Information for an individual connected and active display monitor
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct DisplayMonitor {
        /// DRM connector directory, e.g. `card0-eDP-1`
        pub device_name: String,
        /// e.g. `eDP-1 (BOE 0x0B4C)`
        pub friendly_name: String,
        pub is_primary: bool,
        pub is_internal: bool,
        pub width: u32,
        pub height: u32,
        pub current_hz: u32,
        pub supported_rates: Vec<u32>,
    }

    type ModeInfo = (u32, u32, u32, Vec<u32>);

    static MODE_INFO_CACHE: LazyLock<KeyedExpiringCache<String, Option<ModeInfo>>> =
        LazyLock::new(|| KeyedExpiringCache::new(Duration::from_secs(30)));

    pub fn invalidate_display_mode_cache() {
        MODE_INFO_CACHE.invalidate_all();
    }

    fn connector_of(device_name: &str) -> String {
        device_name
            .split_once('-')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| device_name.to_string())
    }

    /// Parse EDID detailed timing descriptors into (w, h, hz) triples. Byte
    /// offsets are fixed by the EDID 1.4 layout; entries with a zero pixel clock
    /// are the monitor-name/range descriptors, not timings.
    fn parse_edid_timings(edid: &[u8]) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        if edid.len() < 126 {
            return out;
        }
        for dtd in (54..=108).step_by(18) {
            let block = &edid[dtd..dtd + 18];
            let pixclk = u16::from_le_bytes([block[0], block[1]]) as u64 * 10_000;
            if pixclk == 0 {
                continue;
            }
            let hactive = block[2] as u32 | ((block[4] as u32) >> 4) << 8;
            let hblank = block[3] as u32 | ((block[4] as u32) & 0x0F) << 8;
            let vactive = block[5] as u32 | ((block[7] as u32) >> 4) << 8;
            let vblank = block[6] as u32 | ((block[7] as u32) & 0x0F) << 8;
            let total = (hactive + hblank) as u64 * (vactive + vblank) as u64;
            if total == 0 || hactive == 0 || vactive == 0 {
                continue;
            }
            let hz = ((pixclk * 1000) / total).max(1) as u32;
            out.push((hactive, vactive, hz));
        }
        out
    }

    fn vendor_of(edid: &[u8]) -> String {
        if edid.len() < 10 {
            return String::new();
        }
        String::from_utf8_lossy(&edid[8..10]).trim().to_string()
    }

    fn probe_mode_info_uncached(device_name: &str) -> Option<ModeInfo> {
        let dir = format!("/sys/class/drm/{device_name}");
        let edid = std::fs::read(format!("{dir}/edid")).unwrap_or_default();
        let timings = parse_edid_timings(&edid);
        let mut rates: BTreeSet<u32> = timings.iter().map(|(_, _, hz)| *hz).collect();
        let current = timings.first().copied().or_else(|| {
            std::fs::read_to_string(format!("{dir}/modes"))
                .ok()
                .and_then(|text| {
                    text.lines().next().and_then(|line| {
                        let (w, h) = line.trim().split_once('x')?;
                        Some((w.parse().ok()?, h.parse().ok()?, 60u32))
                    })
                })
        })?;
        if rates.is_empty() {
            rates.insert(current.2);
        }
        Some((current.0, current.1, current.2, rates.into_iter().collect()))
    }

    fn probe_mode_info(device_name: &str) -> Option<ModeInfo> {
        MODE_INFO_CACHE.get_or_compute(device_name.to_string(), || {
            probe_mode_info_uncached(device_name)
        })
    }

    pub fn get_all_monitors() -> Vec<DisplayMonitor> {
        let mut monitors = Vec::new();
        let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
            return fallback_monitor();
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.contains('-') {
                continue;
            }
            let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
            if status.trim() != "connected" {
                continue;
            }
            let Some((width, height, current_hz, supported_rates)) = probe_mode_info(&name) else {
                continue;
            };
            let connector = connector_of(&name);
            let internal = connector.starts_with("eDP") || connector.starts_with("LVDS");
            let edid = std::fs::read(entry.path().join("edid")).unwrap_or_default();
            let vendor = vendor_of(&edid);
            let friendly_name = if internal {
                format!("内置屏 ({connector} {vendor})")
            } else {
                format!("外接屏 ({connector} {vendor})")
            };
            monitors.push(DisplayMonitor {
                device_name: name,
                friendly_name,
                is_primary: monitors.is_empty(),
                is_internal: internal,
                width,
                height,
                current_hz,
                supported_rates,
            });
        }
        if monitors.is_empty() {
            return fallback_monitor();
        }
        monitors
    }

    fn fallback_monitor() -> Vec<DisplayMonitor> {
        vec![DisplayMonitor {
            device_name: "card0-eDP-1".into(),
            friendly_name: "默认显示器".into(),
            is_primary: true,
            is_internal: true,
            width: 1920,
            height: 1080,
            current_hz: 60,
            supported_rates: vec![60],
        }]
    }

    fn primary() -> Option<DisplayMonitor> {
        get_all_monitors().into_iter().find(|m| m.is_primary)
    }

    fn run_tool(program: &str, args: &[&str]) -> bool {
        crate::pal::process::command(program)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn set_monitor_refresh_rate(device_name: &str, hz: u32) -> Result<(), String> {
        let connector = connector_of(device_name);
        let mode = probe_mode_info(device_name)
            .map(|(w, h, _, _)| format!("{w}x{h}@{hz}"))
            .unwrap_or_else(|| format!("@{hz}"));
        if run_tool("wlr-randr", &["--output", &connector, "--mode", &mode]) {
            invalidate_display_mode_cache();
            return Ok(());
        }
        if run_tool("kscreen-doctor", &[&format!("output.{connector}.mode.{mode}")]) {
            invalidate_display_mode_cache();
            return Ok(());
        }
        let xmode = format!("{hz}");
        if run_tool("xrandr", &["--output", &connector, "--rate", &xmode]) {
            invalidate_display_mode_cache();
            return Ok(());
        }
        Err(format!(
            "no display backend accepted a refresh-rate change on {connector} (tried wlr-randr, kscreen-doctor, xrandr)"
        ))
    }

    pub fn toggle_primary_display_orientation() -> Result<bool, String> {
        let monitor = primary().ok_or("no display to rotate")?;
        let connector = connector_of(&monitor.device_name);
        let landscape = monitor.width >= monitor.height;
        let next = if landscape { "left" } else { "normal" };
        let transform = if landscape { "90" } else { "0" };
        if run_tool("wlr-randr", &["--output", &connector, "--transform", transform]) {
            invalidate_display_mode_cache();
            return Ok(!landscape);
        }
        if run_tool("xrandr", &["--output", &connector, "--rotate", next]) {
            invalidate_display_mode_cache();
            return Ok(!landscape);
        }
        Err(format!(
            "no display backend accepted an orientation change on {connector}"
        ))
    }

    pub fn get_refresh_rate() -> u32 {
        primary().map(|m| m.current_hz).unwrap_or(60)
    }

    pub fn set_refresh_rate(hz: u32) -> Result<(), String> {
        let monitor = primary().ok_or("no primary display")?;
        set_monitor_refresh_rate(&monitor.device_name.clone(), hz)
    }

    pub fn get_available_refresh_rates() -> Vec<u32> {
        primary().map(|m| m.supported_rates).unwrap_or_else(|| vec![60])
    }

    fn backlight_dir() -> Option<std::path::PathBuf> {
        std::fs::read_dir("/sys/class/backlight")
            .ok()?
            .flatten()
            .map(|e| e.path())
            .next()
    }

    pub fn get_brightness() -> Result<u8, String> {
        let dir = backlight_dir().ok_or("no backlight device (need a kernel backlight driver)")?;
        let cur: u32 = std::fs::read_to_string(dir.join("brightness"))
            .map_err(|e| e.to_string())?
            .trim()
            .parse()
            .map_err(|_| "unparsable brightness value".to_string())?;
        let max: u32 = std::fs::read_to_string(dir.join("max_brightness"))
            .map_err(|e| e.to_string())?
            .trim()
            .parse()
            .map_err(|_| "unparsable max_brightness value".to_string())?;
        if max == 0 {
            return Err("backlight reports max_brightness 0".into());
        }
        Ok(((cur * 100) / max).min(100) as u8)
    }

    pub fn set_brightness(level: u8) -> Result<(), String> {
        let dir = backlight_dir().ok_or("no backlight device (need a kernel backlight driver)")?;
        let max: u32 = std::fs::read_to_string(dir.join("max_brightness"))
            .map_err(|e| e.to_string())?
            .trim()
            .parse()
            .map_err(|_| "unparsable max_brightness value".to_string())?;
        let target = (max as u64 * level.min(100) as u64 / 100) as u32;
        std::fs::write(dir.join("brightness"), target.to_string())
            .map_err(|e| format!("write {}: {e}", dir.join("brightness").display()))
    }
}

#[cfg(not(windows))]
pub use imp::*;
