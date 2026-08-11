//! Error types for stacking operations.

use thiserror::Error;

use common::CancelToken;

use crate::error::{FrameDimensionMismatch, InvalidConfigField};
use crate::io::image::error::ImageError;
use crate::stacking::calibration_masters::CalibrationError;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::warp_quality::FramePlane;

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
pub enum Error {
    #[error(transparent)]
    Config(#[from] StackConfigError),

    #[error(transparent)]
    FrameStore(#[from] FrameStoreError),

    /// A calibration bundle built alongside a stack does not describe one coherent sensor.
    #[error(transparent)]
    Calibration(#[from] CalibrationError),

    #[error("No frames provided for stacking")]
    NoFrames,

    #[error("stacking cancelled")]
    Cancelled,

    #[error("registered frames have no pixels with common valid warp support")]
    NoCommonCoverage,

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
    /// Reached when both frames declare a span and the spans differ — a `uint16` FITS divided by
    /// 65535 stacked against a `float32` one taken as already normalized, or two RAWs whose
    /// `maximum − black` differ. `Normalization::Global` would otherwise absorb the ratio into its
    /// fitted gain and hand back a plausible-looking result.
    #[error(
        "frame {index} was decoded with sample span {actual}, but frame {reference_index} used \
         {expected}; frames divided by different spans cannot be combined"
    )]
    SampleSpanMismatch {
        index: usize,
        actual: f32,
        reference_index: usize,
        expected: f32,
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

    /// The two warp-quality planes disagree about whether the frame has support at a pixel. A warp
    /// produces support and confidence together or neither, and the combine gates on coverage while
    /// weighting by confidence, so a pixel covered at zero confidence would enter the statistics
    /// weightless and one confident at zero coverage would be dropped despite having data.
    #[error(
        "frame {index} has coverage {coverage} with confidence {confidence} at pixel {pixel}: a warped pixel has support and confidence together or neither"
    )]
    WarpQualityPairMismatch {
        index: usize,
        pixel: usize,
        coverage: f32,
        confidence: f32,
    },
}

/// `Err(Error::Cancelled)` once the token is set, so a long walk can `?` its way out between
/// chunks. Free rather than a method because `CancelToken` is another crate's type.
pub(crate) fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;
    use std::path::PathBuf;

    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::stacking::combine::error::*;

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
        let err = Error::NoFrames;
        assert_eq!(err.to_string(), "No frames provided for stacking");
        assert_eq!(
            Error::NoCommonCoverage.to_string(),
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
        let error = Error::from(ImageError::Io {
            path: path.clone(),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        });

        let message = error.to_string();
        assert_eq!(
            message,
            "Failed to read file '/path/to/image.fits': file not found"
        );
        assert_eq!(message.matches("/path/to/image.fits").count(), 1);
        assert!(matches!(error, Error::ImageLoad(ImageError::Io { .. })));

        let source = error.source().expect("the decode's own cause");
        assert!(source.downcast_ref::<io::Error>().is_some(), "{source}");
    }

    /// The variants that carry a shared payload print exactly what the payload prints — the wording
    /// lives with the type that owns it, not copied into each subsystem's error.
    #[test]
    fn shared_payloads_are_reported_transparently() {
        let store = Error::from(FrameStoreError::WriteFile {
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
            Error::from(mismatch).to_string(),
            "frame 5 is 200x100x3, expected 100x100x3"
        );
    }

    #[test]
    fn error_is_debug() {
        let err = Error::NoFrames;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NoFrames"));
    }
}
