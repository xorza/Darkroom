//! What a registered light knows about how well each of its pixels survived the warp.
//!
//! Two planes that always travel together: how much of an output pixel had real source support,
//! and how confident the interpolation was there. Both are absent for a calibration frame and for
//! a light read straight from disk, which is what lets one combine engine serve all three.

/// Which of a frame's planes a validation failure is about.
///
/// Names the plane in the errors below, and picks the range each one must satisfy: coverage is a
/// fraction of a pixel that had support, confidence an interpolation weight with no upper bound.
/// Carrying the kind rather than its label is what keeps that rule out of a string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePlane {
    /// One of the image's colour planes.
    Channel,
    /// Per-pixel warp support, in `[0, 1]`.
    Coverage,
    /// Per-pixel interpolation confidence, non-negative.
    Confidence,
}

impl FramePlane {
    /// Whether `value` is in range for this plane. Non-finite is out of range for all of them.
    pub(crate) fn accepts(self, value: f32) -> bool {
        value.is_finite()
            && match self {
                // Finiteness is the whole rule for image data, as in `validate_sample_channels`:
                // dark subtraction takes a calibrated channel below zero legitimately.
                Self::Channel => true,
                Self::Coverage => (0.0..=1.0).contains(&value),
                Self::Confidence => value >= 0.0,
            }
    }
}

impl std::fmt::Display for FramePlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Channel => "a channel",
            Self::Coverage => "coverage",
            Self::Confidence => "confidence",
        })
    }
}

/// The per-pixel warp quality a registered light carries: how much of each output pixel had
/// support, and how confident the interpolation was.
///
/// Both are absent for a calibration frame and for a light loaded straight from disk, and they
/// travel together everywhere a frame does — so the two planes are converted, written and named
/// in one step rather than one apiece.
#[derive(Debug, Default)]
pub(crate) struct WarpQuality<P> {
    pub(crate) coverage: Option<P>,
    pub(crate) confidence: Option<P>,
}

impl<P> WarpQuality<P> {
    pub(crate) fn new(coverage: Option<P>, confidence: Option<P>) -> Self {
        Self {
            coverage,
            confidence,
        }
    }

    /// No warp quality at all — a calibration frame, or a light read straight from disk.
    pub(crate) fn none() -> Self {
        Self {
            coverage: None,
            confidence: None,
        }
    }

    pub(crate) fn map<Q>(self, mut convert: impl FnMut(P) -> Q) -> WarpQuality<Q> {
        WarpQuality {
            coverage: self.coverage.map(&mut convert),
            confidence: self.confidence.map(&mut convert),
        }
    }

    /// Every plane the frame actually carries, each with the kind that names it. The one place
    /// that decides what "all the quality planes" means, so a caller cannot enumerate a subset.
    ///
    /// Reads the two fields rather than going through [`FramePlane`], which also names the image
    /// channels and so has a variant this type could only ever answer `None` for.
    pub(crate) fn present(&self) -> impl Iterator<Item = (FramePlane, &P)> {
        [
            (FramePlane::Coverage, self.coverage.as_ref()),
            (FramePlane::Confidence, self.confidence.as_ref()),
        ]
        .into_iter()
        .filter_map(|(kind, plane)| plane.map(|plane| (kind, plane)))
    }

    /// How many planes are present, 0 to 2.
    pub(crate) fn count(&self) -> usize {
        self.present().count()
    }

    /// Whether the frame carries no warp quality at all.
    pub(crate) fn is_none(&self) -> bool {
        self.coverage.is_none() && self.confidence.is_none()
    }

    /// Convert each present plane, tagging it with the name its spill file carries. Which plane
    /// answers to which name is stated here alone, so a writer and a later reader cannot disagree.
    pub(crate) fn try_map<Q, E>(
        self,
        mut convert: impl FnMut(&'static str, P) -> Result<Q, E>,
    ) -> Result<WarpQuality<Q>, E> {
        Ok(WarpQuality {
            coverage: self.coverage.map(|p| convert("coverage", p)).transpose()?,
            confidence: self
                .confidence
                .map(|p| convert("confidence", p))
                .transpose()?,
        })
    }
}
