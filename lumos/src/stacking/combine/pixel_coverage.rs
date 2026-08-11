//! The rule for whether one frame contributes at one pixel.

/// How much of one output pixel had real source data behind it, for one frame — a fraction in
/// `[0, 1]`, or [`Self::FULL`] for a frame that carries no warp quality at all.
///
/// The one place the "does this frame contribute here?" rule lives. Three passes ask it: the
/// combine gathering the samples at a pixel, the coverage plane counting that pixel's
/// contributors, and normalization intersecting the pixels every frame reached. They have to agree
/// frame for frame, or the stacked value, the coverage reported beside it, and the scale it was
/// measured on describe three different sets of frames.
///
/// Confidence is not part of the rule. A warp emits support and interpolation confidence together
/// and agreeing on where the frame has data — the invariant
/// [`WarpQuality`](crate::stacking::frame_store::warp_quality::WarpQuality) documents and
/// [`validate_warp_quality`](crate::stacking::combine::cache::validation::validate_warp_quality)
/// enforces — so a pixel over the floor below is guaranteed a positive confidence to weight it by,
/// and gating on that as well would only restate it. Confidence scales a contribution; coverage
/// decides whether there is one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelCoverage(f32);

impl PixelCoverage {
    /// Coverage at or below this is dominated by warp border fill rather than source data, so it is
    /// kept out of the statistics.
    pub(crate) const MIN_CONTRIBUTING: f32 = 1e-3;

    /// The support a frame with no coverage plane has everywhere: a calibration frame, or a light
    /// read straight from disk.
    pub(crate) const FULL: Self = Self(1.0);

    #[inline]
    pub(crate) fn new(fraction: f32) -> Self {
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
    use crate::stacking::combine::pixel_coverage::PixelCoverage;

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
        assert!(PixelCoverage::FULL.contributes());
    }
}
