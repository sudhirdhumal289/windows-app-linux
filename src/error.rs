//! Error types for the Windows App Linux client.

use thiserror::Error;

/// Central error type covering network, authentication, and RDP failures.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("network communication error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("json serialization/deserialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("token expired or missing")]
    TokenExpired,

    #[error("device code authorization pending")]
    AuthorizationPending,

    #[error("rdp client execution error: {0}")]
    RdpLauncher(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("graph api error [{status}]: {message}")]
    GraphApi { status: u16, message: String },

    #[error("configuration error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
