//! Windows App for Linux — a native client for Windows 365 Cloud PCs and Azure Virtual
//! Desktop.
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
//! | `frontend` | Native UI (GTK 4 / libadwaita today) | per platform |

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
pub mod platform;
pub mod rdp;
pub mod rdp_file;
pub mod session;

pub use config::AppConfig;
pub use error::{AppError, Result};
