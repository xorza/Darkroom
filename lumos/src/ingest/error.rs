//! [`AdmissionError`]: why a frame was refused at the checks every frame of a set passes.

use thiserror::Error;

use crate::error::Cancelled;
use crate::frame::frame_plane::FramePlane;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::error::FrameDimensionMismatch;
use crate::image::metadata::image_provenance::RowOrder;
use crate::image::metadata::sample_domain::SampleDomain;

/// Why frame `idx` of a set failed the checks every frame passes before it is combined: its
/// geometry, the facts it must state alike with the others, its samples and its quality planes.
#[derive(Debug, Error)]
pub enum AdmissionError {
    #[error(transparent)]
    DimensionMismatch(#[from] FrameDimensionMismatch),

    /// A frame already in the frame store does not match the geometry the cache was built for.
    /// Reported as a plane count and sample counts rather than as
    /// [`PlanarDesc`](imaginarium::PlanarDesc) because a stored plane knows only its length —
    /// it has no width or height to report.
    #[error("stored frame {idx} has {actual} channel planes, expected {expected}")]
    StoredFrameChannels {
        idx: usize,
        expected: usize,
        actual: usize,
    },

    #[error("stored frame {idx} {plane} holds {actual} samples, expected {expected}")]
    StoredFramePlaneSamples {
        idx: usize,
        plane: FramePlane,
        expected: usize,
        actual: usize,
    },

    /// Two frames were decoded into different sample domains, so combining them would average
    /// values that do not mean the same thing.
    ///
    /// Reached when both frames declare a domain and one cannot be expressed in the other's: a
    /// different stated unit, or a span the decoder had to assume — a `float32` FITS taken as
    /// already normalized stacked against a `uint16` one divided by 65535. Two declared spans in
    /// one unit (two RAWs whose `maximum − black` differ) are converted, not refused.
    #[error(
        "frame {idx} was decoded into sample domain {actual}, but frame {reference_idx} used \
         {expected}; neither can be expressed in the other"
    )]
    SampleDomainMismatch {
        idx: usize,
        actual: Box<SampleDomain>,
        reference_idx: usize,
        expected: Box<SampleDomain>,
    },

    /// Two frames store their rows from opposite ends, so they are mirrored views of one field.
    ///
    /// The rows are never reordered on decode, so a `BOTTOM-UP` frame and a `TOP-DOWN` one of the
    /// same target are upside-down relative to each other. Named here rather than left to surface
    /// as a registration failure with no stated cause.
    #[error(
        "frame {idx} stores its rows {actual}, but frame {reference_idx} stores them \
         {expected}; the two are mirrored views and cannot be combined"
    )]
    RowOrderMismatch {
        idx: usize,
        actual: RowOrder,
        reference_idx: usize,
        expected: RowOrder,
    },

    /// Two frames carry different sensor patterns, or one carries a pattern and the other none,
    /// so the same pixel is a different color in each.
    #[error("frame {idx} has CFA pattern {actual:?}, but frame {reference_idx} has {expected:?}")]
    CfaPatternMismatch {
        idx: usize,
        actual: Option<CfaType>,
        reference_idx: usize,
        expected: Option<CfaType>,
    },

    #[error("frame {idx}, channel {channel}, pixel {pixel} has non-finite image value {value}")]
    NonFiniteImageSample {
        idx: usize,
        channel: usize,
        pixel: usize,
        value: f32,
    },

    #[error("{plane} for frame {idx} has invalid value {value} at pixel {pixel}")]
    InvalidQualityPlaneValue {
        idx: usize,
        plane: FramePlane,
        pixel: usize,
        value: f32,
    },

    /// The two frame-quality planes disagree about whether the frame has support at a pixel. A warp
    /// or a drizzle produces support and confidence together or neither, and the combine gates on
    /// support — `plane`, the coverage or the drop weight — while dividing the noise by
    /// confidence, so a pixel supported at zero confidence would enter the statistics with no
    /// finite noise and one confident at zero support would be dropped despite having data.
    #[error(
        "frame {idx} has {plane} {support} with confidence {confidence} at pixel {pixel}: a pixel has support and confidence together or neither"
    )]
    FrameQualityPairMismatch {
        idx: usize,
        pixel: usize,
        plane: FramePlane,
        support: f32,
        confidence: f32,
    },

    /// The run was cancelled while a frame was being checked.
    #[error("cancelled while checking a frame")]
    Cancelled,
}

impl AdmissionError {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) fn renumbered(self, inputs: &[usize]) -> Self {
        match self {
            Self::DimensionMismatch(mismatch) => {
                Self::DimensionMismatch(mismatch.renumbered(inputs))
            }
            Self::StoredFrameChannels {
                idx,
                expected,
                actual,
            } => Self::StoredFrameChannels {
                idx: inputs[idx],
                expected,
                actual,
            },
            Self::StoredFramePlaneSamples {
                idx,
                plane,
                expected,
                actual,
            } => Self::StoredFramePlaneSamples {
                idx: inputs[idx],
                plane,
                expected,
                actual,
            },
            Self::SampleDomainMismatch {
                idx,
                actual,
                reference_idx,
                expected,
            } => Self::SampleDomainMismatch {
                idx: inputs[idx],
                actual,
                reference_idx: inputs[reference_idx],
                expected,
            },
            Self::RowOrderMismatch {
                idx,
                actual,
                reference_idx,
                expected,
            } => Self::RowOrderMismatch {
                idx: inputs[idx],
                actual,
                reference_idx: inputs[reference_idx],
                expected,
            },
            Self::CfaPatternMismatch {
                idx,
                actual,
                reference_idx,
                expected,
            } => Self::CfaPatternMismatch {
                idx: inputs[idx],
                actual,
                reference_idx: inputs[reference_idx],
                expected,
            },
            Self::NonFiniteImageSample {
                idx,
                channel,
                pixel,
                value,
            } => Self::NonFiniteImageSample {
                idx: inputs[idx],
                channel,
                pixel,
                value,
            },
            Self::InvalidQualityPlaneValue {
                idx,
                plane,
                pixel,
                value,
            } => Self::InvalidQualityPlaneValue {
                idx: inputs[idx],
                plane,
                pixel,
                value,
            },
            Self::FrameQualityPairMismatch {
                idx,
                pixel,
                plane,
                support,
                confidence,
            } => Self::FrameQualityPairMismatch {
                idx: inputs[idx],
                pixel,
                plane,
                support,
                confidence,
            },
            Self::Cancelled => Self::Cancelled,
        }
    }
}

impl From<Cancelled> for AdmissionError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}
