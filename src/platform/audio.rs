//! Session audio controls: microphone muting, gain control, and output volume.
//!
//! Provides a unified, toolkit-independent interface for querying and controlling
//! host audio devices (default microphone and speaker/output sink) across Linux
//! (PipeWire/WirePlumber, PulseAudio, and ALSA), macOS, and Windows.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Once;

static INIT: Once = Once::new();
static MIC_MUTED: AtomicBool = AtomicBool::new(false);
static MIC_VOLUME_LEVEL: AtomicU32 = AtomicU32::new(100);
static VOLUME_LEVEL: AtomicU32 = AtomicU32::new(100);

fn ensure_initialized() {
    INIT.call_once(|| {
        if let Some(muted) = os::query_mic_muted() {
            MIC_MUTED.store(muted, Ordering::Relaxed);
        }
        if let Some(mic_vol) = os::query_mic_volume() {
            MIC_VOLUME_LEVEL.store(mic_vol.clamp(0, 150), Ordering::Relaxed);
        }
        if let Some(vol) = os::query_volume() {
            VOLUME_LEVEL.store(vol.min(100), Ordering::Relaxed);
        }
    });
}

/// Checks whether the microphone is currently muted.
#[must_use]
pub fn is_mic_muted() -> bool {
    ensure_initialized();
    MIC_MUTED.load(Ordering::Relaxed)
}

/// Sets the microphone mute state. Returns the new mute state.
pub fn set_mic_muted(muted: bool) -> bool {
    ensure_initialized();
    MIC_MUTED.store(muted, Ordering::Relaxed);
    os::set_mic_muted(muted);
    if !muted {
        // When unmuting, ensure microphone volume is at least 100% to prevent low recording volume
        let current = MIC_VOLUME_LEVEL.load(Ordering::Relaxed);
        if current < 80 {
            set_mic_volume(100);
        }
    }
    muted
}

/// Toggles the microphone mute state. Returns the new mute state.
pub fn toggle_mic_muted() -> bool {
    let new_state = !is_mic_muted();
    set_mic_muted(new_state)
}

/// Gets the session microphone input gain / volume (0 to 150, standard 100).
#[must_use]
pub fn get_mic_volume() -> u32 {
    ensure_initialized();
    MIC_VOLUME_LEVEL.load(Ordering::Relaxed)
}

/// Sets the session microphone input gain / volume (0 to 150). Returns the applied volume level.
pub fn set_mic_volume(level: u32) -> u32 {
    ensure_initialized();
    let clamped = level.clamp(0, 150);
    MIC_VOLUME_LEVEL.store(clamped, Ordering::Relaxed);
    os::set_mic_volume(clamped);
    clamped
}

/// Ensures that the default microphone is unmuted and its recording volume is at an
/// optimal level (at least 100%), preventing inaudible or low-volume recordings in remote sessions.
pub fn ensure_mic_optimal_gain() -> u32 {
    ensure_initialized();
    if is_mic_muted() {
        set_mic_muted(false);
    }
    let current = get_mic_volume();
    if current < 80 {
        set_mic_volume(100)
    } else {
        current
    }
}

/// Gets the session output volume (0 to 100).
#[must_use]
pub fn get_volume() -> u32 {
    ensure_initialized();
    VOLUME_LEVEL.load(Ordering::Relaxed)
}

/// Sets the session output volume (0 to 100). Returns the clamped volume level.
pub fn set_volume(level: u32) -> u32 {
    ensure_initialized();
    let clamped = level.min(100);
    VOLUME_LEVEL.store(clamped, Ordering::Relaxed);
    os::set_volume(clamped);
    clamped
}

