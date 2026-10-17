//! [`RejectionScale`]: the unit a sigma clip measures distance in.

/// The unit a sigma clip measures a sample's distance from the median in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RejectionScale {
    /// The spread of the samples — the MAD on the first pass, then a fit to their ranks among every
    /// sample — raised to the frames' noise model at the median.
    #[default]
    Robust,
    /// Each sample's own CCD noise model at the median, as IRAF `imcombine reject=ccdclip` does: a
    /// sample stays while `|x − c| / σᵢ` lies within the band. Nothing is estimated from the
    /// samples, so on a small stack the band does not move with a spread's own noise, and a precise
    /// sample is held to its own σ rather than to the stack's. Its photon term needs each frame's
    /// gain, as `ccdclip` needs one: a set in which a frame states none is refused before the
    /// combine ([`StackError::UnknownGain`](crate::StackError::UnknownGain)), since the background
    /// alone would understate the spread on stars and nebulae and reject their bright samples.
    CcdModel,
}
