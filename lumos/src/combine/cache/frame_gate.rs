//! [`FrameGate`]: where the combine gathers one frame's samples over a chunk, and at what
//! confidence.

use crate::combine::pixel_coverage::PixelCoverage;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::stored_frame::StoredFrame;
use crate::io::image::pixel_flags::QualityFlags;

/// One frame's quality over a chunk of pixels: whether its sample at each is gathered, the
/// confidence that divides the sample's noise there, and the weight that multiplies its frame's.
/// The one reading of a frame's quality, which the combine, its coverage plane and normalization's
/// common domain all gather by, so the three describe one set of frames at every pixel.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FrameGate<'a> {
    /// Every pixel, at unit confidence: a frame that carries no frame quality.
    Everywhere,
    /// Where the coverage clears [`PixelCoverage`]'s floor, at the confidence beside it.
    Planes {
        coverage: &'a [f32],
        confidence: &'a [f32],
    },
    /// Where the flags hold none of `excluded`, at unit confidence.
    Mask {
        flags: &'a [u8],
        excluded: QualityFlags,
    },
    /// Where drops landed, a positive `weight`, at the drops' Kish size, weighted by their sum.
    Drizzled {
        weight: &'a [f32],
        confidence: &'a [f32],
    },
}

/// What a frame's gathered sample carries beside its value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GateSample {
    /// The factor that divides the sample's noise.
    pub(crate) confidence: f32,
    /// The factor that multiplies the frame's weight in the mean: 1 but for a drizzled frame,
    /// whose drops weigh what they deposited there.
    pub(crate) weight: f32,
}

impl<'a> FrameGate<'a> {
    /// `frame`'s gate over the pixels `start..end`.
    pub(crate) fn of(frame: &'a StoredFrame, start: usize, end: usize) -> Self {
        match &frame.quality {
            FrameQuality::None => Self::Everywhere,
            FrameQuality::Planes {
                coverage,
                confidence,
            } => Self::Planes {
                coverage: coverage.chunk(start, end),
                confidence: confidence.chunk(start, end),
            },
            FrameQuality::Mask { excluded } => Self::Mask {
                flags: frame
                    .flags
                    .as_ref()
                    .expect("a masked frame stores the flags its mask reads")
                    .chunk(start, end),
                excluded: *excluded,
            },
            FrameQuality::Drizzled { weight, confidence } => Self::Drizzled {
                weight: weight.chunk(start, end),
                confidence: confidence.chunk(start, end),
            },
        }
    }

    /// The sample at `index` of the chunk when it is gathered; `None` when it is not.
    ///
    /// A pair that agrees on where the frame has data — the invariant
    /// [`FrameQuality`] documents — gives every gathered sample a positive confidence.
    #[inline]
    pub(crate) fn sample(&self, index: usize) -> Option<GateSample> {
        let whole = |confidence| GateSample {
            confidence,
            weight: 1.0,
        };
        match self {
            Self::Everywhere => Some(whole(1.0)),
            Self::Planes {
                coverage,
                confidence,
            } => PixelCoverage::new(coverage[index])
                .contributes()
                .then(|| whole(confidence[index])),
            Self::Mask { flags, excluded } => {
                (!QualityFlags::from_byte(flags[index]).intersects(*excluded)).then(|| whole(1.0))
            }
            Self::Drizzled { weight, confidence } => (weight[index] > 0.0).then(|| GateSample {
                confidence: confidence[index],
                weight: weight[index],
            }),
        }
    }

    /// Whether the frame is gathered at every pixel.
    pub(crate) const fn everywhere(&self) -> bool {
        matches!(self, Self::Everywhere)
    }
}
