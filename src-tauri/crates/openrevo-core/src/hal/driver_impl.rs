// OpenRevo Concrete Hardware HAL Implementation
// Bridges HAL trait with Mechrevo / Uniwill EC RAM primitives and Hardware Vault.

use super::traits::*;
use super::vault::{ec, mask};
use crate::driver::wmi_ec;

pub struct MechrevoHALImpl;

impl MechrevoHALImpl {
    pub fn new() -> Self {
        Self
    }
}

impl MechrevoHardwareHAL for MechrevoHALImpl {
    fn set_performance_mode(&self, mode: HardwarePowerMode) -> Result<(), HalError> {
        let mode_u8 = mode as u8;
        crate::system::power_mode::apply_full_power_mode(mode_u8);
        Ok(())
    }

    fn set_battery_health(&self, mode: BatteryHealthMode) -> Result<(), HalError> {
        let (limit_up, mode_byte) = match mode {
            BatteryHealthMode::LongLife => (100u8, mask::BATTERY_MODE_LONGLIFE),
            BatteryHealthMode::Balance => (80u8, mask::BATTERY_MODE_BALANCE),
            BatteryHealthMode::Station => (60u8, mask::BATTERY_MODE_STATION),
        };

        let down_limit = if limit_up > 5 { limit_up - 5 } else { limit_up };

        // 1. Write cutoff limits (Upper, Lower)
        let ok1 = wmi_ec::wmi_write_ec(ec::ADDR_BATTERY_LIMIT_UP, limit_up);
        let ok2 = wmi_ec::wmi_write_ec(ec::ADDR_BATTERY_LIMIT_DOWN, down_limit);

        // 2. Atomically update battery protection mode preserving touchpad bit
        let ok3 = wmi_ec::wmi_modify_ec(ec::ADDR_BATTERY_PROTECTION_MODE, |cur_bat_mode| {
            let preserved_touchpad = cur_bat_mode & mask::TOUCHPAD_BITMASK;
            (mode_byte & !mask::TOUCHPAD_BITMASK) | preserved_touchpad
        }).is_some();

        if ok1 && ok2 && ok3 {
            Ok(())
        } else {
            Err(HalError::RegisterAccessFailed("Failed to update battery health registers".to_string()))
        }
    }

    fn set_fan_boost(&self, active: bool) -> Result<(), HalError> {
        // Prefer the kernel driver: when `tongfang-mifs-wmi` is loaded it owns the
        // EC mailbox, and a competing write from userspace risks a lost command.
        if crate::pal::tongfang::driver_present() && crate::pal::tongfang::set_fan_boost(active) {
            return Ok(());
        }
        let success = wmi_ec::set_fan_boost_hardware(active);
        if success {
            Ok(())
        } else {
            Err(HalError::RegisterAccessFailed("Failed to toggle Fan Boost".to_string()))
        }
    }

    fn set_device_switch(&self, switch: DeviceSwitchType, enable: bool) -> Result<(), HalError> {
        let switch_name = match switch {
            DeviceSwitchType::WinKeyLock => "win_lock",
            DeviceSwitchType::Touchpad => "touchpad",
            DeviceSwitchType::CameraPower => "camera",
            DeviceSwitchType::FnLock => "fn_lock",
            DeviceSwitchType::AcPowerRecovery => "ac_recovery",
            DeviceSwitchType::PowerOffUsbCharge => "usb_charge",
        };

        crate::system::device_switches::set_switch(switch_name, enable)
            .map(|_| ())
            .map_err(|e| HalError::RegisterAccessFailed(e))
    }

    fn read_fan_rpm(&self) -> Result<(u16, u16), HalError> {
        // hwmon is one read per fan and cannot collide with our own EC traffic.
        if let Some(rpm) = crate::pal::tongfang::read_fan_rpm() {
            return Ok(rpm);
        }
        let registers = [
            ec::ADDR_MAIN_FAN_RPM_B1,
            ec::ADDR_MAIN_FAN_RPM_B2,
            ec::ADDR_SECOND_FAN_RPM_B1,
            ec::ADDR_SECOND_FAN_RPM_B2,
        ];
        if let Some(bytes) = wmi_ec::wmi_batch_read_ec(&registers) {
            if bytes.len() == 4 {
                let cpu_rpm = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
                let gpu_rpm = ((bytes[2] as u16) << 8) | (bytes[3] as u16);
                return Ok((cpu_rpm, gpu_rpm));
            }
        }
        Err(HalError::RegisterAccessFailed("Failed to batch read fan RPM".to_string()))
    }

    fn set_monochrome_backlight(&self, level: u8) -> Result<(), HalError> {
        if crate::pal::tongfang::set_backlight_level(level) {
            return Ok(());
        }
        Err(HalError::UnsupportedModel(
            "No LED class node for the keyboard backlight (is tongfang-mifs-wmi loaded?)".to_string(),
        ))
    }

    fn get_monochrome_backlight(&self) -> Option<u8> {
        crate::pal::tongfang::read_backlight_level()
    }

    fn is_hardware_supported(&self) -> bool {
        self.probe()
    }
}

impl HardwareDriver for MechrevoHALImpl {
    fn name(&self) -> &'static str {
        "Tongfang-ACPI"
    }

    fn kind(&self) -> DriverKind {
        DriverKind::TongfangAcpi
    }

    fn probe(&self) -> bool {
        // The kernel module is proof of both halves at once: it only binds on a
        // matching chassis, and it means the EC is reachable through sysfs. Skip
        // the PROJECT_ID read in that case — it would race the kernel driver.
        if crate::pal::tongfang::driver_present() {
            crate::log_info!("HAL", "tongfang-mifs-wmi present: {}", crate::pal::tongfang::describe());
            return true;
        }
        crate::hal::platform_probe::is_tongfang_family()
            && wmi_ec::wmi_read_ec(ec::ADDR_PROJECT_ID).is_some()
    }

    fn get_performance_mode(&self) -> Result<HardwarePowerMode, HalError> {
        let cfg = crate::system::config::load_config();
        match cfg.power_mode {
            1 => Ok(HardwarePowerMode::Office),
            2 => Ok(HardwarePowerMode::Balance),
            3 => Ok(HardwarePowerMode::Turbo),
            4 => Ok(HardwarePowerMode::Custom),
            _ => Ok(HardwarePowerMode::Balance),
        }
    }

    fn set_keyboard_effect(&self, effect: u8, r: u8, g: u8, b: u8) -> Result<(), HalError> {
        // Kernel ABI effect ids: 0 off, 1 cyclic, 2 fixed, 3 custom. Anything else
        // falls through to the EC colour write so per-key boards keep working.
        let mapped = match effect {
            0 => Some(0),
            1 => Some(1),
            3 => Some(3),
            _ => Some(2),
        };
        if let Some(id) = mapped {
            if crate::pal::tongfang::set_keyboard_effect(id) {
                return Ok(());
            }
        }
        self.set_keyboard_rgb(r, g, b)
    }

    fn read_cpu_temp(&self) -> Result<u8, HalError> {
        if let Some(temp) = crate::pal::tongfang::read_cpu_temp_celsius() {
            return Ok(temp);
        }
        let temp = crate::system::cpu_monitor::get_cpu_monitor().get_temp();
        if temp > 0 && temp < 125 {
            Ok(temp as u8)
        } else {
            Ok(50)
        }
    }
}
