//! [`AdmissionError`]: why a frame was refused at the checks every frame of a set passes.

use thiserror::Error;

use crate::error::Cancelled;
use crate::frame::frame_plane::FramePlane;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::error::FrameDimensionMismatch;
use crate::image::image_provenance::RowOrder;
use crate::image::sample_domain::SampleDomain;

/// Why frame `index` of a set failed the checks every frame passes before it is combined: its
/// geometry, the facts it must state alike with the others, its samples and its quality planes.
#[derive(Debug, Error)]
pub enum AdmissionError {
    #[error(transparent)]
    DimensionMismatch(#[from] FrameDimensionMismatch),

    /// A frame already in the frame store does not match the geometry the cache was built for.
    /// Reported as a plane count and sample counts rather than as
    /// [`ImageDimensions`](crate::ImageDimensions) because a stored plane knows only its length —
    /// it has no width or height to report.
    #[error("stored frame {index} has {actual} channel planes, expected {expected}")]
    StoredFrameChannels {
        index: usize,
        expected: usize,
        actual: usize,
    },

    #[error("stored frame {index} {plane} holds {actual} samples, expected {expected}")]
    StoredFramePlaneSamples {
        index: usize,
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
        "frame {index} was decoded into sample domain {actual}, but frame {reference_index} used \
         {expected}; neither can be expressed in the other"
    )]
    SampleDomainMismatch {
        index: usize,
        actual: Box<SampleDomain>,
        reference_index: usize,
        expected: Box<SampleDomain>,
    },

    /// Two frames store their rows from opposite ends, so they are mirrored views of one field.
    ///
    /// The rows are never reordered on decode, so a `BOTTOM-UP` frame and a `TOP-DOWN` one of the
    /// same target are upside-down relative to each other. Named here rather than left to surface
    /// as a registration failure with no stated cause.
    #[error(
        "frame {index} stores its rows {actual}, but frame {reference_index} stores them \
         {expected}; the two are mirrored views and cannot be combined"
    )]
    RowOrderMismatch {
        index: usize,
        actual: RowOrder,
        reference_index: usize,
        expected: RowOrder,
    },

    /// Two frames carry different mosaic patterns, or one is a mosaic and the other is not, so
    /// the same pixel is a different color in each.
    #[error(
        "frame {index} has CFA pattern {actual:?}, but frame {reference_index} has {expected:?}"
    )]
    CfaPatternMismatch {
        index: usize,
        actual: Option<CfaType>,
        reference_index: usize,
        expected: Option<CfaType>,
    },

    #[error("frame {index}, channel {channel}, pixel {pixel} has non-finite image value {value}")]
    NonFiniteImageSample {
        index: usize,
        channel: usize,
        pixel: usize,
        value: f32,
    },

    #[error("{plane} for frame {index} has invalid value {value} at pixel {pixel}")]
    InvalidQualityPlaneValue {
        index: usize,
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
        "frame {index} has {plane} {support} with confidence {confidence} at pixel {pixel}: a pixel has support and confidence together or neither"
    )]
    FrameQualityPairMismatch {
        index: usize,
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
                index,
                expected,
                actual,
            } => Self::StoredFrameChannels {
                index: inputs[index],
                expected,
                actual,
            },
            Self::StoredFramePlaneSamples {
                index,
                plane,
                expected,
                actual,
            } => Self::StoredFramePlaneSamples {
                index: inputs[index],
                plane,
                expected,
                actual,
            },
            Self::SampleDomainMismatch {
                index,
                actual,
                reference_index,
                expected,
            } => Self::SampleDomainMismatch {
                index: inputs[index],
                actual,
                reference_index: inputs[reference_index],
                expected,
            },
            Self::RowOrderMismatch {
                index,
                actual,
                reference_index,
                expected,
            } => Self::RowOrderMismatch {
                index: inputs[index],
                actual,
                reference_index: inputs[reference_index],
                expected,
            },
            Self::CfaPatternMismatch {
                index,
                actual,
                reference_index,
                expected,
            } => Self::CfaPatternMismatch {
                index: inputs[index],
                actual,
                reference_index: inputs[reference_index],
                expected,
            },
            Self::NonFiniteImageSample {
                index,
                channel,
                pixel,
                value,
            } => Self::NonFiniteImageSample {
                index: inputs[index],
                channel,
                pixel,
                value,
            },
            Self::InvalidQualityPlaneValue {
                index,
                plane,
                pixel,
                value,
            } => Self::InvalidQualityPlaneValue {
                index: inputs[index],
                plane,
                pixel,
                value,
            },
            Self::FrameQualityPairMismatch {
                index,
                pixel,
                plane,
                support,
                confidence,
            } => Self::FrameQualityPairMismatch {
                index: inputs[index],
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
