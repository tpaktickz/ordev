//! Reads the live RamFan table straight out of the EC and replays it against the
//! current temperatures, so a spontaneous fan ramp can be attributed either to the
//! injected curve (upt/duty/gain) or to the EC's own ramp rate.
//!
//! Usage: `ec_fan_probe` (one shot), `ec_fan_probe --loop 2 120` (watch).

use std::fs;
use std::thread;
use std::time::Duration;

use openrevo_core::driver::wmi_ec as ec;

const CPU_UPT: u16 = 0x0F00;
const CPU_DOWNT: u16 = 0x0F10;
const CPU_DUTY: u16 = 0x0F20;
const GPU_UPT: u16 = 0x0F30;
const GPU_DOWNT: u16 = 0x0F40;
const GPU_DUTY: u16 = 0x0F50;
const TABLE_CTRL: u16 = 0x0F5F;

fn read_table(base: u16, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ec::wmi_read_ec(base + i as u16).unwrap_or(0xFF))
        .collect()
}

fn cpu_temp() -> Option<f32> {
    for entry in fs::read_dir("/sys/class/hwmon").ok()? {
        let dir = entry.ok()?.path();
        let name = fs::read_to_string(dir.join("name")).unwrap_or_default();
        if !matches!(name.trim(), "coretemp" | "k10temp" | "zenpower") {
            continue;
        }
        for i in 0..16 {
            let label = fs::read_to_string(dir.join(format!("temp{i}_label"))).unwrap_or_default();
            let value = fs::read_to_string(dir.join(format!("temp{i}_input"))).ok();
            if let Some(v) = value {
                let c = v.trim().parse::<f32>().ok()? / 1000.0;
                if c > 1.0 && c < 125.0 {
                    if label.trim() == "Package id 0" {
                        return Some(c);
                    }
                }
            }
        }
    }
    None
}

fn gpu_temp() -> Option<f32> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().split_whitespace().next()?.parse().ok()
}

fn pick_step(upt: &[u8], temp: f32) -> usize {
    let mut step = 0;
    for (i, t) in upt.iter().enumerate() {
        if temp >= *t as f32 {
            step = i;
        }
    }
    step
}

fn sample() {
    let c_upt = read_table(CPU_UPT, 16);
    let c_downt = read_table(CPU_DOWNT, 16);
    let c_duty = read_table(CPU_DUTY, 16);
    let g_upt = read_table(GPU_UPT, 16);
    let g_downt = read_table(GPU_DOWNT, 16);
    let g_duty = read_table(GPU_DUTY, 15);
    let ctrl = ec::wmi_read_ec(TABLE_CTRL).unwrap_or(0xFF);
    let ramp = ec::wmi_read_ec(ec::ADDR_FAN_RAMP_RATE).unwrap_or(0xFF);
    let duty_l = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_L_DUTY_BYTE).unwrap_or(0xFF);
    let duty_r = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_R_DUTY_BYTE).unwrap_or(0xFF);
    let t_cpu = cpu_temp();
    let t_gpu = gpu_temp();

    let step_c = t_cpu.map(|t| pick_step(&c_upt, t));
    let step_g = t_gpu.map(|t| pick_step(&g_upt, t));

    println!("--- fan table");
    println!("cpu_upt  {c_upt:?}");
    println!("cpu_downt{c_downt:?}");
    println!("cpu_duty {c_duty:?}");
    println!("gpu_upt  {g_upt:?}");
    println!("gpu_downt{g_downt:?}");
    println!("gpu_duty {g_duty:?}");
    println!("table_ctrl={ctrl:#04x} ramp_rate={ramp:#04x}");
    println!(
        "temps cpu={t_cpu:?} gpu={t_gpu:?} -> step cpu={step_c:?} gpu={step_g:?} \
         expect duty cpu={:?} gpu={:?}",
        step_c.map(|s| c_duty[s]),
        step_g.map(|s| g_duty[s.min(g_duty.len() - 1)])
    );
    println!("live duty L={duty_l:#04x} ({}) R={duty_r:#04x} ({})", duty_l, duty_r);
}