// -----------------------------------------------------------------------------
// Linux implementation (PipeWire / WirePlumber `wpctl`, PulseAudio `pactl`, ALSA `amixer`)
// -----------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod os {
    use std::process::Command;

    pub fn query_mic_muted() -> Option<bool> {
        // Try WirePlumber (wpctl) first
        if let Ok(out) = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                return Some(text.contains("[MUTED]"));
            }
        }

        // Fallback to PulseAudio (pactl)
        if let Ok(out) = Command::new("pactl")
            .args(["get-source-mute", "@DEFAULT_SOURCE@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
                return Some(text.contains("yes"));
            }
        }

        None
    }

    pub fn set_mic_muted(muted: bool) {
        let flag = if muted { "1" } else { "0" };
        let _ = Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", flag])
            .output();
        let _ = Command::new("pactl")
            .args(["set-source-mute", "@DEFAULT_SOURCE@", flag])
            .output();
    }

    pub fn query_mic_volume() -> Option<u32> {
        // Try WirePlumber (wpctl)
        if let Ok(out) = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                // format: "Volume: 0.54" or "Volume: 1.00 [MUTED]"
                if let Some(vol_str) = text.split_whitespace().nth(1) {
                    if let Ok(val) = vol_str.parse::<f64>() {
                        return Some((val * 100.0).round().clamp(0.0, 150.0) as u32);
                    }
                }
            }
        }

        // Fallback to PulseAudio (pactl)
        if let Ok(out) = Command::new("pactl")
            .args(["get-source-volume", "@DEFAULT_SOURCE@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for part in text.split('/') {
                    let trimmed = part.trim().trim_end_matches('%');
                    if let Ok(v) = trimmed.parse::<u32>() {
                        return Some(v.min(150));
                    }
                }
            }
        }

        // Fallback to ALSA (amixer)
        if let Ok(out) = Command::new("amixer").args(["sget", "Capture"]).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(start) = text.find('[') {
                    if let Some(end) = text[start..].find("%]") {
                        if let Ok(v) = text[start + 1..start + end].trim().parse::<u32>() {
                            return Some(v.min(100));
                        }
                    }
                }
            }
        }

        None
    }

    pub fn set_mic_volume(level: u32) {
        let clamped = level.clamp(0, 150);
        let float_val = format!("{:.2}", f64::from(clamped) / 100.0);
        let pct = format!("{clamped}%");

        let _ = Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SOURCE@", &float_val])
            .output();
        let _ = Command::new("pactl")
            .args(["set-source-volume", "@DEFAULT_SOURCE@", &pct])
            .output();
        let _ = Command::new("amixer")
            .args(["sset", "Capture", &pct])
            .output();
    }

    pub fn query_volume() -> Option<u32> {
        // Try WirePlumber (wpctl)
        if let Ok(out) = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                // format: "Volume: 0.75" or "Volume: 1.00 [MUTED]"
                if let Some(vol_str) = text.split_whitespace().nth(1) {
                    if let Ok(val) = vol_str.parse::<f64>() {
                        return Some((val * 100.0).round().clamp(0.0, 100.0) as u32);
                    }
                }
            }
        }

        // Fallback to PulseAudio (pactl)
        if let Ok(out) = Command::new("pactl")
            .args(["get-sink-volume", "@DEFAULT_SINK@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for part in text.split('/') {
                    let trimmed = part.trim().trim_end_matches('%');
                    if let Ok(v) = trimmed.parse::<u32>() {
                        return Some(v.min(100));
                    }
                }
            }
        }

        None
    }

    pub fn set_volume(level: u32) {
        let clamped = level.min(100);
        let float_val = format!("{:.2}", (clamped as f64) / 100.0);
        let pct = format!("{clamped}%");

        let _ = Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &float_val])
            .output();
        let _ = Command::new("pactl")
            .args(["set-sink-volume", "@DEFAULT_SINK@", &pct])
            .output();
    }
}

// -----------------------------------------------------------------------------
// macOS implementation (AppleScript / osascript)
// -----------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod os {
    use std::process::Command;

    pub fn query_mic_muted() -> Option<bool> {
        if let Ok(out) = Command::new("osascript")
            .args(["-e", "input volume of (get volume settings)"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(val) = text.parse::<u32>() {
                    return Some(val == 0);
                }
            }
        }
        None
    }

    pub fn set_mic_muted(muted: bool) {
        let val = if muted { "0" } else { "100" };
        let script = format!("set volume input volume {val}");
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }

    pub fn query_mic_volume() -> Option<u32> {
        if let Ok(out) = Command::new("osascript")
            .args(["-e", "input volume of (get volume settings)"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(val) = text.parse::<u32>() {
                    return Some(val.min(100));
                }
            }
        }
        None
    }

    pub fn set_mic_volume(level: u32) {
        let script = format!("set volume input volume {}", level.min(100));
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }

    pub fn query_volume() -> Option<u32> {
        if let Ok(out) = Command::new("osascript")
            .args(["-e", "output volume of (get volume settings)"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(val) = text.parse::<u32>() {
                    return Some(val.min(100));
                }
            }
        }
        None
    }

    pub fn set_volume(level: u32) {
        let script = format!("set volume output volume {}", level.min(100));
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }
}

