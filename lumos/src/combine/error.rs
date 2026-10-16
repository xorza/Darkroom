//! The errors of a stack: its configuration's and its run's.

use std::convert::Infallible;

use thiserror::Error;

use crate::error::Cancelled;
use crate::error::InvalidConfigField;
use crate::frame::frame_plane::FramePlane;
use crate::frame_store::error::{FrameStoreError, LoadError};
use crate::image::error::FrameDimensionMismatch;
use crate::ingest::error::AdmissionError;
use crate::io::error::ImageError;
use crate::math::size2us::Size2us;

/// Invalid [`crate::StackConfig`] parameters.
///
/// Plain range checks report through [`InvalidConfigField`]; the variants below are the
/// constraints that aren't one — a per-element check and two that span fields.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StackConfigError {
    #[error(transparent)]
    Field(#[from] InvalidConfigField),

    #[error(
        "manual weight {index} must be finite and positive, got {value}: drop a frame from the set rather than weigh it 0"
    )]
    InvalidManualWeight { index: usize, value: f32 },

    #[error("manual weights must contain at least one value, with a finite sum")]
    InvalidManualWeightSum,

    #[error("manual weight count {actual} does not match frame count {expected}")]
    ManualWeightCountMismatch { expected: usize, actual: usize },

    #[error("small-stack fallback must not use pixel rejection")]
    RejectingSmallNFallback,
}

/// Errors that can occur during stacking operations.
#[derive(Debug, Error)]
pub enum StackError {
    #[error(transparent)]
    Config(#[from] StackConfigError),

    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),

    #[error("no frames provided")]
    NoFrames,

    #[error("cancelled")]
    Cancelled,

    #[error("registered frames have no pixels with common valid warp support")]
    NoCommonCoverage,

    /// Multiplicative normalization divides by each frame's median, and this one is not positive.
    #[error(
        "frame {index} has median {median} in slot {slot}; a multiplicative normalization needs a positive one"
    )]
    NonPositiveMedian {
        index: usize,
        /// A channel, or a color of a mosaic.
        slot: usize,
        median: f32,
    },

    /// A normalization that scales each frame by its own spread found this one's spread no larger
    /// than its white noise: the slot holds no signal structure to scale by.
    #[error(
        "frame {index} has spread {spread} in slot {slot}, no larger than its white noise {noise}; \
         it holds no signal to scale a normalization by"
    )]
    NoSignalScale {
        index: usize,
        /// A channel, or a color of a mosaic.
        slot: usize,
        spread: f32,
        noise: f32,
    },

    /// `Weighting::Noise` weighs each frame by its inverse noise variance, and this frame measured
    /// none: only synthetic data has no noise at all.
    #[error("frame {index} has no measured noise to weight by; use Equal or Manual weighting")]
    NoNoiseToWeigh { index: usize },

    /// [`RejectionScale::CcdModel`](crate::RejectionScale::CcdModel) holds each sample to its own
    /// CCD noise, whose photon term needs the frame's gain, and this frame, the first of the set,
    /// states none.
    #[error(
        "frame {index} states no gain, which CCD-model rejection needs; use Robust rejection or \
         give every frame its gain"
    )]
    UnknownGain { index: usize },

    /// A file that could not be decoded, held as the decoder's own error — which already names the
    /// path, and stays matchable on *which* decode failed. Wrapping it in a variant of our own
    /// printed the path twice and flattened the cause into an `io::Error`. A decode that was
    /// cancelled arrives as [`Self::Cancelled`], never here.
    #[error(transparent)]
    ImageLoad(ImageError),

    #[error("{plane} dimensions for frame {index} do not match: expected {expected}, got {actual}")]
    PlaneDimensionMismatch {
        index: usize,
        plane: FramePlane,
        expected: Size2us,
        actual: Size2us,
    },

    /// A frame failed the checks every frame of a set passes before it is combined.
    #[error(transparent)]
    Admission(AdmissionError),
}

