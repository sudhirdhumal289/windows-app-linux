//! Error types for the Windows App Linux client.

use thiserror::Error;

/// Central error type covering network, authentication, configuration and session failures.
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

    #[error("remote desktop session error: {0}")]
    Session(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("graph api error [{status}]: {message}")]
    GraphApi { status: u16, message: String },

    #[error("configuration error: {0}")]
    Config(String),

    /// A service refused a request; the message is already user-facing.
    #[error("{0}")]
    Service(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
