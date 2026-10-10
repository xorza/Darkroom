//! The rule for whether one frame contributes at one pixel.

/// How much of one output pixel had real source data behind it, for one frame: a fraction in
/// `[0, 1]`.
///
/// The one place the "does this frame contribute here?" rule for a coverage plane lives, which
/// [`FrameGate`](crate::combine::cache::frame_gate::FrameGate) reads for the combine, its coverage
/// plane and normalization's common domain alike.
///
/// Confidence is not part of the rule. A warp emits support and interpolation confidence together
/// and agreeing on where the frame has data — the invariant
/// [`FrameQuality`](crate::frame_store::frame_quality::FrameQuality) documents and
/// [`FrameCheck::quality_pair`](crate::combine::cache::frame_check::FrameCheck::quality_pair)
/// enforces — so a pixel over the floor below is guaranteed a positive confidence to divide its
/// noise by, and gating on that as well would only restate it. Confidence scales a sample's noise;
/// coverage decides whether there is a sample.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelCoverage(f32);

impl PixelCoverage {
    /// Coverage at or below this is dominated by warp border fill rather than source data, so it is
    /// kept out of the statistics.
    pub(crate) const MIN_CONTRIBUTING: f32 = 1e-3;

    #[inline]
    pub(crate) const fn new(fraction: f32) -> Self {
        Self(fraction)
    }

    /// Whether this frame's sample at this pixel is data rather than border fill.
    #[inline]
    pub(crate) fn contributes(self) -> bool {
        self.0 > Self::MIN_CONTRIBUTING
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::pixel_coverage::PixelCoverage;

    #[test]
    fn the_border_fill_floor_is_exclusive() {
        for (fraction, contributes) in [
            // No support at all, and the fill just above it, are both out.
            (0.0, false),
            (f32::MIN_POSITIVE, false),
            (PixelCoverage::MIN_CONTRIBUTING, false),
            // A tenth of a percent over the floor is data — the floor keeps out fill, not faint
            // support.
            (PixelCoverage::MIN_CONTRIBUTING * 1.001, true),
            (0.5, true),
            (1.0, true),
        ] {
            assert_eq!(
                PixelCoverage::new(fraction).contributes(),
                contributes,
                "coverage {fraction}"
            );
        }
    }
}
