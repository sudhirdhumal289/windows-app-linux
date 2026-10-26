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
//! * what the main window shows, connection preparation, the session controller, web-view
//!   sign-in and all user-facing text: [`common`]
//!
//! Three frontends exist, each behind its own Cargo feature and each following its
//! platform's design guidelines:
//!
//! | Feature  | Module   | Toolkit                                   | Platform |
//! |----------|----------|-------------------------------------------|----------|
//! | `gtk`    | `gtk`    | GTK 4 + libadwaita (GNOME HIG)            | Linux    |
//! | `winui`  | `winui`  | WinUI 3 via `windows-reactor` (Fluent)    | Windows  |
//! | `appkit` | `appkit` | AppKit via `objc2` (Human Interface Guidelines) | macOS |
//!
//! Each provides the same screens: the connection library, the sign-in web view, the
//! session-host sign-in ([`crate::rdp::BridgeEvent::AuthRequired`]) and a session window
//! that renders [`crate::rdp::Frame`] and forwards input.

pub mod common;

#[cfg(feature = "gtk")]
pub mod gtk;

#[cfg(all(windows, feature = "winui"))]
pub mod winui;

#[cfg(all(target_os = "macos", feature = "appkit"))]
pub mod appkit;
