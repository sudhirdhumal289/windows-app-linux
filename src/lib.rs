//! Windows App — a native client for Windows 365 Cloud PCs, Azure Virtual Desktop and any
//! Remote Desktop (RDP) computer, on Linux, Windows and macOS.
//!
//! The crate is split into a portable core and per-platform frontends:
//!
//! | Module | Responsibility | UI toolkit |
//! |---|---|---|
//! | [`config`] | Preferences, constants, private file storage | none |
//! | [`auth`] | Entra ID sign-in, token store, PKeyAuth | none |
//! | [`api`] | Windows 365 / Graph / RD Web clients | none |
//! | [`connections`] | Saved Remote PCs (direct RDP by address) | none |
//! | [`overrides`] | Per-connection settings and pre/post commands | none |
//! | [`history`] | Last-connected times | none |
//! | [`rdp_file`] | `.rdp` file import and export | none |
//! | [`session`] | `.rdp` file + FreeRDP argument preparation | none |
//! | [`launcher`] | Launch targets (`--connect`), desktop search index | none |
//! | [`rdp`] | Embedded FreeRDP 3 engine (C bridge + safe Rust wrapper) | none |
//! | [`platform`] | Host OS integration (display scale, password store, hooks) | none |
//! | `frontend` | Shared view models (`frontend::common`) and native UIs: GTK 4 / libadwaita (Linux), WinUI 3 (Windows), AppKit (macOS) | per platform |

pub mod api;
pub mod auth;
pub mod config;
pub mod connections;
pub mod error;
pub mod frontend;
pub mod history;
pub mod launcher;
pub mod models;
pub mod overrides;
pub mod placement;
pub mod platform;
pub mod rdp;
pub mod rdp_file;
pub mod session;

pub use config::AppConfig;
pub use error::{AppError, Result};
