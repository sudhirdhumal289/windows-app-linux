//! Native Linux client for Microsoft Windows 365 and Azure Virtual Desktop.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod models;
pub mod rdp;
pub mod ui;

pub use config::AppConfig;
pub use error::{AppError, Result};
