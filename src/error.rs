//! Unified application error type.

/// Top-level error type for the v3-node binary and library.
///
/// Each module adds its own variant here so that error propagation with `?`
/// works uniformly throughout the codebase.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A failure originating from the P2P network layer.
    #[error("network error: {0}")]
    Network(String),

    /// An I/O error (file, socket, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An error produced while parsing or loading configuration.
    #[error("config error: {0}")]
    Config(String),

    /// A database (SQLite) error.
    #[error("database error: {0}")]
    Database(String),

    /// A generic catch-all for errors that don't fit a specific variant yet.
    #[error("{0}")]
    Other(String),
}