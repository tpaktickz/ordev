//! Prints the EC ground truth the dashboard consumes, next to the raw register
//! bytes it was decoded from, so a panel mismatch can be traced to either the
//! decode or the transport.

use open_revo_lib::driver::wmi_ec::wmi_read_ec;
use open_revo_lib::system::hw_truth;

fn dump(label: &str, addr: u16) {
    match wmi_read_ec(addr) {
        Some(v) => println!("  raw {label:<22} 0x{addr:04X} = 0x{v:02X} ({v})"),
        None => println!("  raw {label:<22} 0x{addr:04X} = <read failed>"),
    }
}

fn main() {
    let truth = hw_truth::read();
    println!("hw_truth::read()");
    println!("  power_mode     {:?}  (1 Office / 2 Balance / 3 Turbo / 4 Custom)", truth.power_mode);
    println!("  pl1            {:?} W", truth.pl1);
    println!("  tgp            {:?} W", truth.tgp);
    println!("  battery_limit  {:?} %", truth.battery_limit);
    println!("  fan_boost      {:?}", truth.fan_boost);
    println!("  turbo_silent   {:?}", truth.turbo_silent);
    println!("  ac_recovery    {:?}", truth.ac_recovery);

    println!("source registers");
    dump("MODE_INDEX", 0x7AB);
    dump("TRIGGER_BYTE", 0x767);
    dump("CUSTOM_MODE", 0x706);
    dump("CUSTOM_PL1", 0x783);
    dump("TGP_OFFSET", 0x744);
    dump("BATTERY_LIMIT_UP", 0x7B9);
    dump("FAN_BOOST_CLAMP", 0x751);
    dump("TURBO_SUBMODE", 0x735);
    dump("AC_RECOVERY", 0x726);

    hw_truth::invalidate();
    let again = hw_truth::read();
    println!("after invalidate: power_mode {:?}", again.power_mode);
}
