//! Key combinations the local desktop would otherwise intercept (Ctrl+Alt+Del, the
//! Windows key, ...) and text typed into the session as keystrokes.

use crate::rdp::keymap::KBDEXT;

const CTRL: u32 = 0x1D;
const ALT: u32 = 0x38;
const SHIFT: u32 = 0x2A;
const ESC: u32 = 0x01;
const TAB: u32 = 0x0F;
const ENTER: u32 = 0x1C;
const F4: u32 = 0x3E;
const L: u32 = 0x26;
const DELETE: u32 = 0x53 | KBDEXT;
const WIN: u32 = 0x5B | KBDEXT;
const PRINT_SCREEN: u32 = 0x37 | KBDEXT;

/// Longest text typed in one go.
pub const MAX_TYPED_CHARS: usize = 10_000;

/// One keyboard event for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInput {
    /// RDP scancode (with `KBDEXT` for extended keys).
    Scancode { code: u32, down: bool },
    /// UTF-16 code unit, independent of the remote keyboard layout.
    Unicode { unit: u16, down: bool },
}

/// Shortcuts offered in the session's "Send Keys" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCombo {
    CtrlAltDel,
    CtrlShiftEsc,
    Windows,
    WindowsL,
    AltTab,
    AltF4,
    PrintScreen,
}

impl KeyCombo {
    pub const ALL: [Self; 7] = [
        Self::CtrlAltDel,
        Self::CtrlShiftEsc,
        Self::Windows,
        Self::WindowsL,
        Self::AltTab,
        Self::AltF4,
        Self::PrintScreen,
    ];

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::CtrlAltDel => "ctrl-alt-del",
            Self::CtrlShiftEsc => "ctrl-shift-esc",
            Self::Windows => "win",
            Self::WindowsL => "win-l",
            Self::AltTab => "alt-tab",
            Self::AltF4 => "alt-f4",
            Self::PrintScreen => "print",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::CtrlAltDel => "Ctrl+Alt+Del",
            Self::CtrlShiftEsc => "Ctrl+Shift+Esc (Task Manager)",
            Self::Windows => "Windows Key (Start)",
            Self::WindowsL => "Windows+L (Lock)",
            Self::AltTab => "Alt+Tab",
            Self::AltF4 => "Alt+F4",
            Self::PrintScreen => "Print Screen",
        }
    }

    fn scancodes(self) -> &'static [u32] {
        match self {
            Self::CtrlAltDel => &[CTRL, ALT, DELETE],
            Self::CtrlShiftEsc => &[CTRL, SHIFT, ESC],
            Self::Windows => &[WIN],
            Self::WindowsL => &[WIN, L],
            Self::AltTab => &[ALT, TAB],
            Self::AltF4 => &[ALT, F4],
            Self::PrintScreen => &[PRINT_SCREEN],
        }
    }

    /// Presses the keys in order and releases them in reverse.
    #[must_use]
    pub fn events(self) -> Vec<KeyInput> {
        let keys = self.scancodes();
        keys.iter()
            .map(|&code| KeyInput::Scancode { code, down: true })
            .chain(
                keys.iter()
                    .rev()
                    .map(|&code| KeyInput::Scancode { code, down: false }),
            )
            .collect()
    }
}

/// Maps standard printable ASCII characters to hardware scancodes and Shift requirement.
/// This ensures keystrokes are registered by all applications, including consoles,
/// password fields, and apps that do not consume `WM_CHAR` / `VK_PACKET`.
fn char_to_scancode(c: char) -> Option<(u32, bool)> {
    match c {
        'a'..='z' => Some((
            match c {
                'a' => 0x1E,
                'b' => 0x30,
                'c' => 0x2E,
                'd' => 0x20,
                'e' => 0x12,
                'f' => 0x21,
                'g' => 0x22,
                'h' => 0x23,
                'i' => 0x17,
                'j' => 0x24,
                'k' => 0x25,
                'l' => 0x26,
                'm' => 0x32,
                'n' => 0x31,
                'o' => 0x18,
                'p' => 0x19,
                'q' => 0x10,
                'r' => 0x13,
                's' => 0x1F,
                't' => 0x14,
                'u' => 0x16,
                'v' => 0x2F,
                'w' => 0x11,
                'x' => 0x2D,
                'y' => 0x15,
                'z' => 0x2C,
                _ => unreachable!(),
            },
            false,
        )),
        'A'..='Z' => Some((
            match c {
                'A' => 0x1E,
                'B' => 0x30,
                'C' => 0x2E,
                'D' => 0x20,
                'E' => 0x12,
                'F' => 0x21,
                'G' => 0x22,
                'H' => 0x23,
                'I' => 0x17,
                'J' => 0x24,
                'K' => 0x25,
                'L' => 0x26,
                'M' => 0x32,
                'N' => 0x31,
                'O' => 0x18,
                'P' => 0x19,
                'Q' => 0x10,
                'R' => 0x13,
                'S' => 0x1F,
                'T' => 0x14,
                'U' => 0x16,
                'V' => 0x2F,
                'W' => 0x11,
                'X' => 0x2D,
                'Y' => 0x15,
                'Z' => 0x2C,
                _ => unreachable!(),
            },
            true,
        )),
        '1'..='9' => Some((0x02 + (c as u32 - '1' as u32), false)),
        '0' => Some((0x0B, false)),
        ' ' => Some((0x39, false)),
        '-' => Some((0x0C, false)),
        '=' => Some((0x0D, false)),
        '[' => Some((0x1A, false)),
        ']' => Some((0x1B, false)),
        ';' => Some((0x27, false)),
        '\'' => Some((0x28, false)),
        '`' => Some((0x29, false)),
        '\\' => Some((0x2B, false)),
        ',' => Some((0x33, false)),
        '.' => Some((0x34, false)),
        '/' => Some((0x35, false)),
        // Shifted numbers and symbols:
        '!' => Some((0x02, true)),
        '@' => Some((0x03, true)),
        '#' => Some((0x04, true)),
        '$' => Some((0x05, true)),
        '%' => Some((0x06, true)),
        '^' => Some((0x07, true)),
        '&' => Some((0x08, true)),
        '*' => Some((0x09, true)),
        '(' => Some((0x0A, true)),
        ')' => Some((0x0B, true)),
        '_' => Some((0x0C, true)),
        '+' => Some((0x0D, true)),
        '{' => Some((0x1A, true)),
        '}' => Some((0x1B, true)),
        ':' => Some((0x27, true)),
        '"' => Some((0x28, true)),
        '~' => Some((0x29, true)),
        '|' => Some((0x2B, true)),
        '<' => Some((0x33, true)),
        '>' => Some((0x34, true)),
        '?' => Some((0x35, true)),
        _ => None,
    }
}

