//! [`Weighting`]: how each frame weighs in the mean.

/// Frame weighting strategy.
///
/// No manual weights: a weight is per-frame data, which a configuration shared by every frame of
/// a role does not hold. IRAF `imcombine`, ccdproc and PixInsight state one per frame where they
/// take one; a caller with such weights would give them on the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weighting {
    /// Equal weights for all frames.
    Equal,
    /// Each frame's inverse noise variance per channel, `w = 1/(gain·σ)²`: σ is the white noise
    /// measured on the frame (by the multiresolution estimator, or per color on a mosaic) and
    /// `gain` its normalization. Not normalized, so the weight plane is an inverse variance. A
    /// frame with no measured noise is an error.
    Noise,
}
