//! Toolkit-independent pointer input encoding (MS-RDPBCGR 2.2.8.1.1.3.1.1.3).
//!
//! Frontends translate native events into [`MouseButton`]s, framebuffer coordinates
//! ([`map_coords`]) and wheel rotations ([`WheelAccumulator`]); [`super::NativeRdpBridge`]
//! turns them into RDP pointer PDUs.

pub(crate) const PTR_FLAGS_HWHEEL: u16 = 0x0400;
pub(crate) const PTR_FLAGS_WHEEL: u16 = 0x0200;
pub(crate) const PTR_FLAGS_MOVE: u16 = 0x0800;
pub(crate) const PTR_FLAGS_DOWN: u16 = 0x8000;
pub(crate) const PTR_FLAGS_BUTTON1: u16 = 0x1000;
pub(crate) const PTR_FLAGS_BUTTON2: u16 = 0x2000;
pub(crate) const PTR_FLAGS_BUTTON3: u16 = 0x4000;
pub(crate) const PTR_XFLAGS_DOWN: u16 = 0x8000;
pub(crate) const PTR_XFLAGS_BUTTON1: u16 = 0x0001;
pub(crate) const PTR_XFLAGS_BUTTON2: u16 = 0x0002;

/// Rotation of one wheel notch (Windows `WHEEL_DELTA`).
pub const WHEEL_DELTA: i32 = 120;
/// Largest rotation a single pointer PDU can carry.
const MAX_WHEEL_STEP: i32 = 0xFF;

/// Lock-key state for [`super::NativeRdpBridge::send_focus_in`] (`KBD_SYNC_*` flags).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockKeys {
    pub scroll_lock: bool,
    pub num_lock: bool,
    pub caps_lock: bool,
}

