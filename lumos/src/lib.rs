//! Lumos - Astronomical image processing library.
//!
//! This library provides tools for processing astronomical images, including:
//! - Star detection and centroiding
//! - Image registration and alignment
//! - Frame stacking (mean, median, sigma-clipped)
//! - Calibration frame handling (darks, flats, bias)
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use lumos::{AstroImage, StarDetectionConfig, StarDetector};
//!
//! // Load an astronomical image
//! let image = AstroImage::from_file("light_001.fits")?;
//!
//! // Detect stars
//! let config = StarDetectionConfig::default();
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! ```

pub(crate) mod background_mesh;
pub(crate) mod concurrency;
pub(crate) mod image_ops;
pub(crate) mod io;
pub(crate) mod math;
pub(crate) mod stacking;

#[cfg(test)]
pub mod testing;

pub use io::astro_image::cfa::{CfaImage, CfaType};
pub use io::astro_image::error::ImageError;
pub use io::astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
pub use io::raw::demosaic::bayer::CfaPattern;
pub use io::raw::{load_raw, load_raw_cfa};
pub use stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, NoiseEstimation};
pub use stacking::calibration_masters::defect_map::DefectMap;

pub use stacking::calibration_masters::{
    CalibrationComponent, CalibrationFrames, CalibrationImages, CalibrationMasters,
    DEFAULT_SIGMA_THRESHOLD, DefectSummary, stack_cfa_master,
};

pub use stacking::star_detection::config::{
    BackgroundRefinement, CentroidMethod, Config as StarDetectionConfig, Connectivity,
    LocalBackgroundMethod, NoiseModel,
};
pub use stacking::star_detection::detector::{
    DetectionResult as StarDetectionResult, Diagnostics as StarDetectionDiagnostics, StarDetector,
};
pub use stacking::star_detection::error::StarDetectionConfigError;
pub use stacking::star_detection::star::Star;

pub use stacking::registration::config::{Config as RegistrationConfig, InterpolationMethod};
pub use stacking::registration::distortion::sip::SipPolynomial;
pub use stacking::registration::result::{
    RansacFailureReason, RegistrationError, RegistrationResult,
};
pub use stacking::registration::transform::{Transform, TransformType, WarpTransform};
pub use stacking::registration::{WarpResult, register, warp};

pub use stacking::combine::cache_config::CacheConfig;
pub use stacking::combine::config::{CombineMethod, Normalization, SmallN, StackConfig, Weighting};
pub use stacking::combine::error::{Error as StackError, StackConfigError};
pub use stacking::combine::rejection::{
    GesdConfig, LinearFitClipConfig, PercentileClipConfig, Rejection, SigmaClipConfig,
    WinsorizedClipConfig,
};
pub use stacking::combine::stack::{StackFrame, stack, stack_images};
pub use stacking::frame_store::FrameStoreError;
pub use stacking::product::StackProduct;
pub use stacking::progress::{ProgressCallback, StackingProgress, StackingStage};

pub use stacking::pipeline::align::align_and_stack;
pub use stacking::pipeline::config::{AlignStackConfig, Reference};
pub use stacking::pipeline::result::{
    AlignStackResult, AlignmentSummary, Error as AlignStackError,
};
pub use stacking::pipeline::streaming::calibrate_align_stack;

pub use stacking::drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
pub use stacking::drizzle::config::{DrizzleConfig, DrizzleKernel};
pub use stacking::drizzle::error::{DrizzleConfigError, DrizzleError};
pub use stacking::drizzle::stack::{drizzle_images, drizzle_stack};

pub use image_ops::stretching::{ColorMode, Stretch, StretchMethod};

pub use image_ops::color_calibration::{NeutralizeBackground, Scnr};

pub use image_ops::background_extraction::{BackgroundMode, ExtractBackground};

pub use image_ops::denoise::{Denoise, Threshold};

pub use image_ops::local_contrast::LocalContrast;

pub use image_ops::hdr::Hdr;

pub use image_ops::op::OpError;

#[cfg(feature = "ml")]
pub use image_ops::ml::backend::{MlError, TiledOnnxConfig};
#[cfg(feature = "ml")]
pub use image_ops::ml::denoise::ml_denoise;
#[cfg(feature = "ml")]
pub use image_ops::ml::star_removal::{
    StarRemovalResult, remove_stars, remove_stars_starless_only,
};
