//! The contract every in-place image op enforces at its `apply` boundary: valid configuration
//! ([`crate::InvalidConfigField`]), reported via [`OpError`] instead of a panic. The ops themselves
//! (`denoise`, `hdr`, `stretching`, …) run over [`crate::image_ops`].
//!
//! There is no format check any more, and no `UnsupportedFormat` error to raise: the ops take a
//! [`crate::LinearImage`], which is one or three `f32` planes by construction. The interleaved
//! `Image` those ops used to take could be integer, RGBA or anything else imaginarium supports, so
//! every `apply` opened by rejecting what it could not handle; making the storage the crate's own
//! planar type turns that from a runtime check into a type.
//!
//! A convention rather than a trait, deliberately. A trait would turn nine inherent `apply`
//! methods into trait methods, so every downstream call site would need it in scope, to save the
//! one-line prologue and express a composability nothing uses — `lens` drives each op from its own
//! node with its own deserialized config type, and would keep doing so. `NeutralizeBackground`
//! takes no parameters and so has no `validate` to call; that is the contract met, not skipped.
//!
//! The two `ml`-gated ops (`MlDenoise`, `RemoveStars`) take the same `apply(&mut LinearImage)`
//! shape but report [`crate::MlError`]: their failures are a missing model or an image smaller
//! than one tile, neither of which is a config range that `InvalidConfigField` describes.

use crate::error::InvalidConfigField;

/// Why a display/processing op failed.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    /// A configuration parameter is outside its valid range.
    #[error("invalid config: {0}")]
    InvalidConfig(#[from] InvalidConfigField),
    /// A model's design matrix does not contain enough independent information.
    #[error("{operation} is rank deficient: rank {rank}, requires {required_rank}")]
    RankDeficient {
        operation: &'static str,
        rank: usize,
        required_rank: usize,
    },
}
