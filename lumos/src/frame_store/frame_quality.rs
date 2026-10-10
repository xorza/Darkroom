//! What a frame knows about how much of a measurement each of its pixels holds.
//!
//! Two planes that always travel together: how much of an output pixel had real source support,
//! and how confident the interpolation was there. A warp is the usual producer. A frame nothing
//! interpolated has whole support and unit confidence wherever it has a measurement, so it needs
//! no planes: a mask of its own flags says the same. Both are absent when neither applies, which
//! is what lets one combine engine serve every case.

use imaginarium::Buffer2;

use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// Which of a frame's planes a validation failure is about.
///
/// Names the plane in the errors below, and picks the range each one must satisfy: coverage is a
/// fraction of a pixel that had support, confidence an interpolation's noise factor with no upper
/// bound.
/// Carrying the kind rather than its label is what keeps that rule out of a string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePlane {
    /// One of the image's colour planes.
    Channel,
    /// Per-pixel warp support, in `[0, 1]`.
    Coverage,
    /// Per-pixel interpolation confidence, non-negative.
    Confidence,
    /// A drizzled frame's per-pixel drop weight `Σw`, non-negative.
    DropWeight,
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
                Self::Confidence | Self::DropWeight => value >= 0.0,
            }
    }
}

impl Display for FramePlane {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Channel => "a channel",
            Self::Coverage => "coverage",
            Self::Confidence => "confidence",
            Self::DropWeight => "drop weight",
        })
    }
}

/// The per-pixel quality a frame carries: how much of each pixel had support, and how confident the
/// interpolation that produced it was.
///
/// Both planes, a mask, or neither. A warp produces the pair; a frame nothing interpolated whose
/// flags name pixels it has no measurement for, a mask (see [`Self::for_unwarped`]). Every
/// consumer's rule for "does this frame contribute at this pixel?" is about the pair, and a frame
/// with neither carries nothing at all. So a lone plane is not a shape any producer means, and
/// this type cannot hold one.
///
/// The two planes agree pixel by pixel as well: `coverage == 0` exactly where `confidence == 0`.
/// `registration::resample::frame_sampler` establishes that — a pixel the warp has no sample for
/// takes zero for both, and every sample it takes has positive coverage and confidence — and
/// [`FrameCheck::quality_pair`] holds caller-supplied planes to it, because the combine leans on
/// it: a sample that clears the coverage floor is guaranteed a positive confidence to divide its
/// noise by, and `source_noise_variance` a non-zero one too.
///
/// [`FrameCheck::quality_pair`]: crate::combine::cache::frame_check::FrameCheck::quality_pair
#[derive(Debug, Clone)]
pub(crate) enum FrameQuality<P> {
    /// No quality planes at all — a frame that was never warped and whose source declared every
    /// pixel measured, which is the overwhelming majority of them.
    None,
    /// The pair a warped light carries.
    Planes { coverage: P, confidence: P },
    /// Whole support and unit confidence where the frame's flags hold none of `excluded`, none
    /// elsewhere: the pair of a frame nothing interpolated, read from the flags it stores beside
    /// its channels instead of two planes of the same bit.
    Mask { excluded: QualityFlags },
    /// A frame drizzled on its own: where its drops landed, `weight` is their summed weight `Σw`,
    /// which the combine multiplies the frame's weight by, and `confidence` their Kish size
    /// `(Σw)²/Σw²`, the factor the frame's noise is divided by. Zero for both where none landed,
    /// which is where the frame is not gathered.
    Drizzled { weight: P, confidence: P },
}

impl FrameQuality<Buffer2<f32>> {
    /// The quality a frame that was never warped carries.
    ///
    /// [`None`](Self::None) unless its source declared pixels with no measurement, in which case
    /// a [`Mask`](Self::Mask) of its nulls: zero coverage exactly where a pixel is null, one
    /// everywhere else. Confidence is the same — nothing was interpolated, so every sample that
    /// exists is a whole one — which is what the type's pairing invariant asks for.
    ///
    /// The `None` case is what keeps this free for the frames that dominate: a sensor reports a
    /// value for every photosite, so no RAW frame and almost no camera FITS allocates anything
    /// here.
    pub(crate) fn for_unwarped(image: &impl StackableImage) -> Self {
        Self::excluding(image.flags(), QualityFlags::NO_DATA)
    }

