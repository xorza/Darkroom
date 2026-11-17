//! [`SpanError`]: why two ends make no span.

use thiserror::Error;

/// Why two ends make no span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SpanError {
    /// An end is NaN or infinite.
    #[error("a span's end is not finite")]
    NotFinite,
    /// The floor is not below the ceiling.
    #[error("a span's floor is not below its ceiling")]
    Empty,
}
