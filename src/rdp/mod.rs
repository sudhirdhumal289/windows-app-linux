//! Embedded FreeRDP 3 engine.
//!
//! `freerdp_bridge.c` contains only the glue that needs FreeRDP's C API (context setup,
//! event loop, channels, input injection). Everything else is safe, toolkit-independent
//! Rust that any frontend can use:
//!
//! * [`NativeRdpBridge`] — one session; emits [`BridgeEvent`]s and accepts input.
//! * [`input`] — pointer/wheel encoding and coordinate mapping.
//! * [`keymap`] — evdev/XKB key codes → RDP scancodes. Windows and macOS frontends add
//!   their own native key-code tables next to it.

mod auth;
mod bridge;
mod clipboard;
mod ffi;
pub mod input;
pub mod keymap;

pub use bridge::{
    join_pending_teardowns, supports_h264, supports_surface_protection, supports_usb_redirection,
    version, BridgeEvent, BridgeOptions, CertificateDecision, CertificatePrompt, ConnectionQuality,
    EventSink, Frame, MonitorRect, NativeRdpBridge, NetworkStats, RemoteFileFetcher, SessionStatus,
};
pub use clipboard::{LocalClipboard, RemoteClipboard, RemoteFile, RgbaImage};
pub use input::{LockKeys, MouseButton, Viewport, WheelAccumulator};
