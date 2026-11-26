use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FanTableData {
    pub cpu_upt: [u8; 16],
    pub cpu_downt: [u8; 16],
    pub cpu_duty: [u8; 16],
    pub gpu_upt: [u8; 16],
    pub gpu_downt: [u8; 16],
    pub gpu_duty: [u8; 16],
    pub max_duty: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelFanTables {
    #[serde(default)]
    pub office: Option<FanTableData>,
    #[serde(default)]
    pub balanced: Option<FanTableData>,
    #[serde(default)]
    pub turbo: Option<FanTableData>,
}

pub const FAN_TABLE_OFFICE: FanTableData = FanTableData {
    cpu_upt:   [0, 55, 57, 59, 61, 63, 65, 67, 69, 255, 255, 255, 255, 255, 255, 255],
    cpu_downt: [48, 58, 60, 62, 64, 66, 68, 70, 255, 255, 255, 255, 255, 255, 255, 255],
    cpu_duty:  [0, 60, 60, 60, 60, 70, 70, 90, 110, 110, 110, 110, 110, 110, 110, 110], // 55% max duty
    gpu_upt:   [0, 55, 55, 57, 57, 59, 62, 64, 66, 255, 255, 255, 255, 255, 255, 255],
    gpu_downt: [48, 56, 58, 58, 60, 63, 65, 67, 255, 255, 255, 255, 255, 255, 255, 255],
    gpu_duty:  [0, 60, 60, 60, 60, 70, 70, 90, 110, 110, 110, 110, 110, 110, 110, 110],
    max_duty:  110,
};

pub const FAN_TABLE_GAMING: FanTableData = FanTableData {
    cpu_upt:   [0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 255, 255, 255, 255, 255, 255],
    cpu_downt: [48, 58, 61, 65, 69, 73, 76, 79, 82, 255, 255, 255, 255, 255, 255, 255],
    cpu_duty:  [0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 160, 160, 160, 160, 160, 160], // 80% max duty
    gpu_upt:   [0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 255, 255, 255, 255, 255, 255],
    gpu_downt: [48, 58, 61, 65, 69, 73, 76, 79, 82, 255, 255, 255, 255, 255, 255, 255],
    gpu_duty:  [0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 160, 160, 160, 160, 160, 160],
    max_duty:  160,
};

pub const FAN_TABLE_TURBO: FanTableData = FanTableData {
    cpu_upt:   [0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 84, 255, 255, 255, 255, 255],
    cpu_downt: [48, 58, 61, 65, 69, 73, 76, 79, 82, 85, 255, 255, 255, 255, 255, 255],
    cpu_duty:  [0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 180, 180, 180, 180, 180, 180], // 90% max duty
    gpu_upt:   [0, 55, 58, 61, 65, 69, 72, 75, 78, 81, 84, 255, 255, 255, 255, 255],
    gpu_downt: [48, 58, 61, 65, 69, 73, 76, 79, 82, 85, 255, 255, 255, 255, 255, 255],
    gpu_duty:  [0, 60, 70, 80, 100, 110, 120, 130, 150, 160, 180, 180, 180, 180, 180, 180],
    max_duty:  180,
};

pub fn get_mode_fan_table(ui_mode: u8) -> FanTableData {
    let active_profile = crate::system::model_engine::get_active_profile();
    let fan_tables = active_profile.as_ref().and_then(|p| p.fan_tables.as_ref());
    match ui_mode {
        1 => fan_tables.and_then(|ft| ft.office.clone()).unwrap_or_else(|| FAN_TABLE_OFFICE.clone()),
        2 => fan_tables.and_then(|ft| ft.balanced.clone()).unwrap_or_else(|| FAN_TABLE_GAMING.clone()),
        3 => fan_tables.and_then(|ft| ft.turbo.clone()).unwrap_or_else(|| FAN_TABLE_TURBO.clone()),
        _ => fan_tables.and_then(|ft| ft.balanced.clone()).unwrap_or_else(|| FAN_TABLE_GAMING.clone()),
    }
}

pub fn apply_mode_fan_table(ui_mode: u8) {
    let cfg = crate::system::config::load_config();
    if cfg.fan_boost || crate::driver::wmi_ec::is_fan_boost_active() {
        crate::log_info!("POWER_MODE", "apply_mode_fan_table: fan boost is active, skipping fan table write to preserve full fan speed");
        return;
    }
    // Ground Truth: Legacy Pascal/Turing chassis (e.g. GK5 / 10-series / 20-series) strictly uses
    // internal EC ROM fan curves. Injecting RAM fan tables corrupts the legacy fan state machine!
    // Note: 30-series (DualModeAmpere) fully supports RamFan1p5 and must NOT be bypassed.
    if crate::hal::is_legacy_pascal_turing_tongfang() {
        crate::log_info!("POWER_MODE", "apply_mode_fan_table: Legacy Pascal/Turing chassis detected, yielding to native EC firmware fan curve");
        return;
    }
    let table = get_mode_fan_table(ui_mode);
    apply_fan_table_to_ec(&table);
}

pub fn build_custom_fan_table(
    cpu_curve: &[crate::system::config::FanCurvePoint],
    gpu_curve: &[crate::system::config::FanCurvePoint],
) -> FanTableData {
    let turbo_table = get_mode_fan_table(3);

    // Standardized safe linear temperature progression (30°C ~ 100°C in 5°C increments)
    // Eliminates factory mute freeze (all 65s) and hysteresis deadlocks (DownT >= UpT)
    let mut custom_upt = [0u8; 16];
    let mut custom_downt = [0u8; 16];
    custom_upt[0] = 0;
    custom_downt[0] = 0;
    for i in 1..16 {
        let t = (30 + (i - 1) * 5) as u8; // Step 1: 30°C ... Step 15: 100°C
        custom_upt[i] = t;
        // Anti-Hunting Hysteresis: Widen gap to 8°C for idle/low-load (<= 55°C) to prevent start-stop oscillation!
        let hyst = if t <= 55 { 8 } else { 5 };
        custom_downt[i] = t.saturating_sub(hyst).max(custom_downt[i - 1]);
    }

    let (effective_cpu_upt, effective_cpu_downt) = if !cpu_curve.is_empty() {
        (custom_upt, custom_downt)
    } else {
        (turbo_table.cpu_upt, turbo_table.cpu_downt)
    };

    let (effective_gpu_upt, effective_gpu_downt) = if !gpu_curve.is_empty() {
        (custom_upt, custom_downt)
    } else {
        let mut upt = turbo_table.gpu_upt;
        let mut downt = turbo_table.gpu_downt;
        if upt[1] == 65 && upt[5] == 65 {
            upt = custom_upt;
            downt = custom_downt;
        } else {
            for i in 1..16 {
                if upt[i] != 255 && downt[i] >= upt[i] {
                    downt[i] = upt[i].saturating_sub(4);
                }
            }
        }
        (upt, downt)
    };

    let interpolate_duty = |curve: &[crate::system::config::FanCurvePoint], base_upt: &[u8; 16], fallback_duty: &[u8; 16]| -> [u8; 16] {
        let mut duty = [0u8; 16];
        if curve.is_empty() {
            return *fallback_duty;
        }

        for step in 0..16 {
            let t = base_upt[step];
            if t == 255 {
                duty[step] = duty[step.saturating_sub(1)];
                continue;
            }
            if t <= curve[0].temp {
                duty[step] = ((curve[0].duty as u16 * 200) / 100).min(200) as u8;
                continue;
            }
            let last_idx = curve.len() - 1;
            if t >= curve[last_idx].temp {
                duty[step] = ((curve[last_idx].duty as u16 * 200) / 100).min(200) as u8;
                continue;
            }
            // Linear interpolation between adjacent curve points
            let mut found = false;
            for i in 0..last_idx {
                let p1 = &curve[i];
                let p2 = &curve[i + 1];
                if t >= p1.temp && t <= p2.temp {
                    let span_t = (p2.temp - p1.temp) as f32;
                    let factor = if span_t > 0.0 { (t - p1.temp) as f32 / span_t } else { 0.0 };
                    let d = p1.duty as f32 + factor * (p2.duty as f32 - p1.duty as f32);
                    duty[step] = ((d * 2.0).round() as u16).min(200) as u8;
                    found = true;
                    break;
                }
            }
            if !found {
                duty[step] = ((curve[last_idx].duty as u16 * 200) / 100).min(200) as u8;
            }
        }

        // Hardware Safety Guard: Enforce strict monotonic non-decreasing progression
        // EC firmware rejects fan tables with inverted/decreasing duty steps!
        let mut running_max = 0u8;
        for step in 0..16 {
            if duty[step] < running_max {
                duty[step] = running_max;
            } else {
                running_max = duty[step];
            }
        }

        // CRITICAL MECHREVO / UNIWILL EC FIRMWARE RULE:
        // Step 0 (EC 3872 / 3920) is the Zero-RPM standby threshold (0~50°C idle).
        // It MUST ALWAYS be 0x00! If written with non-zero, the EC fan FSM state machine
        // treats it as invalid standby configuration and completely locks up fan control!
        duty[0] = 0;

        duty
    };

    let cpu_duty = interpolate_duty(cpu_curve, &effective_cpu_upt, &turbo_table.cpu_duty);
    let mut gpu_duty = interpolate_duty(gpu_curve, &effective_gpu_upt, &turbo_table.gpu_duty);

    // Live hardware sniffer rule: GPU tail steps 13..15 (3933..3935 / RamFan1p5) must sync to max GPU duty
    let max_gpu = gpu_duty[12].max(gpu_duty[15]);
    gpu_duty[13] = gpu_duty[13].max(max_gpu);
    gpu_duty[14] = gpu_duty[14].max(max_gpu);
    gpu_duty[15] = max_gpu;

    let max_duty = cpu_duty[15].max(gpu_duty[15]).max(160);

    FanTableData {
        cpu_upt: effective_cpu_upt,
        cpu_downt: effective_cpu_downt,
        cpu_duty,
        gpu_upt: effective_gpu_upt,
        gpu_downt: effective_gpu_downt,
        gpu_duty,
        max_duty,
    }
}

/// Table handed to the EC by the most recent commit, kept only so the diagnostic
/// sweep in [`schedule_verification`] has something to compare the fans against.
static LAST_APPLIED: std::sync::Mutex<Option<FanTableData>> = std::sync::Mutex::new(None);

/// Hand the fans to the AP by setting ADDR_FAN_MANUAL_BYTE bit 6 (0x40) — the switch
/// the EC checks before it will run an injected table — keeping the neighbouring
/// camera-cutoff bit 3 intact.
pub(crate) fn assert_fan_manual() {
    use crate::driver::wmi_ec;
    let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_FAN_MANUAL_BYTE) else {
        return;
    };
    if (val & wmi_ec::FAN_MANUAL_ENABLE_BIT) != 0 {
        return;
    }
    let ok = wmi_ec::wmi_write_ec(wmi_ec::ADDR_FAN_MANUAL_BYTE, val | wmi_ec::FAN_MANUAL_ENABLE_BIT);
    crate::log_info!(
        "POWER_MODE",
        "fan manual mode asserted: {:#04x} -> {:#04x} on {:#06x} (ok={})",
        val,
        val | wmi_ec::FAN_MANUAL_ENABLE_BIT,
        wmi_ec::ADDR_FAN_MANUAL_BYTE,
        ok
    );
}

