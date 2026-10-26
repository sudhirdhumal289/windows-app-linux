//! Linux evdev key codes → RDP scancodes (scan code set 1 + `KBDEXT`).
//!
//! RDP transmits physical key positions; the remote session applies its own keyboard
//! layout. Mapping evdev codes directly avoids FreeRDP's deprecated X11 keyboard
//! helpers and works identically on X11 and Wayland. GTK/XKB keycodes are evdev + 8
//! ([`rdp_scancode_from_xkb`]). Windows and macOS frontends use
//! [`rdp_scancode_from_win32`] and [`rdp_scancode_from_mac`].

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

/// RDP scancode for a Windows keyboard message: the scan code (`lParam` bits 16-23) and
/// the extended-key flag (bit 24). `None` for synthesized keys without a scan code.
#[must_use]
pub fn rdp_scancode_from_win32(scan: u32, extended: bool) -> Option<u32> {
    let code = match (scan, extended) {
        (0, _) => return None,
        // Windows reports Num Lock as extended and Pause as plain 0x45.
        (0x45, true) => 0x45,
        (0x45, false) => ext(0x46),
        // Alt+Print Screen (SysRq).
        (0x54, false) => ext(0x37),
        (scan, _) if scan > 0x7F => return None,
        (scan, true) => ext(scan),
        (scan, false) => scan,
    };
    Some(code)
}

