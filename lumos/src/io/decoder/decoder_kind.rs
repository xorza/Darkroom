//! [`DecoderKind`]: which decoder a frame's planes came from.

use serde::{Deserialize, Serialize};

/// Which decoder produced a cached frame's planes.
///
/// A RAW frame decodes to a demosaiced image or to its undemosaiced sensor plane, and both can have
/// the same size on disk. Without this in the key, a mosaic load would map a demosaiced channel as
/// its sensor plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DecoderKind {
    /// [`LoadCtx::linear`](crate::LoadCtx::linear).
    Linear,
    /// [`LoadCtx::cfa`](crate::LoadCtx::cfa).
    Cfa,
}