/// The step whose upper temperature threshold still exceeds `temp`.
fn step_for_temp(upt: &[u8; 16], temp: u8) -> usize {
    for (i, &t) in upt.iter().enumerate() {
        if t == 255 || temp < t {
            return i;
        }
    }
    15
}

/// Live duty of both fans straight from the EC, before any ramp has started.
fn live_fan_duty() -> Option<(u8, u8)> {
    use crate::driver::wmi_ec;
    if wmi_ec::ADDR_MAIN_FAN_L_DUTY_BYTE == 0 || wmi_ec::ADDR_MAIN_FAN_R_DUTY_BYTE == 0 {
        return None;
    }
    let left = wmi_ec::wmi_read_ec(wmi_ec::ADDR_MAIN_FAN_L_DUTY_BYTE)?;
    let right = wmi_ec::wmi_read_ec(wmi_ec::ADDR_MAIN_FAN_R_DUTY_BYTE)?;
    Some((left, right))
}

/// Whether the duty the EC is holding matches what `table` asks for at `temp`.
///
/// The EC follows a temperature step with a ramp delay, so a reading taken one or
/// two steps late still proves the table is live: a firmware that ignores the table
/// sits on its own curve (0x32 here) and matches none of them. Accepting only the
/// exact step made a mode switch at 57 °C read as "ignored" while the fans were
/// still climbing out of the 60 duty that the previous step asked for.
fn firmware_follows_table(table: &FanTableData, temp: u8, duty: (u8, u8)) -> bool {
    const DUTY_TOLERANCE: i16 = 6;
    // The EC follows the table with a lag: right after a threshold crossing the duty
    // is still climbing out of the previous step's value, and a single-point compare
    // then reads as "ignored" (seen at 57 °C, where the step asks 0x46 while the fan
    // still held the 0x3c of the 55 °C step). Accept any step inside a small lag
    // window; a firmware that ignores the table sits on its own curve (0x32 here) and
    // matches none of them.
    const RAMP_LAG_STEPS: usize = 2;
    let step = step_for_temp(&table.cpu_upt, temp);
    let lowest = step.saturating_sub(RAMP_LAG_STEPS);
    let got = duty.0.max(duty.1) as i16;
    (lowest..=step).any(|i| (got - table.cpu_duty[i] as i16).abs() <= DUTY_TOLERANCE)
}

