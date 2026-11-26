fn main() {
    for m in openrevo_core::system::display::get_all_monitors() {
        println!("{} | {} | {}x{} @{} Hz | rates {:?}", m.device_name, m.friendly_name, m.width, m.height, m.current_hz, m.supported_rates);
    }
    println!("get_refresh_rate() = {}", openrevo_core::system::display::get_refresh_rate());
}
