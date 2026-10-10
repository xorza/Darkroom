//! StackError types for stacking operations.

use thiserror::Error;

use crate::calibration_masters::error::DarkMismatch;
use crate::calibration_masters::master_role::MasterRole;
use crate::error::{FrameDimensionMismatch, InvalidConfigField};
use crate::frame_store::error::{ConditionMismatch, FrameStoreError};
use crate::frame_store::frame_quality::FramePlane;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::CfaType;
use crate::io::image::error::ImageError;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;
use crate::math::size2us::Size2us;

/// Invalid [`crate::StackConfig`] parameters.
///
/// Plain range checks report through [`InvalidConfigField`]; the variants below are the
/// constraints that aren't one — a per-element check and two that span fields.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StackConfigError {
    #[error(transparent)]
    Field(#[from] InvalidConfigField),

    #[error("manual weight {index} must be finite and non-negative, got {value}")]
    InvalidManualWeight { index: usize, value: f32 },

    #[error("manual weights must contain at least one positive value with a finite sum")]
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

    #[error("No frames provided for stacking")]
    NoFrames,

    #[error("stacking cancelled")]
    Cancelled,

    #[error("registered frames have no pixels with common valid warp support")]
    NoCommonCoverage,

    /// Multiplicative normalization divides by each frame's median, and this one is not positive.
    #[error(
        "frame {index} has median {median} in channel {channel}; a multiplicative normalization needs a positive one"
    )]
    NonPositiveMedian {
        index: usize,
        channel: usize,
        median: f32,
    },

    /// The master a calibration stack subtracts from every frame is of another sensor shape or
    /// pattern than frame `index`.
    #[error(
        "frame {index} is {frame} on its sensor, but the master it subtracts is {subtractor}, or of another pattern"
    )]
    SubtractorShape {
        index: usize,
        frame: Size2us,
        subtractor: Size2us,
    },

    /// The master a calibration stack subtracts cannot be expressed in frame `index`'s domain.
    #[error(
        "frame {index} was decoded into sample domain {frame}, which the subtracted master's {subtractor} cannot be converted to"
    )]
    SubtractorDomain {
        index: usize,
        frame: Box<SampleDomain>,
        subtractor: Box<SampleDomain>,
    },

    /// The master a calibration stack would subtract from every frame lost, when it was itself
    /// stacked, everything its role holds, or more than its role may lose: a bias anything, a dark
    /// or a flat-dark more than its bias.
    #[error("the {subtractor} master is calibrated past anything it could subtract")]
    OverCalibratedSubtractor { subtractor: MasterRole },

    /// The master a calibration stack would subtract holds more than a frame of the stacked role
    /// may lose before it is combined: a dark or a flat-dark only its bias, a bias nothing, and no
    /// frame its flat response.
    #[error(
        "the {subtractor} master holds more than a {target} frame may lose before it is combined"
    )]
    SubtractorForRole {
        target: MasterRole,
        subtractor: MasterRole,
    },

    /// Frame `index` already lost a part of what the subtracted master holds: subtracted again,
    /// that part would be removed twice.
    #[error("frame {index} already lost part of what the {subtractor} master holds")]
    SubtractedTwice {
        index: usize,
        subtractor: MasterRole,
    },

    /// The master holding dark signal that a calibration stack subtracts was taken under other
    /// conditions than frame `index`.
    #[error("frame {index} cannot take the subtracted master: {source}")]
    SubtractorConditions { index: usize, source: DarkMismatch },

    /// The frames of a master whose dark signal is one exposure's at one temperature — a dark or a
    /// flat-dark — were not all taken under one: the master would state one frame's conditions and
    /// hold the majority's signal.
    #[error("the {role} frames must share one exposure and one temperature: {source}")]
    MasterConditions {
        role: MasterRole,
        source: ConditionMismatch,
    },

    /// `Weighting::Noise` weighs each frame by its inverse noise variance, and this frame measured
    /// none: only synthetic data has no noise at all.
    #[error("frame {index} has no measured noise to weight by; use Equal or Manual weighting")]
    NoNoiseToWeigh { index: usize },

    /// A file that could not be decoded, held as the decoder's own error — which already names the
    /// path, and stays matchable on *which* decode failed. Wrapping it in a variant of our own
    /// printed the path twice and flattened the cause into an `io::Error`. A decode that was
    /// cancelled arrives as [`Self::Cancelled`], never here.
    #[error(transparent)]
    ImageLoad(#[from] ImageError),

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
    /// the same pixel is a different colour in each.
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

    #[error(
        "{plane} dimensions for frame {index} do not match: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    WarpPlaneDimensionMismatch {
        index: usize,
        plane: FramePlane,
        expected_width: usize,
        expected_height: usize,
        actual_width: usize,
        actual_height: usize,
    },

    #[error("{plane} for frame {index} has invalid value {value} at pixel {pixel}")]
    InvalidWarpPlaneValue {
        index: usize,
        plane: FramePlane,
        pixel: usize,
        value: f32,
    },

    /// The two frame-quality planes disagree about whether the frame has support at a pixel. A warp
    /// produces support and confidence together or neither, and the combine gates on coverage while
    /// weighting by confidence, so a pixel covered at zero confidence would enter the statistics
    /// weightless and one confident at zero coverage would be dropped despite having data.
    #[error(
        "frame {index} has coverage {coverage} with confidence {confidence} at pixel {pixel}: a warped pixel has support and confidence together or neither"
    )]
    FrameQualityPairMismatch {
        index: usize,
        pixel: usize,
        coverage: f32,
        confidence: f32,
    },
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
    use crate::io::image::image_dimensions::ImageDimensions;

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
        assert_eq!(err.to_string(), "No frames provided for stacking");
        assert_eq!(
            StackError::NoCommonCoverage.to_string(),
            "registered frames have no pixels with common valid warp support"
        );
    }

    /// A load failure travels as the decoder's own error: the path appears once, the variant stays
    /// matchable on which decode failed, and `source()` reaches the real cause. Wrapping it in a
    /// `{ path, source: io::Error }` of our own printed "Failed to load image '{p}': Failed to read
    /// file '{p}': …" and flattened the `ImageError` into a string.
    #[test]
    fn a_load_failure_states_the_path_once_and_stays_typed() {
        let path = PathBuf::from("/path/to/image.fits");
        let error = StackError::from(ImageError::Io {
            path: path.clone(),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        });

        let message = error.to_string();
        assert_eq!(
            message,
            "Failed to read file '/path/to/image.fits': file not found"
        );
        assert_eq!(message.matches("/path/to/image.fits").count(), 1);
        assert!(matches!(
            error,
            StackError::ImageLoad(ImageError::Io { .. })
        ));

        let source = error.source().expect("the decode's own cause");
        assert!(source.downcast_ref::<io::Error>().is_some(), "{source}");
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
            ImageDimensions::new((100, 100), 3),
            ImageDimensions::new((200, 100), 3),
        )
        .unwrap_err();
        assert_eq!(
            StackError::from(mismatch).to_string(),
            "frame 5 is 200x100x3, expected 100x100x3"
        );
    }

    #[test]
    fn error_is_debug() {
        let err = StackError::NoFrames;
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("NoFrames"));
    }
}