/// Keystrokes that type `text`: standard characters as hardware scancodes (so they act
/// in any application, console or password prompt), other characters as Unicode input.
/// Carriage returns are dropped (`\r\n` types one Enter) as are other control characters.
/// Returns `None` when `text` is too long.
#[must_use]
pub fn type_text(text: &str) -> Option<Vec<KeyInput>> {
    if text.chars().count() > MAX_TYPED_CHARS {
        return None;
    }
    let mut events = Vec::with_capacity(text.len() * 4);
    let press = |events: &mut Vec<KeyInput>, code| {
        events.push(KeyInput::Scancode { code, down: true });
        events.push(KeyInput::Scancode { code, down: false });
    };
    for c in text.chars() {
        match c {
            '\n' => press(&mut events, ENTER),
            '\t' => press(&mut events, TAB),
            c if c.is_control() => {}
            c => {
                if let Some((code, shifted)) = char_to_scancode(c) {
                    if shifted {
                        events.push(KeyInput::Scancode {
                            code: SHIFT,
                            down: true,
                        });
                        events.push(KeyInput::Scancode { code, down: true });
                        events.push(KeyInput::Scancode { code, down: false });
                        events.push(KeyInput::Scancode {
                            code: SHIFT,
                            down: false,
                        });
                    } else {
                        press(&mut events, code);
                    }
                } else {
                    let mut units = [0u16; 2];
                    for &unit in c.encode_utf16(&mut units).iter() {
                        events.push(KeyInput::Unicode { unit, down: true });
                        events.push(KeyInput::Unicode { unit, down: false });
                    }
                }
            }
        }
    }
    Some(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_press_then_release_in_reverse() {
        let events = KeyCombo::CtrlAltDel.events();
        let codes: Vec<(u32, bool)> = events
            .iter()
            .map(|e| match *e {
                KeyInput::Scancode { code, down } => (code, down),
                KeyInput::Unicode { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            codes,
            [
                (CTRL, true),
                (ALT, true),
                (DELETE, true),
                (DELETE, false),
                (ALT, false),
                (CTRL, false)
            ]
        );
        for combo in KeyCombo::ALL {
            assert_eq!(KeyCombo::from_id(combo.id()), Some(combo));
        }
        assert_eq!(KeyCombo::from_id("nope"), None);
    }

    #[test]
    fn types_text_as_scancodes_and_unicode_with_real_enter_and_tab() {
        let events = type_text("a\r\n\t😀").unwrap();
        // 'a' is scancode 0x1E
        assert_eq!(
            events[0],
            KeyInput::Scancode {
                code: 0x1E,
                down: true
            }
        );
        assert_eq!(
            events[1],
            KeyInput::Scancode {
                code: 0x1E,
                down: false
            }
        );
        assert_eq!(
            events[2],
            KeyInput::Scancode {
                code: ENTER,
                down: true
            }
        );
        assert_eq!(
            events[4],
            KeyInput::Scancode {
                code: TAB,
                down: true
            }
        );
        // The emoji needs a surrogate pair: 2 units × press/release.
        assert_eq!(events.len(), 2 + 2 + 2 + 4);
        assert!(type_text(&"x".repeat(MAX_TYPED_CHARS + 1)).is_none());
        assert_eq!(type_text("").unwrap(), Vec::new());
    }
}