impl StackConfigError {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) const fn renumbered(self, inputs: &[usize]) -> Self {
        match self {
            Self::InvalidManualWeight { index, value } => Self::InvalidManualWeight {
                index: inputs[index],
                value,
            },
            error @ (Self::Field(_)
            | Self::InvalidManualWeightSum
            | Self::ManualWeightCountMismatch { .. }
            | Self::RejectingSmallNFallback) => error,
        }
    }
}

impl StackError {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) fn renumbered(self, inputs: &[usize]) -> Self {
        match self {
            Self::Config(error) => Self::Config(error.renumbered(inputs)),
            Self::NonPositiveMedian {
                index,
                slot,
                median,
            } => Self::NonPositiveMedian {
                index: inputs[index],
                slot,
                median,
            },
            Self::NoSignalScale {
                index,
                slot,
                spread,
                noise,
            } => Self::NoSignalScale {
                index: inputs[index],
                slot,
                spread,
                noise,
            },
            Self::NoNoiseToWeigh { index } => Self::NoNoiseToWeigh {
                index: inputs[index],
            },
            Self::UnknownGain { index } => Self::UnknownGain {
                index: inputs[index],
            },
            Self::PlaneDimensionMismatch {
                index,
                plane,
                expected,
                actual,
            } => Self::PlaneDimensionMismatch {
                index: inputs[index],
                plane,
                expected,
                actual,
            },
            Self::Admission(error) => Self::Admission(error.renumbered(inputs)),
            error @ (Self::FrameStore(_)
            | Self::NoFrames
            | Self::Cancelled
            | Self::NoCommonCoverage
            | Self::ImageLoad(_)) => error,
        }
    }
}

impl From<FrameDimensionMismatch> for StackError {
    fn from(mismatch: FrameDimensionMismatch) -> Self {
        Self::Admission(AdmissionError::DimensionMismatch(mismatch))
    }
}

/// A frame refused at admission is the run's refusal, a cancel inside the checks the run's cancel.
impl From<AdmissionError> for StackError {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::Cancelled => Self::Cancelled,
            error => Self::Admission(error),
        }
    }
}

/// Cancellation is the run stopping, not a file failing, so a cancelled decode leaves the load
/// error behind and reports as the stack's own.
impl From<ImageError> for StackError {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::Cancelled { .. } => Self::Cancelled,
            error => Self::ImageLoad(error),
        }
    }
}

impl From<LoadError> for StackError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::Decode(error) => error.into(),
            LoadError::Store(error) => error.into(),
        }
    }
}

/// The error of a step no run takes.
impl From<Infallible> for StackError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