/// Re-read the hardware a ramp-length later and report whether the table actually
/// drives the fans. Diagnostic only: like the Windows build, every mode switch
/// writes the table regardless of what the last reading said, so a switch taken
/// while the fans were still ramping can never disable injection.
fn schedule_verification(table: FanTableData) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(28));
        let (Some(temp), Some(duty)) = (
            crate::pal::tongfang::read_cpu_temp_celsius(),
            live_fan_duty(),
        ) else {
            return;
        };
        let step = step_for_temp(&table.cpu_upt, temp);
        if firmware_follows_table(&table, temp, duty) {
            crate::log_info!("POWER_MODE", "fan table verdict: honoured ({} °C, EC holds {:#04x}/{:#04x})", temp, duty.0, duty.1);
        } else {
            crate::log_warn!(
                "POWER_MODE",
                "fan table verdict: EC not following the table yet ({} °C step {} asks {:#04x}, EC holds {:#04x}/{:#04x})",
                temp, step, table.cpu_duty[step], duty.0, duty.1
            );
        }
    });
}

pub fn apply_fan_table_to_ec(table: &FanTableData) {
    use crate::driver::wmi_ec;

    if crate::hal::is_legacy_pascal_turing_tongfang() {
        crate::log_warn!("POWER_MODE", "apply_fan_table_to_ec: Legacy Pascal/Turing chassis does not support RamFan1p5 table injection, skipping to prevent fan oscillation");
        return;
    }

    // 0. Ensure APExist is asserted so motherboard accepts table writes
    if let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_AP_OEM_BYTE) {
        if (val & 0x01) == 0 {
            let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_AP_OEM_BYTE, val | 0x01);
        }
    }

    // 0b. Hand fan control to the AP. ADDR_FAN_MANUAL_BYTE bit 6 (0x40) is the switch
    // the EC actually checks before it will run an injected table. Without it the whole
    // 96-entry write below is accepted, latched and ignored: measured on this chassis
    // (0x727=0x88, i7-14650HX) the EC held its own 0x32 idle duty while the table asked
    // for 0x60, and with 0x727=0xC8 it tracked the table instead (0x48 → 0x80 following
    // duty[2]=0x80, and 0x7d → 0x3c on the way back down). Read-modify-write, because
    // this byte shares bit 3 with ADDR_CAMERA_CUTOFF — clearing that would switch a
    // camera the user turned off back on.
    assert_fan_manual();

    // Official Mechrevo / Uniwill EC specification & Ground-Truth Live Sniffer Rules:
    // 1. Write full CPU & GPU temperature thresholds (UpT, DownT, GPU UpT, GPU DownT).
    //    Leaving temperature thresholds unwritten (all 0s) causes EC to evaluate any temp > 0 as exceeding Step 15,
    //    leading to the notorious 100% full-speed fan lockup bug!
    // 2. CPU Fan Duty and GPU Fan Duty (16 bytes).
    // 3. Step 0 is Zero-RPM Standby: MUST ALWAYS BE 0x00!
    // 4. RamFan1p5 status & control registers sync with maximum GPU duty.
    // 5. Hardware Latch: Pre-latch pulse, then write all tables, then commit!

    // Step A: Pre-latch reset pulse
    wmi_ec::latch_fan_table_ec(true);

    // Step B: Assert APExist & AP Active
    if let Some(val) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_AP_OEM_BYTE) {
        let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_AP_OEM_BYTE, val | 0x81);
    }

    let mut entries = Vec::with_capacity(70);
    // 1. CPU Up Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_UPT_BASE + i as u16, table.cpu_upt[i]));
    }
    // 2. CPU Down Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_DOWNT_BASE + i as u16, table.cpu_downt[i]));
    }
    // 3. CPU Fan Duty (16 bytes, Step 0 = 0)
    for i in 0..16 {
        let duty = if i == 0 { 0 } else { table.cpu_duty[i] };
        entries.push((wmi_ec::ADDR_RAMFAN_CPU_DUTY_BASE + i as u16, duty));
    }
    // 4. GPU Up Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_UPT_BASE + i as u16, table.gpu_upt[i]));
    }
    // 5. GPU Down Temperature Thresholds (16 bytes)
    for i in 0..16 {
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_DOWNT_BASE + i as u16, table.gpu_downt[i]));
    }
    // 6. GPU Fan Duty (13 bytes, Step 0 = 0)
    for i in 0..13 {
        let duty = if i == 0 { 0 } else { table.gpu_duty[i] };
        entries.push((wmi_ec::ADDR_RAMFAN_GPU_DUTY_BASE + i as u16, duty));
    }

    // 7. Sync RamFan1p5 status & control registers
    // CRITICAL GROUND-TRUTH RULES:
    // - Status/max-duty thresholds: sync to max GPU duty (table.gpu_duty[15]).
    // - ADDR_RAMFAN_TABLE_CTRL:
    //   ABSOLUTELY NEVER WRITE 0 TO TABLE_CTRL!
    //   Writing 0 signals a controller reset/disable to the EC firmware, which causes the EC
    //   fail-safe watchdog to panic and blast both fans to 5000+ RPM full-speed lockup!
    //   In official GCUService and live sniffer telemetry, active control byte is 0xA0 (160) or clamp to safe non-zero.
    let max_gpu = table.gpu_duty[15];
    let ctrl_val = if max_gpu > 0 { max_gpu.max(160) } else { 0xA0 };
    entries.push((wmi_ec::ADDR_RAMFAN_GPU_MAX_THRESHOLD1, max_gpu));
    entries.push((wmi_ec::ADDR_RAMFAN_GPU_MAX_THRESHOLD2, max_gpu));
    entries.push((wmi_ec::ADDR_RAMFAN_TABLE_CTRL, ctrl_val));

    let batch_started = std::time::Instant::now();
    wmi_ec::wmi_write_ec_batch(&entries);
    crate::log_info!(
        "POWER_MODE",
        "fan table: {} entries written in {} ms",
        entries.len(),
        batch_started.elapsed().as_millis()
    );

    // Step C: Post-latch Commit: Register ADDR_FAN_TABLE_LATCH = 0x04!
    // Ground-truth reverse engineered from official GCUService:
    // Writing 0x04 latches and commits the RAM fan table into the EC firmware active controller!
    wmi_ec::commit_fan_table_latch_ec();

    if !wmi_ec::is_nextgen_oem_magic_ec() {
        if let Some(cur_trig) = wmi_ec::wmi_read_ec(wmi_ec::ADDR_TRIGGER_BYTE) {
            let _ = wmi_ec::wmi_write_ec(wmi_ec::ADDR_TRIGGER_BYTE, (cur_trig & 0x10) | 0x20);
        }
    }

    crate::log_info!(
        "POWER_MODE",
        "Full fan table committed to EC RAM (UpT/DownT thresholds + Duties, fan table latch committed):\n  CPU Duty: {:?}\n  GPU Duty: {:?}",
        table.cpu_duty,
        table.gpu_duty
    );

    if let Ok(mut guard) = LAST_APPLIED.lock() {
        *guard = Some(table.clone());
    }
    schedule_verification(table.clone());
}

