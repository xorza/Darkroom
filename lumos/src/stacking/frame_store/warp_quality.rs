//! What a frame knows about how much of a measurement each of its pixels holds.
//!
//! Two planes that always travel together: how much of an output pixel had real source support,
//! and how confident the interpolation was there. A warp is the usual producer, but not the only
//! one — a frame read straight from disk carries them too when its source declared pixels with no
//! measurement. Both are absent when neither applies, which is what lets one combine engine serve
//! every case.

use imaginarium::Buffer2;

use crate::stacking::frame_store::StackableImage;

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

/// The per-pixel quality a frame carries: how much of each pixel had support, and how confident
/// the interpolation that produced it was.
///
/// Both planes or neither. Two things produce the pair — a warp, and a decoder that found pixels
/// its source declared undefined (see [`Self::for_unwarped`]) — every consumer's rule for "does
/// this frame contribute at this pixel?" is about the pair, and a frame with neither carries
/// nothing at all. So a lone plane is not a shape any producer means, and this type cannot hold
/// one.
///
/// The two planes agree pixel by pixel as well: `coverage == 0` exactly where `confidence == 0`.
/// `registration::resample::quality::quality_at` establishes that — outside the source footprint
/// both are zero, and inside it every branch gives a positive confidence wherever there is support
/// — and [`validate_warp_quality`] holds caller-supplied planes to it, because the combine leans on
/// it: a sample that clears the coverage floor is guaranteed a positive confidence to weight it by,
/// and `source_noise_variance` a non-zero one to divide by.
///
/// [`validate_warp_quality`]: crate::stacking::combine::cache::validation::validate_warp_quality
#[derive(Debug)]
pub(crate) enum WarpQuality<P> {
    /// No warp quality at all — a calibration frame, or a light read straight from disk.
    None,
    /// The pair a warped light carries.
    Planes { coverage: P, confidence: P },
}

impl WarpQuality<Buffer2<f32>> {
    /// The quality a frame that was never warped carries.
    ///
    /// [`None`](Self::None) unless its source declared pixels with no measurement, in which case
    /// the mask becomes the pair the combine already knows how to gate on: zero coverage exactly
    /// where a pixel is null, one everywhere else. Confidence is the same plane — nothing was
    /// interpolated, so every sample that exists is a whole one — which is what the type's
    /// pairing invariant asks for.
    ///
    /// The `None` case is what keeps this free for the frames that dominate: a sensor reports a
    /// value for every photosite, so no RAW frame and almost no camera FITS allocates anything
    /// here.
    pub(crate) fn for_unwarped(image: &impl StackableImage) -> Self {
        let Some(nulls) = image.nulls() else {
            return Self::None;
        };
        let coverage = nulls.validity_plane();
        Self::Planes {
            confidence: coverage.clone(),
            coverage,
        }
    }
}

impl<P> WarpQuality<P> {
    /// The frame's per-pixel warp support, or `None` for a frame that carries no warp quality.
    pub(crate) fn coverage(&self) -> Option<&P> {
        match self {
            Self::None => None,
            Self::Planes { coverage, .. } => Some(coverage),
        }
    }

    /// The frame's per-pixel interpolation confidence, or `None` as in [`Self::coverage`].
    pub(crate) fn confidence(&self) -> Option<&P> {
        match self {
            Self::None => None,
            Self::Planes { confidence, .. } => Some(confidence),
        }
    }

    pub(crate) fn map<Q>(self, mut convert: impl FnMut(P) -> Q) -> WarpQuality<Q> {
        match self {
            Self::None => WarpQuality::None,
            Self::Planes {
                coverage,
                confidence,
            } => WarpQuality::Planes {
                coverage: convert(coverage),
                confidence: convert(confidence),
            },
        }
    }

    /// Every plane the frame carries, each with the kind that names it — both or neither. The one
    /// place that decides what "all the quality planes" means, so a caller cannot enumerate a
    /// subset.
    ///
    /// Reads the variant rather than going through [`FramePlane`], which also names the image
    /// channels and so has a variant this type could only ever answer `None` for.
    pub(crate) fn present(&self) -> impl Iterator<Item = (FramePlane, &P)> {
        [
            self.coverage().map(|plane| (FramePlane::Coverage, plane)),
            self.confidence()
                .map(|plane| (FramePlane::Confidence, plane)),
        ]
        .into_iter()
        .flatten()
    }

    /// How many planes are present: 0 or 2.
    pub(crate) fn count(&self) -> usize {
        self.present().count()
    }

    /// Whether the frame carries no warp quality at all.
    pub(crate) fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Convert each plane by reference, tagging it with the name its spill file carries. Which
    /// plane answers to which name is stated here alone, so a writer and a later reader cannot
    /// disagree.
    ///
    /// Borrows rather than consumes because its one caller writes the planes to disk and maps them
    /// back — it never needed to own them, and leaving them with the caller is what lets a warped
    /// frame's buffers be reused for the next frame instead of being freed and faulted in again.
    pub(crate) fn try_map<Q, E>(
        &self,
        mut convert: impl FnMut(&'static str, &P) -> Result<Q, E>,
    ) -> Result<WarpQuality<Q>, E> {
        match self {
            Self::None => Ok(WarpQuality::None),
            Self::Planes {
                coverage,
                confidence,
            } => Ok(WarpQuality::Planes {
                coverage: convert("coverage", coverage)?,
                confidence: convert("confidence", confidence)?,
            }),
        }
    }

    /// Read a pair back from the spill names [`Self::try_map`] wrote it under.
    ///
    /// Its inverse, and here for the same reason: which plane answers to which name is decided in
    /// this file alone, so a writer and a later reader cannot disagree. The caller establishes that
    /// both are there — see [`CachedQuality`](crate::stacking::frame_store::spill::CachedQuality) —
    /// which is why this reads a plane rather than looking for one.
    pub(crate) fn read_spilled<E>(
        mut read: impl FnMut(&'static str) -> Result<P, E>,
    ) -> Result<Self, E> {
        Ok(Self::Planes {
            coverage: read("coverage")?,
            confidence: read("confidence")?,
        })
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::stacking::frame_store::warp_quality::WarpQuality;

    impl WarpQuality<Buffer2<f32>> {
        /// A coverage plane with the confidence plane a warp would have produced beside it: unit
        /// confidence where there is support and zero where there is none, which is the pairing
        /// every consumer relies on. Lets a test about coverage gating state the one plane it
        /// cares about.
        pub(crate) fn from_coverage(coverage: Buffer2<f32>) -> Self {
            let confidence = Buffer2::new(
                coverage.width(),
                coverage.height(),
                coverage
                    .pixels()
                    .iter()
                    .map(|&value| if value > 0.0 { 1.0 } else { 0.0 })
                    .collect(),
            );
            Self::Planes {
                coverage,
                confidence,
            }
        }
    }
}
