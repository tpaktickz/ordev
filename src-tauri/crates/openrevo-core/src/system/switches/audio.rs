#[cfg(windows)]
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
#[cfg(windows)]
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

#[cfg(windows)]
/// Read real-time microphone active status (true = active/unmuted, false = muted)
pub fn get_microphone_state() -> Option<bool> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let muted = endpoint_volume.GetMute().ok()?;
        Some(!muted.as_bool())
    }
}

#[cfg(windows)]
/// Set microphone mute state (enable: true = unmute/active, false = mute)
pub fn set_microphone_state(enable: bool) -> Result<bool, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("Failed CoCreateInstance: {:?}", e))?;
        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|e| format!("Failed GetDefaultAudioEndpoint: {:?}", e))?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Failed Activate endpoint: {:?}", e))?;
        let muted = !enable;
        endpoint_volume.SetMute(windows::Win32::Foundation::BOOL::from(muted), std::ptr::null())
            .map_err(|e| format!("Failed SetMute: {:?}", e))?;
        Ok(enable)
    }
}

// ---------------------------------------------------------------------------
// Linux: PipeWire (`wpctl`) with a PulseAudio (`pactl`) fallback. Both are the
// standard session-level APIs; no desktop-specific tool is involved.
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn mixer_has_wpctl() -> bool {
    crate::pal::process::command("wpctl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read real-time microphone active status (true = active/unmuted, false = muted)
#[cfg(not(windows))]
pub fn get_microphone_state() -> Option<bool> {
    if mixer_has_wpctl() {
        let out = crate::pal::process::command("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        return Some(!text.contains("MUTED"));
    }
    let out = crate::pal::process::command("pactl")
        .args(["get-source-mute", "@DEFAULT_SOURCE@"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    Some(text.contains("no"))
}

/// Set microphone mute state (enable: true = unmute/active, false = mute)
#[cfg(not(windows))]
pub fn set_microphone_state(enable: bool) -> Result<bool, String> {
    let flag = if enable { "0" } else { "1" };
    let (program, args) = if mixer_has_wpctl() {
        ("wpctl", vec!["set-mute", "@DEFAULT_AUDIO_SOURCE@", flag])
    } else {
        ("pactl", vec!["set-source-mute", "@DEFAULT_SOURCE@", flag])
    };
    let out = crate::pal::process::command(program)
        .args(&args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(enable)
}