#[cfg(test)]
mod verdict_tests {
    use super::*;

    fn table() -> FanTableData {
        FAN_TABLE_TURBO.clone()
    }

    #[test]
    fn step_selection_follows_upper_thresholds() {
        let t = table();
        // Step 0 is the zero-rpm standby step: anything at or above its 0 °C threshold moves on.
        assert_eq!(step_for_temp(&t.cpu_upt, 0), 1);
        assert_eq!(step_for_temp(&t.cpu_upt, 54), 1);
        assert_eq!(step_for_temp(&t.cpu_upt, 57), 2);
        assert_eq!(step_for_temp(&t.cpu_upt, 100), 11);
    }

    #[test]
    fn duty_within_tolerance_counts_as_obeyed() {
        let t = table();
        let want = t.cpu_duty[step_for_temp(&t.cpu_upt, 50)];
        assert!(firmware_follows_table(&t, 50, (want, want)));
        assert!(firmware_follows_table(&t, 50, (want + 6, want)));
        assert!(!firmware_follows_table(&t, 50, (want + 7, want)));
    }

    #[test]
    fn ramp_lag_still_counts_as_obeyed() {
        // Regression: a switch taken right after a threshold crossing evaluated the
        // live duty against the new step only and declared the firmware "ignored"
        // (57 °C, step asks 0x46, fan still held 0x3c from the 55 °C step).
        let t = table();
        let hot = step_for_temp(&t.cpu_upt, 57);
        assert!(t.cpu_duty[hot] > t.cpu_duty[hot - 1], "test only makes sense when the next step is louder");
        let lagging = t.cpu_duty[hot - 1];
        assert!(firmware_follows_table(&t, 57, (lagging, lagging)));
        // ...but the idle duty of a firmware that ignores the table is still caught.
        assert!(!firmware_follows_table(&t, 57, (0x32, 0x32)));
    }

    #[test]
    fn idle_duty_of_this_firmware_reads_as_ignored() {
        // The whole point: a flat 0x60 table is thrown away by an EC that sits at 0x32.
        let t = FAN_TABLE_GAMING.clone();
        assert!(!firmware_follows_table(&t, 50, (0x32, 0x32)));
    }
}