impl LockKeys {
    pub(crate) fn sync_flags(self) -> u16 {
        u16::from(self.scroll_lock)
            | (u16::from(self.num_lock) << 1)
            | (u16::from(self.caps_lock) << 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// "Back" side button.
    X1,
    /// "Forward" side button.
    X2,
}

impl MouseButton {
    /// Maps X11/Wayland/GTK button numbers (1 = left, 2 = middle, 3 = right, 8/9 = side).
    #[must_use]
    pub fn from_button_number(button: u32) -> Option<Self> {
        match button {
            1 => Some(Self::Left),
            2 => Some(Self::Middle),
            3 => Some(Self::Right),
            8 => Some(Self::X1),
            9 => Some(Self::X2),
            _ => None,
        }
    }

    /// `(flags, extended)`: extended buttons use the extended mouse PDU.
    pub(crate) fn flags(self, down: bool) -> (u16, bool) {
        match self {
            Self::Left | Self::Middle | Self::Right => {
                let button = match self {
                    Self::Left => PTR_FLAGS_BUTTON1,
                    Self::Right => PTR_FLAGS_BUTTON2,
                    _ => PTR_FLAGS_BUTTON3,
                };
                (button | if down { PTR_FLAGS_DOWN } else { 0 }, false)
            }
            Self::X1 | Self::X2 => {
                let button = if self == Self::X1 {
                    PTR_XFLAGS_BUTTON1
                } else {
                    PTR_XFLAGS_BUTTON2
                };
                (button | if down { PTR_XFLAGS_DOWN } else { 0 }, true)
            }
        }
    }
}

/// Maps a widget-relative position to framebuffer coordinates, clamped to the framebuffer.
///
/// `widget_size` is the widget's logical size; the framebuffer is stretched over it.
#[must_use]
pub fn map_coords(x: f64, y: f64, widget_size: (f64, f64), frame_size: (u32, u32)) -> (u16, u16) {
    let (ww, wh) = widget_size;
    let (fw, fh) = frame_size;
    if ww <= 0.0 || wh <= 0.0 || fw == 0 || fh == 0 {
        return (0, 0);
    }
    let scale = |pos: f64, widget: f64, frame: u32| -> u16 {
        let max = f64::from(frame.min(u32::from(u16::MAX) + 1) - 1);
        // Truncation is intended: clamped to [0, u16::MAX] first.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (pos * f64::from(frame) / widget).floor().clamp(0.0, max) as u16;
        v
    };
    (scale(x, ww, fw), scale(y, wh, fh))
}

/// A rectangle of the remote desktop in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    /// Whether the viewport lies entirely within a `frame_size` frame.
    #[must_use]
    pub fn fits(&self, frame_size: (u32, u32)) -> bool {
        self.width > 0
            && self.height > 0
            && u64::from(self.x) + u64::from(self.width) <= u64::from(frame_size.0)
            && u64::from(self.y) + u64::from(self.height) <= u64::from(frame_size.1)
    }
}

/// Like [`map_coords`], for a widget that shows only `viewport` of the remote desktop.
#[must_use]
pub fn map_viewport_coords(
    x: f64,
    y: f64,
    widget_size: (f64, f64),
    viewport: Viewport,
    frame_size: (u32, u32),
) -> (u16, u16) {
    if !viewport.fits(frame_size) {
        return map_coords(x, y, widget_size, frame_size);
    }
    let (lx, ly) = map_coords(x, y, widget_size, (viewport.width, viewport.height));
    let clamp = |offset: u32, local: u16| -> u16 {
        u16::try_from(offset.saturating_add(u32::from(local))).unwrap_or(u16::MAX)
    };
    (clamp(viewport.x, lx), clamp(viewport.y, ly))
}

/// Pointer flags for wheel `rotation` (positive = up / right), split into PDU-sized steps.
pub(crate) fn wheel_flags(rotation: i32, horizontal: bool) -> Vec<u16> {
    let axis = if horizontal {
        PTR_FLAGS_HWHEEL
    } else {
        PTR_FLAGS_WHEEL
    };
    let mut remaining = rotation;
    let mut out = Vec::new();
    while remaining != 0 {
        let step = remaining.clamp(-MAX_WHEEL_STEP, MAX_WHEEL_STEP);
        remaining -= step;
        // 9-bit two's complement; bit 8 is PTR_FLAGS_WHEEL_NEGATIVE.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let value = (step & 0x1FF) as u16;
        out.push(axis | value);
    }
    out
}

/// Accumulates fractional scroll deltas (smooth scrolling, touchpads) into whole
/// wheel rotations so small movements aren't rounded up to full notches.
#[derive(Debug, Default, Clone)]
pub struct WheelAccumulator {
    horizontal: f64,
    vertical: f64,
}

impl WheelAccumulator {
    /// Adds a scroll delta in notches (toolkit convention: positive `dy` = down,
    /// positive `dx` = right). Returns `(horizontal, vertical)` rotations ready to send.
    pub fn push(&mut self, dx: f64, dy: f64) -> (i32, i32) {
        self.horizontal += dx * f64::from(WHEEL_DELTA);
        self.vertical -= dy * f64::from(WHEEL_DELTA);
        (
            Self::take(&mut self.horizontal),
            Self::take(&mut self.vertical),
        )
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn take(acc: &mut f64) -> i32 {
        let whole = acc.trunc();
        *acc -= whole;
        // Deltas per event are tiny compared to i32's range.
        #[allow(clippy::cast_possible_truncation)]
        let v = whole.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_encoding_matches_rdp_twos_complement() {
        assert_eq!(wheel_flags(120, false), vec![PTR_FLAGS_WHEEL | 0x78]);
        // xfreerdp sends 0x0188 (NEGATIVE | 0x88) for one notch down.
        assert_eq!(wheel_flags(-120, false), vec![PTR_FLAGS_WHEEL | 0x188]);
        assert_eq!(wheel_flags(-1, true), vec![PTR_FLAGS_HWHEEL | 0x1FF]);
        assert_eq!(
            wheel_flags(360, false),
            vec![PTR_FLAGS_WHEEL | 0xFF, PTR_FLAGS_WHEEL | 0x69]
        );
        assert!(wheel_flags(0, false).is_empty());
    }

    #[test]
    fn wheel_accumulator_handles_smooth_scrolling() {
        let mut acc = WheelAccumulator::default();
        assert_eq!(acc.push(0.0, 0.25), (0, -30));
        assert_eq!(acc.push(0.0, -0.1), (0, 12));
        assert_eq!(acc.push(1.0, 0.0), (120, 0));
        assert_eq!(acc.push(0.004, 0.0), (0, 0));
        assert_eq!(acc.push(0.005, 0.0), (1, 0));
    }

    #[test]
    fn mouse_buttons() {
        assert_eq!(MouseButton::from_button_number(1), Some(MouseButton::Left));
        assert_eq!(MouseButton::from_button_number(3), Some(MouseButton::Right));
        assert_eq!(MouseButton::from_button_number(4), None);
        assert_eq!(MouseButton::Right.flags(true), (0xA000, false));
        assert_eq!(MouseButton::Middle.flags(false), (PTR_FLAGS_BUTTON3, false));
        assert_eq!(MouseButton::X2.flags(true), (0x8002, true));
    }

    #[test]
    fn coordinates_are_scaled_and_clamped() {
        assert_eq!(map_coords(50.0, 25.0, (100.0, 50.0), (200, 100)), (100, 50));
        assert_eq!(map_coords(-5.0, 80.0, (100.0, 50.0), (200, 100)), (0, 99));
        assert_eq!(map_coords(1.0, 1.0, (0.0, 50.0), (200, 100)), (0, 0));
    }

    #[test]
    fn lock_key_flags() {
        let all = LockKeys {
            scroll_lock: true,
            num_lock: true,
            caps_lock: true,
        };
        assert_eq!(all.sync_flags(), 7);
        assert_eq!(LockKeys::default().sync_flags(), 0);
    }

    #[test]
    fn viewport_coords_are_offset_into_the_frame() {
        let vp = Viewport {
            x: 1920,
            y: 0,
            width: 1280,
            height: 1024,
        };
        let frame = (3200, 1080);
        assert_eq!(
            map_viewport_coords(0.0, 0.0, (640.0, 512.0), vp, frame),
            (1920, 0)
        );
        assert_eq!(
            map_viewport_coords(639.9, 511.9, (640.0, 512.0), vp, frame),
            (3199, 1023)
        );
        // A frame that doesn't contain the viewport falls back to the whole frame.
        assert_eq!(
            map_viewport_coords(320.0, 0.0, (640.0, 512.0), vp, (1280, 1024)),
            (640, 0)
        );
    }
}