    /// The quality the reference of a registered stack carries, from its flags: zero coverage
    /// wherever a flag of [`QualityFlags::RESAMPLE_EXCLUDED`] stands, as the warp of every other
    /// frame leaves those pixels out. The reference is the frame a warp would sample at whole
    /// pixels, where every other tap weighs exactly zero, so its excluded pixels are left out
    /// exactly, and its carried flags stand where they are.
    pub(crate) fn for_reference(flags: Option<&PixelFlags>) -> Self {
        Self::excluding(flags, QualityFlags::RESAMPLE_EXCLUDED)
    }

    /// The [`Mask`](Self::Mask) of `excluded` when a pixel of `flags` holds one, and
    /// [`None`](Self::None) when none does.
    fn excluding(flags: Option<&PixelFlags>, excluded: QualityFlags) -> Self {
        if flags.is_some_and(|flags| flags.contains(excluded)) {
            Self::Mask { excluded }
        } else {
            Self::None
        }
    }
}

impl<P> FrameQuality<P> {
    /// The frame's per-pixel warp support, or `None` for a frame that carries no such plane.
    pub(crate) const fn coverage(&self) -> Option<&P> {
        match self {
            Self::None | Self::Mask { .. } | Self::Drizzled { .. } => None,
            Self::Planes { coverage, .. } => Some(coverage),
        }
    }

    /// The factor the frame's noise is divided by at each pixel — a warp's interpolation
    /// confidence, a drizzle's Kish size — or `None` for a frame that carries no such plane.
    pub(crate) const fn confidence(&self) -> Option<&P> {
        match self {
            Self::None | Self::Mask { .. } => None,
            Self::Planes { confidence, .. } | Self::Drizzled { confidence, .. } => Some(confidence),
        }
    }

    /// The flags a [`Mask`](Self::Mask) leaves the frame out where; `None` for another form.
    pub(crate) const fn mask(&self) -> Option<QualityFlags> {
        match self {
            Self::Mask { excluded } => Some(*excluded),
            Self::None | Self::Planes { .. } | Self::Drizzled { .. } => None,
        }
    }

    pub(crate) fn map<Q>(self, mut convert: impl FnMut(P) -> Q) -> FrameQuality<Q> {
        match self {
            Self::None => FrameQuality::None,
            Self::Mask { excluded } => FrameQuality::Mask { excluded },
            Self::Drizzled { weight, confidence } => FrameQuality::Drizzled {
                weight: convert(weight),
                confidence: convert(confidence),
            },
            Self::Planes {
                coverage,
                confidence,
            } => FrameQuality::Planes {
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
        let weight = match self {
            Self::Drizzled { weight, .. } => Some((FramePlane::DropWeight, weight)),
            Self::None | Self::Mask { .. } | Self::Planes { .. } => None,
        };
        [
            self.coverage().map(|plane| (FramePlane::Coverage, plane)),
            weight,
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

    /// Whether the frame carries no frame quality at all.
    pub(crate) const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Convert each plane by reference, tagging it with the [`FramePlane`] it is. Which plane is
    /// which is stated here alone, so a writer and a later reader cannot disagree.
    ///
    /// Borrows rather than consumes because its one caller writes the planes to disk and maps them
    /// back — it never needed to own them, and leaving them with the caller is what lets a warped
    /// frame's buffers be reused for the next frame instead of being freed and faulted in again.
    pub(crate) fn try_map<Q, E>(
        &self,
        mut convert: impl FnMut(FramePlane, &P) -> Result<Q, E>,
    ) -> Result<FrameQuality<Q>, E> {
        match self {
            Self::None => Ok(FrameQuality::None),
            Self::Mask { excluded } => Ok(FrameQuality::Mask {
                excluded: *excluded,
            }),
            Self::Drizzled { weight, confidence } => Ok(FrameQuality::Drizzled {
                weight: convert(FramePlane::DropWeight, weight)?,
                confidence: convert(FramePlane::Confidence, confidence)?,
            }),
            Self::Planes {
                coverage,
                confidence,
            } => Ok(FrameQuality::Planes {
                coverage: convert(FramePlane::Coverage, coverage)?,
                confidence: convert(FramePlane::Confidence, confidence)?,
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::frame_store::frame_quality::FrameQuality;

    impl FrameQuality<Buffer2<f32>> {
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