// -----------------------------------------------------------------------------
// Windows implementation (PowerShell / WASAPI Core Audio)
// -----------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod os {
    use std::process::Command;

    const WASAPI_MIC_HELPER: &str = r#"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
[Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IAudioEndpointVolume {
    int f(); int g(); int h(); int i();
    int SetMasterVolumeLevelScalar(float fLevel, System.Guid pguidEventContext);
    int j();
    int GetMasterVolumeLevelScalar(out float pfLevel);
    int k(); int l(); int m(); int n();
    int SetMute([MarshalAs(UnmanagedType.Bool)] bool bMute, System.Guid pguidEventContext);
    int GetMute(out bool pbMute);
}
[Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IMMDevice { int Activate(ref System.Guid id, int clsCtx, int p, out IAudioEndpointVolume aev); }
[Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IMMDeviceEnumerator { int f(); int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice ep); }
[ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] public class MMDeviceEnumeratorComObject { }
public class AudioMic {
    static IAudioEndpointVolume Get() {
        var e = new MMDeviceEnumeratorComObject() as IMMDeviceEnumerator;
        IMMDevice d; e.GetDefaultAudioEndpoint(1, 1, out d);
        IAudioEndpointVolume ep; var g = typeof(IAudioEndpointVolume).GUID;
        d.Activate(ref g, 23, 0, out ep); return ep;
    }
    public static int GetVolume() { float v = 0; Get().GetMasterVolumeLevelScalar(out v); return (int)Math.Round(v * 100); }
    public static void SetVolume(int vol) { Get().SetMasterVolumeLevelScalar(vol / 100f, Guid.Empty); }
    public static bool GetMute() { bool m = false; Get().GetMute(out m); return m; }
    public static void SetMute(bool mute) { Get().SetMute(mute, Guid.Empty); }
}
'@
"#;

    pub fn query_mic_muted() -> Option<bool> {
        let script = format!("{WASAPI_MIC_HELPER}; [AudioMic]::GetMute()");
        if let Ok(out) = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
                if text == "true" {
                    return Some(true);
                } else if text == "false" {
                    return Some(false);
                }
            }
        }
        None
    }

    pub fn set_mic_muted(muted: bool) {
        let flag = if muted { "$true" } else { "$false" };
        let script = format!("{WASAPI_MIC_HELPER}; [AudioMic]::SetMute({flag})");
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output();
    }

    pub fn query_mic_volume() -> Option<u32> {
        let script = format!("{WASAPI_MIC_HELPER}; [AudioMic]::GetVolume()");
        if let Ok(out) = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(val) = text.parse::<u32>() {
                    return Some(val.min(100));
                }
            }
        }
        None
    }

    pub fn set_mic_volume(level: u32) {
        let clamped = level.min(100);
        let script = format!("{WASAPI_MIC_HELPER}; [AudioMic]::SetVolume({clamped})");
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output();
    }

    pub fn query_volume() -> Option<u32> {
        None
    }

    pub fn set_volume(level: u32) {
        let vol_fraction = (level.min(100) as f64) / 100.0;
        let script = format!(
            "$vol = [math]::Round({vol_fraction}, 2); \
             [Audio]::SetVolume($vol)"
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output();
    }
}

// -----------------------------------------------------------------------------
// Fallback stub for other platforms
// -----------------------------------------------------------------------------
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod os {
    pub fn query_mic_muted() -> Option<bool> {
        None
    }
    pub fn set_mic_muted(_muted: bool) {}
    pub fn query_mic_volume() -> Option<u32> {
        None
    }
    pub fn set_mic_volume(_level: u32) {}
    pub fn query_volume() -> Option<u32> {
        None
    }
    pub fn set_volume(_level: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_controls_work() {
        // Output volume clamping
        assert_eq!(set_volume(150), 100);
        assert_eq!(get_volume(), 100);
        assert_eq!(set_volume(50), 50);
        assert_eq!(get_volume(), 50);

        // Mic volume clamping
        assert_eq!(set_mic_volume(200), 150);
        assert_eq!(get_mic_volume(), 150);
        assert_eq!(set_mic_volume(50), 50);
        assert_eq!(get_mic_volume(), 50);

        // Mic mute toggle
        let initial = is_mic_muted();
        let toggled = toggle_mic_muted();
        assert_eq!(toggled, !initial);
        let restored = toggle_mic_muted();
        assert_eq!(restored, initial);

        // Optimal gain ensure
        let optimal = ensure_mic_optimal_gain();
        assert!(optimal >= 80);
        assert!(!is_mic_muted());
    }
}
