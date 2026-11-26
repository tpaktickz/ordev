//! Hardware ground truth for the dashboard.
//!
//! Every field the panel used to echo straight out of `config.json` is read back
//! from the EC here instead. `config.json` is a *cache of last intent*: it goes
//! stale the moment the Fn keys, the OEM console, the firmware's own scheduler or
//! a previous OS writes the same registers, so anything with a hardware source of
//! truth must be resolved from hardware and only fall back to config.
//!
//! Reads are serialised through a short-lived cache because each mailbox
//! transaction costs 10-60 ms and the sync paths fire on every telemetry tick.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::driver::wmi_ec;

/// Registers are cheap but not free; one read per 2 s is far above any human
/// rate of change and keeps a panel redraw from blocking on EC round-trips.
const TTL: Duration = Duration::from_millis(2000);

#[derive(Debug, Clone, Copy, Default)]
pub struct HwTruth {
    /// 1 = Office, 2 = Balance, 3 = Turbo, 4 = Custom (Turbo+super-performance clamp).
    pub power_mode: Option<u8>,
    /// CPU long-duration limit actually latched in the EC, in watts.
    pub pl1: Option<u8>,
    /// GPU cTGP offset actually latched in the EC, in watts.
    pub tgp: Option<u8>,
    pub battery_limit: Option<u8>,
    pub fan_boost: Option<bool>,
    pub turbo_silent: Option<bool>,
    pub ac_recovery: Option<bool>,
}

struct Cache {
    at: Option<Instant>,
    value: HwTruth,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    at: None,
    value: HwTruth {
        power_mode: None,
        pl1: None,
        tgp: None,
        battery_limit: None,
        fan_boost: None,
        turbo_silent: None,
        ac_recovery: None,
    },
});

/// `ADDR_CUSTOM_MODE` holds 64 in every factory mode and 65 only while the custom
/// profile is asserted, which is the only way to tell Turbo from Custom: both
/// carry MODE_INDEX 0x10 and the same clamp base byte 0x10 (that 0x10 is the Turbo
/// Q-Key base, NOT the super-performance bit, so the clamp must not be used here).
fn decode_power_mode(mode_index: u8, trigger: u8, custom_mode: u8) -> u8 {
    if (mode_index & 0x10) != 0 {
        if custom_mode >= 65 {
            4
        } else {
            3
        }
    } else if (trigger & 0x08) != 0 {
        1
    } else {
        2
    }
}

fn probe() -> HwTruth {
    let mode_index = wmi_ec::wmi_read_ec(wmi_ec::ADDR_MODE_INDEX);
    let trigger = wmi_ec::wmi_read_ec(wmi_ec::ADDR_TRIGGER_BYTE);
    let clamp = wmi_ec::wmi_read_ec(wmi_ec::ADDR_FAN_BOOST_CLAMP);

    let custom_mode = wmi_ec::wmi_read_ec(wmi_ec::ADDR_CUSTOM_MODE);
    let power_mode = match (mode_index, trigger, custom_mode) {
        (Some(m), Some(t), Some(c)) => Some(decode_power_mode(m, t, c)),
        _ => None,
    };

    // Plausibility gates: an unreachable EC reads 0x00 or 0xFF, and a 0 W PL1 or a
    // 200 W TGP offset is a decode failure, not a real state.
    let pl1 = wmi_ec::wmi_read_ec(wmi_ec::ADDR_CUSTOM_PL1).filter(|v| (5..=200).contains(v));
    let tgp = wmi_ec::wmi_read_ec(wmi_ec::ADDR_TGP_OFFSET).filter(|v| *v <= 60);

    let battery_limit = wmi_ec::wmi_read_ec(wmi_ec::ADDR_BATTERY_LIMIT_UP)
        .filter(|v| (50..=100).contains(v))
        .or_else(|| {
            let protect = wmi_ec::wmi_read_ec(wmi_ec::ADDR_BATTERY_PROTECTION_MODE)?;
            match protect & 0xF0 {
                0x20 => Some(60),
                0x10 => Some(80),
                _ => Some(100),
            }
        });

    HwTruth {
        power_mode,
        pl1,
        tgp,
        battery_limit,
        fan_boost: clamp.map(|v| (v & wmi_ec::FAN_BOOST_CLAMP_BIT) != 0),
        turbo_silent: wmi_ec::wmi_read_ec(wmi_ec::ADDR_TURBO_SUBMODE).map(|v| (v & 0x01) != 0),
        ac_recovery: wmi_ec::wmi_read_ec(wmi_ec::ADDR_AC_RECOVERY)
            .map(|v| (v & wmi_ec::MASK_QKEY_WHITE_AC_RECOVERY) != 0),
    }
}

/// Cached hardware truth. Never blocks on the EC more than once per `TTL`.
pub fn read() -> HwTruth {
    let mut guard = match CACHE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(at) = guard.at {
        if at.elapsed() < TTL {
            return guard.value;
        }
    }
    let value = probe();
    guard.at = Some(Instant::now());
    guard.value = value;
    value
}

/// Force the next `read()` to hit hardware (call after every write).
pub fn invalidate() {
    let mut guard = match CACHE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.at = None;
}

#[cfg(test)]
mod tests {
    use super::decode_power_mode;

    #[test]
    fn decodes_office_from_trigger_byte() {
        assert_eq!(decode_power_mode(0x00, 0x28, 0x00), 1);
    }

    #[test]
    fn decodes_balance_from_plain_trigger() {
        assert_eq!(decode_power_mode(0x00, 0x20, 0x00), 2);
    }

    #[test]
    fn decodes_turbo_and_custom_from_custom_mode_marker() {
        assert_eq!(decode_power_mode(0x10, 0x00, 64), 3);
        assert_eq!(decode_power_mode(0x10, 0x00, 65), 4);
    }
}
