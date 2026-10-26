//! Native user interfaces.
//!
//! A frontend owns windows, dialogs and input handling only. Everything else lives in the
//! toolkit-independent core and is shared by all frontends:
//!
//! * sign-in and tokens: [`crate::auth::AuthManager`], [`crate::auth::Account`]
//! * Cloud PC discovery and actions: [`crate::api::CloudPcService`]
//! * session preparation: [`crate::session::prepare`]
//! * the RDP engine: [`crate::rdp::NativeRdpBridge`] (events in, input out; keyboard and
//!   pointer helpers in [`crate::rdp::input`] and [`crate::rdp::keymap`])
//! * host queries: [`crate::platform`]
//!
//! To add a frontend (e.g. WinUI on Windows or AppKit on macOS), add a sibling module
//! behind its own Cargo feature that provides the same screens as [`gtk`]: the Cloud PC
//! list, the sign-in web view, the session-host sign-in ([`crate::rdp::BridgeEvent::AuthRequired`])
//! and a session window that renders [`crate::rdp::Frame`] and forwards input.

#[cfg(feature = "gtk")]
pub mod gtk;
