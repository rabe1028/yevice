//! Error types for diagram rendering.

use thiserror::Error;

/// Errors that can occur during architecture diagram rendering.
#[derive(Debug, Error)]
pub enum RenderError {
    /// JSON serialization failed.
    #[cfg(feature = "json")]
    #[error("JSON serialization error")]
    Json(#[from] serde_json::Error),

    /// Topology is empty and cannot produce a meaningful diagram.
    #[error("topology is empty: no nodes to render")]
    EmptyTopology,
}
