//! # Error Module
//!
//! Unified error handling for the fastcarve crate.
//! Provides a central error type that wraps domain-specific errors.

use thiserror::Error;

use crate::carve::CarveError;
use crate::evidence::EvidenceError;
use crate::metadata::MetadataError;

/// Central error type for fastcarve operations.
#[derive(Debug, Error)]
pub enum FastCarveError {
    /// Error during file carving operations
    #[error("carve error: {0}")]
    Carve(#[from] CarveError),

    /// Error accessing evidence source
    #[error("evidence error: {0}")]
    Evidence(#[from] EvidenceError),

    /// Error recording metadata
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),

    /// Configuration error
    #[error("config error: {0}")]
    Config(String),

    /// Lock was poisoned (another thread panicked while holding it)
    #[error("lock poisoned: {0}")]
    LockPoisoned(String),

    /// I/O error
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Channel send/receive error
    #[error("channel error: {0}")]
    Channel(String),

    /// Generic error for other cases
    #[error("{0}")]
    Other(String),
}

impl FastCarveError {
    /// Create a lock poisoned error with context
    pub fn lock_poisoned(context: &str) -> Self {
        Self::LockPoisoned(context.to_string())
    }

    /// Create a channel error with context
    pub fn channel_error(context: &str) -> Self {
        Self::Channel(context.to_string())
    }

    /// Create a config error with message
    pub fn config_error(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
}

/// Result type alias using FastCarveError
pub type Result<T> = std::result::Result<T, FastCarveError>;

/// Extension trait for converting PoisonError to FastCarveError
pub trait LockResultExt<T> {
    /// Convert a lock result to FastCarveError
    fn map_lock_err(self, context: &str) -> std::result::Result<T, FastCarveError>;
}

impl<T, G> LockResultExt<T> for std::result::Result<T, std::sync::PoisonError<G>> {
    fn map_lock_err(self, context: &str) -> std::result::Result<T, FastCarveError> {
        self.map_err(|_| FastCarveError::lock_poisoned(context))
    }
}