/// RDP scancode for a macOS virtual key code (`NSEvent.keyCode`, `kVK_*` from Carbon's
/// `Events.h`). Command maps to the Windows key and Option to Alt.
#[must_use]
pub fn rdp_scancode_from_mac(key_code: u16) -> Option<u32> {
    let code = match key_code {
        0x00 => 0x1E,      // A
        0x01 => 0x1F,      // S
        0x02 => 0x20,      // D
        0x03 => 0x21,      // F
        0x04 => 0x23,      // H
        0x05 => 0x22,      // G
        0x06 => 0x2C,      // Z
        0x07 => 0x2D,      // X
        0x08 => 0x2E,      // C
        0x09 => 0x2F,      // V
        0x0A => 0x56,      // ISO section (102nd key)
        0x0B => 0x30,      // B
        0x0C => 0x10,      // Q
        0x0D => 0x11,      // W
        0x0E => 0x12,      // E
        0x0F => 0x13,      // R
        0x10 => 0x15,      // Y
        0x11 => 0x14,      // T
        0x12 => 0x02,      // 1
        0x13 => 0x03,      // 2
        0x14 => 0x04,      // 3
        0x15 => 0x05,      // 4
        0x16 => 0x07,      // 6
        0x17 => 0x06,      // 5
        0x18 => 0x0D,      // =
        0x19 => 0x0A,      // 9
        0x1A => 0x08,      // 7
        0x1B => 0x0C,      // -
        0x1C => 0x09,      // 8
        0x1D => 0x0B,      // 0
        0x1E => 0x1B,      // ]
        0x1F => 0x18,      // O
        0x20 => 0x16,      // U
        0x21 => 0x1A,      // [
        0x22 => 0x17,      // I
        0x23 => 0x19,      // P
        0x24 => 0x1C,      // Return
        0x25 => 0x26,      // L
        0x26 => 0x24,      // J
        0x27 => 0x28,      // '
        0x28 => 0x25,      // K
        0x29 => 0x27,      // ;
        0x2A => 0x2B,      // \
        0x2B => 0x33,      // ,
        0x2C => 0x35,      // /
        0x2D => 0x31,      // N
        0x2E => 0x32,      // M
        0x2F => 0x34,      // .
        0x30 => 0x0F,      // Tab
        0x31 => 0x39,      // Space
        0x32 => 0x29,      // `
        0x33 => 0x0E,      // Delete (Backspace)
        0x35 => 0x01,      // Escape
        0x36 => ext(0x5C), // Right Command
        0x37 => ext(0x5B), // Command
        0x38 => 0x2A,      // Shift
        0x39 => 0x3A,      // Caps Lock
        0x3A => 0x38,      // Option
        0x3B => 0x1D,      // Control
        0x3C => 0x36,      // Right Shift
        0x3D => ext(0x38), // Right Option
        0x3E => ext(0x1D), // Right Control
        0x40 => 0x68,      // F17
        0x41 => 0x53,      // Keypad .
        0x43 => 0x37,      // Keypad *
        0x45 => 0x4E,      // Keypad +
        0x47 => 0x45,      // Keypad Clear (Num Lock)
        0x48 => ext(0x30), // Volume Up
        0x49 => ext(0x2E), // Volume Down
        0x4A => ext(0x20), // Mute
        0x4B => ext(0x35), // Keypad /
        0x4C => ext(0x1C), // Keypad Enter
        0x4E => 0x4A,      // Keypad -
        0x4F => 0x69,      // F18
        0x50 => 0x6A,      // F19
        0x51 => 0x59,      // Keypad =
        0x52 => 0x52,      // Keypad 0
        0x53 => 0x4F,      // Keypad 1
        0x54 => 0x50,      // Keypad 2
        0x55 => 0x51,      // Keypad 3
        0x56 => 0x4B,      // Keypad 4
        0x57 => 0x4C,      // Keypad 5
        0x58 => 0x4D,      // Keypad 6
        0x59 => 0x47,      // Keypad 7
        0x5A => 0x6B,      // F20
        0x5B => 0x48,      // Keypad 8
        0x5C => 0x49,      // Keypad 9
        0x5D => 0x7D,      // JIS Yen
        0x5E => 0x73,      // JIS Underscore (Ro)
        0x5F => 0x7E,      // JIS Keypad ,
        0x60 => 0x3F,      // F5
        0x61 => 0x40,      // F6
        0x62 => 0x41,      // F7
        0x63 => 0x3D,      // F3
        0x64 => 0x42,      // F8
        0x65 => 0x43,      // F9
        0x66 => 0x7B,      // JIS Eisu (Muhenkan)
        0x67 => 0x57,      // F11
        0x68 => 0x70,      // JIS Kana
        0x69 => 0x64,      // F13
        0x6A => 0x67,      // F16
        0x6B => 0x65,      // F14
        0x6D => 0x44,      // F10
        0x6E => ext(0x5D), // Context menu
        0x6F => 0x58,      // F12
        0x71 => 0x66,      // F15
        0x72 => ext(0x52), // Help (Insert)
        0x73 => ext(0x47), // Home
        0x74 => ext(0x49), // Page Up
        0x75 => ext(0x53), // Forward Delete
        0x76 => 0x3E,      // F4
        0x77 => ext(0x4F), // End
        0x78 => 0x3C,      // F2
        0x79 => ext(0x51), // Page Down
        0x7A => 0x3B,      // F1
        0x7B => ext(0x4B), // Left
        0x7C => ext(0x4D), // Right
        0x7D => ext(0x50), // Down
        0x7E => ext(0x48), // Up
        _ => return None,
    };
    Some(code)
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
    fn maps_win32_scan_codes() {
        assert_eq!(rdp_scancode_from_win32(0x1E, false), Some(0x1E)); // A
        assert_eq!(rdp_scancode_from_win32(0x38, true), Some(0x138)); // Right Alt
        assert_eq!(rdp_scancode_from_win32(0x45, true), Some(0x45)); // Num Lock
        assert_eq!(rdp_scancode_from_win32(0x45, false), Some(0x146)); // Pause
        assert_eq!(rdp_scancode_from_win32(0, false), None);
    }

    #[test]
    fn maps_mac_key_codes() {
        assert_eq!(rdp_scancode_from_mac(0x00), Some(0x1E)); // A
        assert_eq!(rdp_scancode_from_mac(0x37), Some(0x15B)); // Command → Win
        assert_eq!(rdp_scancode_from_mac(0x3A), Some(0x38)); // Option → Alt
        assert_eq!(rdp_scancode_from_mac(0x7E), Some(0x148)); // Up
        assert_eq!(rdp_scancode_from_mac(0x3F), None); // Fn
                                                       // Every mapped key is distinct (except none).
        let mut seen = std::collections::BTreeSet::new();
        for code in 0..=0x7F {
            if let Some(scancode) = rdp_scancode_from_mac(code) {
                assert!(seen.insert(scancode), "duplicate for {code:#x}");
            }
        }
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
