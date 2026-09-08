//! Shared errors used at spoke boundaries.

use thiserror::Error;

/// An error raised while decoding a source format.
#[derive(Debug, Error)]
pub enum ImportError {
    /// The input container or syntax is invalid.
    #[error("invalid {format} input: {detail}")]
    Invalid {
        /// Human-readable format name.
        format: &'static str,
        /// Diagnostic detail safe to show to a caller.
        detail: String,
    },
    /// A required input or piece of metadata is absent.
    #[error("missing required input `{0}`")]
    Missing(String),
    /// The source uses a feature the importer does not support yet.
    #[error("unsupported input feature: {0}")]
    Unsupported(String),
}

/// An error raised while encoding a target format.
#[derive(Debug, Error)]
pub enum ExportError {
    /// The neutral model is invalid and cannot be encoded safely.
    #[error("invalid material model: {0}")]
    InvalidModel(String),
    /// The target encoder failed.
    #[error("failed to encode {format}: {detail}")]
    Encode {
        /// Human-readable format name.
        format: &'static str,
        /// Diagnostic detail safe to show to a caller.
        detail: String,
    },
    /// The requested output option is not supported.
    #[error("unsupported output option: {0}")]
    Unsupported(String),
}
