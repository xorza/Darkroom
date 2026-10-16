//! [`Weighting`]: how each frame weighs in the mean.

/// Frame weighting strategy.
#[derive(Debug, Clone, PartialEq)]
pub enum Weighting {
    /// Equal weights for all frames.
    Equal,
    /// Each frame's inverse noise variance per channel, `w = 1/(gain·σ)²`: σ is the white noise
    /// measured on the frame (by the multiresolution estimator, or per color on a mosaic) and
    /// `gain` its normalization. Not normalized, so the weight plane is an inverse variance. A
    /// frame with no measured noise is an error.
    Noise,
    /// Explicit per-frame weights provided by the user.
    Manual(Vec<f32>),
}
