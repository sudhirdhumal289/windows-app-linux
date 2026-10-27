//! Session audio controls: microphone muting and output volume.
//!
//! Provides a unified, toolkit-independent interface for querying and controlling
//! host audio devices (default microphone and speaker/output sink) across Linux
//! (PipeWire/WirePlumber and PulseAudio), macOS, and Windows.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Once;

static INIT: Once = Once::new();
static MIC_MUTED: AtomicBool = AtomicBool::new(false);
static VOLUME_LEVEL: AtomicU32 = AtomicU32::new(100);

fn ensure_initialized() {
    INIT.call_once(|| {
        if let Some(muted) = os::query_mic_muted() {
            MIC_MUTED.store(muted, Ordering::Relaxed);
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
    muted
}

/// Toggles the microphone mute state. Returns the new mute state.
pub fn toggle_mic_muted() -> bool {
    let new_state = !is_mic_muted();
    set_mic_muted(new_state)
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
// Linux implementation (PipeWire / WirePlumber `wpctl` and PulseAudio `pactl`)
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
        let val = if muted { "0" } else { "75" };
        let script = format!("set volume input volume {val}");
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
// Windows implementation (PowerShell / WScript / Core Audio)
// -----------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod os {
    use std::process::Command;

    pub fn query_mic_muted() -> Option<bool> {
        None
    }

    pub fn set_mic_muted(muted: bool) {
        // Toggle system mic mute using PowerShell WScript keystroke simulation
        let script = if muted {
            "$w = New-Object -ComObject WScript.Shell; $w.SendKeys([char]173)"
        } else {
            "$w = New-Object -ComObject WScript.Shell; $w.SendKeys([char]173)"
        };
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
    pub fn query_volume() -> Option<u32> {
        None
    }
    pub fn set_volume(_level: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_mute_toggle_works() {
        let initial = is_mic_muted();
        let toggled = toggle_mic_muted();
        assert_eq!(toggled, !initial);
        let restored = toggle_mic_muted();
        assert_eq!(restored, initial);
    }

    #[test]
    fn volume_clamping_works() {
        assert_eq!(set_volume(150), 100);
        assert_eq!(get_volume(), 100);
        assert_eq!(set_volume(50), 50);
        assert_eq!(get_volume(), 50);
    }
}
