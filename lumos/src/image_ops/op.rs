//! The contract every in-place image op enforces at its `apply` boundary: a linear f32 master
//! ([`require_f32_master`]) and valid configuration ([`ensure`]), reported via [`OpError`] instead
//! of a panic. The ops themselves (`denoise`, `hdr`, `stretching`, …) run over [`crate::image_ops`].

use imaginarium::{ColorFormat, Image};

/// Why a display/processing op failed.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    /// The image isn't a linear f32 master (`L_F32` or `RGB_F32`).
    #[error("image op requires an L_F32 or RGB_F32 image, got {0}")]
    UnsupportedFormat(ColorFormat),
    /// A configuration parameter is outside its valid range.
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    /// A model's design matrix does not contain enough independent information.
    #[error("{operation} is rank deficient: rank {rank}, requires {required_rank}")]
    RankDeficient {
        operation: &'static str,
        rank: usize,
        required_rank: usize,
    },
}

/// The display ops are defined on a linear f32 master in L or RGB; reject anything else
/// (integer formats, RGBA) at the op boundary, before the per-pixel helpers (which assume it).
pub(crate) fn require_f32_master(image: &Image) -> Result<(), OpError> {
    let format = image.desc().color_format;
    if format == ColorFormat::L_F32 || format == ColorFormat::RGB_F32 {
        Ok(())
    } else {
        Err(OpError::UnsupportedFormat(format))
    }
}

/// `Ok(())` when `cond` holds, else an [`OpError::InvalidConfig`] carrying `msg`. The Result-returning
/// counterpart to `assert!` for config validation.
pub(crate) fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Result<(), OpError> {
    if cond {
        Ok(())
    } else {
        Err(OpError::InvalidConfig(msg()))
    }
}
