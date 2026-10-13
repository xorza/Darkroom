//! Errors the image ops report from their `apply` boundary.

use crate::error::InvalidConfigField;
#[cfg(feature = "ml")]
use crate::image_ops::ml::backend::WINDOW;
#[cfg(feature = "ml")]
use crate::math::size2us::Size2us;

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
    /// A background division met a model whose mean is not positive, so it has no level to
    /// normalize the model to.
    #[error("background division needs a positive model mean, got {mean}")]
    NonPositiveBackground { mean: f64 },
    /// An auto stretch cannot map the measured background onto its target: the median sits at or
    /// below zero (nothing for a brightening curve to lift), or the curve's range does not reach
    /// the target from it.
    #[error("{method} cannot map a background median of {median} to the target {target}")]
    UnreachableBackground {
        method: &'static str,
        median: f32,
        target: f32,
    },
}

#[cfg(feature = "ml")]
/// Why an ML filter failed.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("image must be at least {WINDOW}×{WINDOW}, got {}×{}", .0.width, .0.height)]
    TooSmall(Size2us),
    #[error("invalid ML tiling configuration: {0}")]
    InvalidConfig(#[from] InvalidConfigField),
    #[error("ONNX model error: {0}")]
    Model(#[from] ort::Error),
    /// The model's output is not one NHWC `[1, WINDOW, WINDOW, 3]` tile.
    #[error(
        "model output has {len} values, expected {} (NHWC [1,{WINDOW},{WINDOW},3])",
        WINDOW * WINDOW * 3
    )]
    OutputLength { len: usize },
}
