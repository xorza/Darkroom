//! [`RejectionScale`]: the unit a sigma clip measures distance in.

/// The unit a sigma clip measures a sample's distance from the median in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RejectionScale {
    /// The MAD of the samples, raised to the frames' measured noise.
    #[default]
    Robust,
    /// The frames' CCD noise model at the median, as IRAF `imcombine reject=ccdclip` does: the
    /// root mean square of each kept sample's model variance there. Nothing is estimated from the
    /// samples, so on a small stack the band does not move with the MAD's own noise. Without a
    /// stated gain the model is the measured background noise alone, which understates the spread
    /// on stars and nebulae.
    CcdModel,
}
