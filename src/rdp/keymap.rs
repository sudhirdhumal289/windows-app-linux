//! Linux evdev key codes → RDP scancodes (scan code set 1 + `KBDEXT`).
//!
//! RDP transmits physical key positions; the remote session applies its own keyboard
//! layout. Mapping evdev codes directly avoids FreeRDP's deprecated X11 keyboard
//! helpers and works identically on X11 and Wayland. GTK/XKB keycodes are evdev + 8
//! ([`rdp_scancode_from_xkb`]).

use std::collections::BTreeSet;

/// FreeRDP's `KBDEXT`: the key is sent with the E0 prefix.
pub const KBDEXT: u32 = 0x0100;

const fn ext(code: u32) -> u32 {
    code | KBDEXT
}

/// RDP scancode for a Linux evdev key code (`KEY_*` from `linux/input-event-codes.h`).
#[must_use]
pub fn rdp_scancode_from_evdev(evdev: u32) -> Option<u32> {
    let code = match evdev {
        // ESC..KP_DOT share the PC/AT layout with scan code set 1.
        1..=83 => evdev,
        86 => 0x56,                        // 102ND
        87 => 0x57,                        // F11
        88 => 0x58,                        // F12
        89 => 0x73,                        // RO
        90 => 0x70,                        // KATAKANA
        91 => 0x70,                        // HIRAGANA
        92 => 0x79,                        // HENKAN
        93 => 0x70,                        // KATAKANAHIRAGANA
        94 => 0x7B,                        // MUHENKAN
        96 => ext(0x1C),                   // KPENTER
        97 => ext(0x1D),                   // RIGHTCTRL
        98 => ext(0x35),                   // KPSLASH
        99 => ext(0x37),                   // SYSRQ / Print Screen
        100 => ext(0x38),                  // RIGHTALT
        102 => ext(0x47),                  // HOME
        103 => ext(0x48),                  // UP
        104 => ext(0x49),                  // PAGEUP
        105 => ext(0x4B),                  // LEFT
        106 => ext(0x4D),                  // RIGHT
        107 => ext(0x4F),                  // END
        108 => ext(0x50),                  // DOWN
        109 => ext(0x51),                  // PAGEDOWN
        110 => ext(0x52),                  // INSERT
        111 => ext(0x53),                  // DELETE
        113 => ext(0x20),                  // MUTE
        114 => ext(0x2E),                  // VOLUMEDOWN
        115 => ext(0x30),                  // VOLUMEUP
        116 => ext(0x5E),                  // POWER
        117 => 0x59,                       // KPEQUAL
        119 => ext(0x46),                  // PAUSE (sent as the Pause sequence by the bridge)
        121 => 0x7E,                       // KPCOMMA
        122 => 0x72,                       // HANGEUL
        123 => 0x71,                       // HANJA
        124 => 0x7D,                       // YEN
        125 => ext(0x5B),                  // LEFTMETA
        126 => ext(0x5C),                  // RIGHTMETA
        127 => ext(0x5D),                  // COMPOSE / Menu
        128 => ext(0x68),                  // STOP (browser)
        140 => ext(0x21),                  // CALC
        142 => ext(0x5F),                  // SLEEP
        155 => ext(0x6C),                  // MAIL
        156 => ext(0x66),                  // BOOKMARKS
        158 => ext(0x6A),                  // BACK
        159 => ext(0x69),                  // FORWARD
        163 => ext(0x19),                  // NEXTSONG
        164 => ext(0x22),                  // PLAYPAUSE
        165 => ext(0x10),                  // PREVIOUSSONG
        166 => ext(0x24),                  // STOPCD
        172 => ext(0x32),                  // HOMEPAGE
        173 => ext(0x67),                  // REFRESH
        183..=193 => 0x64 + (evdev - 183), // F13..F23
        194 => 0x76,                       // F24
        217 => ext(0x65),                  // SEARCH
        226 => ext(0x6D),                  // MEDIA
        _ => return None,
    };
    Some(code)
}

/// RDP scancode for an XKB keycode (what GTK reports as the hardware keycode).
#[must_use]
pub fn rdp_scancode_from_xkb(keycode: u32) -> Option<u32> {
    rdp_scancode_from_evdev(keycode.checked_sub(8)?)
}

/// Tracks keys held down in the session so they can be released when the window
/// loses focus (otherwise e.g. Alt stays "stuck" after Alt+Tab).
#[derive(Debug, Default)]
pub struct PressedKeys {
    keys: BTreeSet<u32>,
}

impl PressedKeys {
    /// Records a key event. Returns `true` if the key was already down (auto-repeat).
    pub fn update(&mut self, scancode: u32, down: bool) -> bool {
        if down {
            !self.keys.insert(scancode)
        } else {
            self.keys.remove(&scancode);
            false
        }
    }

    /// Clears the set and returns the keys that need a release event.
    pub fn take_all(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.keys).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_keys() {
        assert_eq!(rdp_scancode_from_evdev(1), Some(0x01)); // Esc
        assert_eq!(rdp_scancode_from_evdev(30), Some(0x1E)); // A
        assert_eq!(rdp_scancode_from_evdev(56), Some(0x38)); // Left Alt
        assert_eq!(rdp_scancode_from_evdev(100), Some(0x138)); // Right Alt
        assert_eq!(rdp_scancode_from_evdev(103), Some(0x148)); // Up
        assert_eq!(rdp_scancode_from_evdev(125), Some(0x15B)); // Left Win
        assert_eq!(rdp_scancode_from_evdev(119), Some(0x146)); // Pause
        assert_eq!(rdp_scancode_from_evdev(183), Some(0x64)); // F13
        assert_eq!(rdp_scancode_from_evdev(193), Some(0x6E)); // F23
        assert_eq!(rdp_scancode_from_evdev(0), None);
        assert_eq!(rdp_scancode_from_evdev(84), None);
        assert_eq!(rdp_scancode_from_evdev(500), None);
    }

    #[test]
    fn xkb_keycodes_are_offset_by_eight() {
        assert_eq!(rdp_scancode_from_xkb(9), Some(0x01)); // Esc
        assert_eq!(rdp_scancode_from_xkb(38), Some(0x1E)); // A
        assert_eq!(rdp_scancode_from_xkb(3), None);
    }

    #[test]
    fn pressed_keys_track_repeat_and_release() {
        let mut keys = PressedKeys::default();
        assert!(!keys.update(0x38, true));
        assert!(keys.update(0x38, true), "second press is a repeat");
        assert!(!keys.update(0x1E, true));
        assert!(!keys.update(0x1E, false));
        assert_eq!(keys.take_all(), vec![0x38]);
        assert!(keys.take_all().is_empty());
    }
}
