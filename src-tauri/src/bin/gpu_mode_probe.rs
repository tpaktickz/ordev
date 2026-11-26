fn main() {
    let hw = openrevo_core::driver::uefi_nvram::read_gpu_mode();
    println!("read_gpu_mode => mode={} raw={} hw_read={} err={:?}", hw.current_mode, hw.raw_value, hw.hardware_read, hw.error);
    let nv = openrevo_core::driver::uefi_nvram::read_active_nvram();
    match nv {
        Ok(buf) => println!("nvram ok var={} len={} byte98={:#04x}", buf.var_name, buf.bytes_read, buf.buf.get(98).copied().unwrap_or(0xFF)),
        Err(e) => println!("nvram err: {}", e),
    }
    println!("is_amd={} legacy_dual={}", openrevo_core::system::cpu_monitor::is_amd_cpu(), openrevo_core::driver::wmi_ec::is_legacy_dual_mode_ec());
}
