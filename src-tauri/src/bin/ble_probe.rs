#[cfg(windows)]
use windows::core::GUID;
#[cfg(windows)]
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};

#[cfg(windows)]
const NUS_SERVICE_UUID: GUID = GUID::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);
const WATER_COOLER_MAC: u64 = 0xcacbc3029718;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== OpenRevo Native BLE Water Cooler Probe ===");
    println!("Resolving device CA:CB:C3:02:97:18...");

    let op = BluetoothLEDevice::FromBluetoothAddressAsync(WATER_COOLER_MAC)?;
    let dev = op.get()?;
    println!("Device: {} (Status: {:?})", dev.Name()?, dev.ConnectionStatus()?);

    // Try Uncached discovery of GATT services
    println!("Querying NUS service (Uncached)...");
    let svc_op = dev.GetGattServicesForUuidWithCacheModeAsync(NUS_SERVICE_UUID, BluetoothCacheMode::Uncached)?;
    let svc_res = svc_op.get()?;
    println!("Services query status: {:?}", svc_res.Status()?);

    let services = svc_res.Services()?;
    println!("Found {} services", services.Size()?);

    if services.Size()? > 0 {
        let service = services.GetAt(0)?;
        let chars_op = service.GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)?;
        let chars_res = chars_op.get()?;
        println!("Chars query status: {:?}", chars_res.Status()?);
        let chars = chars_res.Characteristics()?;
        println!("Chars found: {}", chars.Size()?);
        for i in 0..chars.Size()? {
            let c = chars.GetAt(i)?;
            println!("  Char: {:?}", c.Uuid()?);
        }
    } else {
        // Try Cached fallback
        println!("Trying Cached fallback...");
        let svc_op = dev.GetGattServicesForUuidWithCacheModeAsync(NUS_SERVICE_UUID, BluetoothCacheMode::Cached)?;
        let svc_res = svc_op.get()?;
        let services = svc_res.Services()?;
        println!("Cached services: {}", services.Size()?);
    }

    Ok(())
}

/// The probe drives the WinRT GATT stack directly; the BlueZ backend is not
/// wired into it yet, so this build just reports that.
#[cfg(not(windows))]
fn main() {
    println!("=== OpenRevo BLE Water Cooler Probe ===");
    println!("This build has no WinRT GATT client; the BlueZ backend is a P3 item.");
    println!("Target device: CA:CB:C3:02:97:18 (NUS service 6e400001-b5a3-f393-e0a9-e50e24dcca9e)");
}
