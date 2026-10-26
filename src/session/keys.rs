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

/// Keystrokes that type `text`: Enter and Tab as keys (so they act in any application),
/// other characters as Unicode input. Carriage returns are dropped (`\r\n` types one
/// Enter) as are other control characters. Returns `None` when `text` is too long.
#[must_use]
pub fn type_text(text: &str) -> Option<Vec<KeyInput>> {
    if text.chars().count() > MAX_TYPED_CHARS {
        return None;
    }
    let mut events = Vec::with_capacity(text.len() * 2);
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
                let mut units = [0u16; 2];
                for &unit in c.encode_utf16(&mut units).iter() {
                    events.push(KeyInput::Unicode { unit, down: true });
                    events.push(KeyInput::Unicode { unit, down: false });
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
    fn types_text_as_unicode_with_real_enter_and_tab() {
        let events = type_text("a\r\n\t😀").unwrap();
        assert_eq!(
            events[0],
            KeyInput::Unicode {
                unit: 'a' as u16,
                down: true
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