impl From<Cancelled> for StackError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;
    use std::path::PathBuf;

    use crate::combine::error::*;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::image_provenance::RowOrder;

    /// Survivors 0, 1 and 2 are inputs 0, 3 and 5: a frame index and a reference index both read
    /// through the list, and an error that names no frame comes back as it was.
    #[test]
    fn a_renumbered_error_names_the_callers_frames() {
        let inputs = [0, 3, 5];
        let error = StackError::Admission(AdmissionError::RowOrderMismatch {
            index: 2,
            actual: RowOrder::BottomUp,
            reference_index: 1,
            expected: RowOrder::TopDown,
        })
        .renumbered(&inputs);
        assert!(
            matches!(
                error,
                StackError::Admission(AdmissionError::RowOrderMismatch {
                    index: 5,
                    reference_index: 3,
                    ..
                })
            ),
            "{error:?}"
        );
        assert!(matches!(
            StackError::NoNoiseToWeigh { index: 1 }.renumbered(&inputs),
            StackError::NoNoiseToWeigh { index: 3 }
        ));
        assert!(matches!(
            StackError::NoCommonCoverage.renumbered(&inputs),
            StackError::NoCommonCoverage
        ));
    }

    #[test]
    fn each_plane_carries_its_own_range_and_label() {
        for (plane, value, accepted) in [
            // Coverage is the fraction of the pixel that had support.
            (FramePlane::Coverage, 0.0, true),
            (FramePlane::Coverage, 1.0, true),
            (FramePlane::Coverage, -0.001, false),
            (FramePlane::Coverage, 1.001, false),
            // Confidence is an interpolation weight — non-negative, no upper bound.
            (FramePlane::Confidence, 0.0, true),
            (FramePlane::Confidence, 5.0, true),
            (FramePlane::Confidence, -0.001, false),
            // A calibrated channel may sit below zero once the dark is subtracted.
            (FramePlane::Channel, -1000.0, true),
            (FramePlane::Channel, 1000.0, true),
        ] {
            assert_eq!(plane.accepts(value), accepted, "{plane} accepting {value}");
        }

        for plane in [
            FramePlane::Channel,
            FramePlane::Coverage,
            FramePlane::Confidence,
        ] {
            assert!(!plane.accepts(f32::NAN), "{plane} accepted NaN");
            assert!(!plane.accepts(f32::INFINITY), "{plane} accepted infinity");
        }

        // The labels reach users through the error messages below.
        assert_eq!(FramePlane::Channel.to_string(), "a channel");
        assert_eq!(FramePlane::Coverage.to_string(), "coverage");
        assert_eq!(FramePlane::Confidence.to_string(), "confidence");
    }

    #[test]
    fn no_frames_error_message() {
        let err = StackError::NoFrames;
        assert_eq!(err.to_string(), "no frames provided");
        assert_eq!(
            StackError::NoCommonCoverage.to_string(),
            "registered frames have no pixels with common valid warp support"
        );
    }

    /// A load failure travels as the decoder's own error: the path appears once, the variant stays
    /// matchable on which decode failed, and `source()` reaches the real cause. Wrapping it in a
    /// `{ path, source: io::Error }` of our own printed "failed to load image '{p}': failed to read
    /// file '{p}': …" and flattened the `ImageError` into a string.
    #[test]
    fn a_load_failure_states_the_path_once_and_stays_typed() {
        let path = PathBuf::from("/path/to/image.fits");
        let error = StackError::from(ImageError::Io {
            path,
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        });

        let message = error.to_string();
        assert_eq!(
            message,
            "failed to read file '/path/to/image.fits': file not found"
        );
        assert_eq!(message.matches("/path/to/image.fits").count(), 1);
        assert!(matches!(
            error,
            StackError::ImageLoad(ImageError::Io { .. })
        ));

        let source = error.source().expect("the decode's own cause");
        assert!(source.downcast_ref::<io::Error>().is_some(), "{source}");

        let cancelled = StackError::from(ImageError::Cancelled {
            path: PathBuf::from("/path/to/image.fits"),
        });
        assert!(matches!(cancelled, StackError::Cancelled), "{cancelled:?}");
    }

    /// The variants that carry a shared payload print exactly what the payload prints — the wording
    /// lives with the type that owns it, not copied into each subsystem's error.
    #[test]
    fn shared_payloads_are_reported_transparently() {
        let store = StackError::from(FrameStoreError::WriteFile {
            path: PathBuf::from("/tmp/cache/frame.bin"),
            source: io::Error::other("disk full"),
        });
        assert_eq!(
            store.to_string(),
            "failed to write frame-store file '/tmp/cache/frame.bin': disk full"
        );

        let mismatch = FrameDimensionMismatch::check(
            5,
            ImageDimensions::new((100, 100), ChannelLayout::Rgb),
            ImageDimensions::new((200, 100), ChannelLayout::Rgb),
        )
        .unwrap_err();
        assert_eq!(
            StackError::from(mismatch).to_string(),
            "frame 5 is 200x100x3, expected 100x100x3"
        );
    }
}