fn watch(total: u64, interval: u64) {
    let mut elapsed = 0;
    while elapsed < total {
        let duty_l = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_L_DUTY_BYTE).unwrap_or(0xFF);
        let duty_r = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_R_DUTY_BYTE).unwrap_or(0xFF);
        let rpm = (ec::wmi_read_ec(ec::ADDR_MAIN_FAN_RPM_B1).unwrap_or(0) as u16) << 8
            | ec::wmi_read_ec(ec::ADDR_MAIN_FAN_RPM_B2).unwrap_or(0) as u16;
        let ctrl = ec::wmi_read_ec(TABLE_CTRL).unwrap_or(0xFF);
        let mode = ec::wmi_read_ec(ec::ADDR_MODE_INDEX).unwrap_or(0xFF);
        let submode = ec::wmi_read_ec(ec::ADDR_TURBO_SUBMODE).unwrap_or(0xFF);
        let tcc = ec::wmi_read_ec(ec::ADDR_TURBO_TCC).unwrap_or(0xFF);
        let pl1 = ec::wmi_read_ec(ec::ADDR_CUSTOM_PL1).unwrap_or(0xFF);
        let boost = ec::wmi_read_ec(ec::ADDR_FAN_BOOST_CLAMP).unwrap_or(0xFF);
        println!(
            "t={elapsed:>4}s cpu={:?} gpu={:?} duty={duty_l:#04x}/{duty_r:#04x} rpm={rpm} \
             ctrl={ctrl:#04x} mode={mode:#04x} sub={submode:#04x} tcc={tcc:#04x} pl1={pl1} boost={boost:#04x}",
            cpu_temp(),
            gpu_temp()
        );
        thread::sleep(Duration::from_secs(interval));
        elapsed += interval;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--ctrl" {
        let raw = args[2].trim_start_matches("0x");
        let val = u8::from_str_radix(raw, 16).unwrap_or(0xA0);
        let ok = ec::wmi_write_ec(TABLE_CTRL, val);
        println!("write table_ctrl {val:#04x} -> {ok}");
        if args.len() >= 5 {
            watch(args[3].parse().unwrap_or(60), args[4].parse().unwrap_or(2));
        }
        return;
    }
    if args.len() >= 4 && args[1] == "--set" {
        let addr = u16::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap_or(0);
        let val = u8::from_str_radix(args[3].trim_start_matches("0x"), 16).unwrap_or(0);
        let ok = ec::wmi_write_ec(addr, val);
        println!("write {addr:#06x}={val:#04x} -> {ok} (readback {:?})", ec::wmi_read_ec(addr));
        return;
    }
    if args.len() >= 3 && args[1] == "--get" {
        let addr = u16::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap_or(0);
        for name in [] as [&str; 0] {
            let _ = name;
        }
        println!("{addr:#06x} = {:?}", ec::wmi_read_ec(addr).map(|v| format!("{v:#04x}")));
        for addr in [0x727u16, 0x706, 0x741, 0x7AB, 0x767, 0x751, 0x735, 0x7DA, 0x726, 0x7B9, 0x7A6] {
            println!("  {addr:#06x} = {:?}", ec::wmi_read_ec(addr).map(|v| format!("{v:#04x}")));
        }
        return;
    }
    if args.len() >= 2 && args[1] == "--watch" {
        watch(
            args.get(2).and_then(|v| v.parse().ok()).unwrap_or(60),
            args.get(3).and_then(|v| v.parse().ok()).unwrap_or(2),
        );
        return;
    }
    if args.len() >= 4 && args[1] == "--loop" {
        let interval: u64 = args[2].parse().unwrap_or(2);
        let total: u64 = args[3].parse().unwrap_or(60);
        let report_table = true;
        let mut elapsed = 0;
        let mut printed = !report_table;
        while elapsed < total {
            if !printed {
                sample();
                printed = true;
            } else {
                let duty_l = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_L_DUTY_BYTE).unwrap_or(0xFF);
                let duty_r = ec::wmi_read_ec(ec::ADDR_MAIN_FAN_R_DUTY_BYTE).unwrap_or(0xFF);
                let rpm = (ec::wmi_read_ec(ec::ADDR_MAIN_FAN_RPM_B1).unwrap_or(0) as u16) << 8
                    | ec::wmi_read_ec(ec::ADDR_MAIN_FAN_RPM_B2).unwrap_or(0) as u16;
                println!(
                    "t={elapsed:>4}s cpu={:?} gpu={:?} duty_l={duty_l} duty_r={duty_r} rpm={rpm}",
                    cpu_temp(),
                    gpu_temp()
                );
            }
            thread::sleep(Duration::from_secs(interval));
            elapsed += interval;
        }
        return;
    }
    sample();
}
