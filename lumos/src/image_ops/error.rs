//! Errors the image ops report from their `apply` boundary.

use std::fmt;

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
    Config(#[from] InvalidConfigField),
    /// The background surface's design matrix does not hold enough independent information: the
    /// sample tiles do not span the image in both directions.
    #[error("the background surface fit is rank deficient: rank {rank}, requires {required_rank}")]
    RankDeficient { rank: usize, required_rank: usize },
    /// A background fit of `degree` has more terms, `(d + 1)(d + 2)/2`, than the image has sample
    /// tiles to determine them.
    #[error(
        "a degree-{degree} background needs {} sample tiles, and the image gives {samples}",
        (degree + 1) * (degree + 2) / 2
    )]
    TooFewSamples { samples: usize, degree: usize },
    /// A background division met a model whose mean is not positive, so it has no level to
    /// normalize the model to.
    #[error("background division needs a positive model mean, got {mean}")]
    NonPositiveBackground { mean: f64 },
    /// An auto stretch cannot map the measured background onto its target: the median sits at or
    /// below zero (nothing for a brightening curve to lift), or the curve's range does not reach
    /// the target from it.
    #[error("{method} cannot map a background median of {median} to the target {target}")]
    UnreachableBackground {
        method: AutoStretch,
        median: f32,
        target: f32,
    },
}

/// Which automatic stretch could not place the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStretch {
    /// [`StretchMethod::AutoStf`](crate::StretchMethod::AutoStf).
    Stf,
    /// [`StretchMethod::AutoAsinh`](crate::StretchMethod::AutoAsinh).
    Asinh,
}

impl fmt::Display for AutoStretch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stf => "auto STF",
            Self::Asinh => "auto asinh",
        })
    }
}

#[cfg(feature = "ml")]
/// Why an ML filter failed.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("image must be at least {WINDOW}×{WINDOW}, got {}×{}", .0.width, .0.height)]
    TooSmall(Size2us),
    #[error("invalid ML tiling configuration: {0}")]
    Config(#[from] InvalidConfigField),
    #[error("ONNX model error: {0}")]
    Model(#[from] ort::Error),
    /// The model's output is not one NHWC `[1, WINDOW, WINDOW, 3]` tile.
    #[error(
        "model output has {len} values, expected {} (NHWC [1,{WINDOW},{WINDOW},3])",
        WINDOW * WINDOW * 3
    )]
    OutputLength { len: usize },
}
