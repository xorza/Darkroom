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
//! use lumos::{LinearImage, LoadContext, StarDetectionConfig, StarDetector};
//!
//! // Load an astronomical image
//! let image = LinearImage::from_file("linear_light_001.fits", &LoadContext::default())?;
//!
//! // Detect stars
//! let config = StarDetectionConfig::default();
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! ```

pub(crate) mod background_mesh;
pub(crate) mod bit_buffer2;
pub(crate) mod buffer_pool;
pub(crate) mod concurrency;
pub(crate) mod error;
pub(crate) mod image_ops;
pub(crate) mod io;
pub(crate) mod math;
pub(crate) mod memory;
pub(crate) mod simd;
pub(crate) mod stacking;

#[cfg(test)]
pub(crate) mod testing;

pub use error::InvalidConfigField;
pub use io::image::PREVIEW_IMAGE_EXTENSIONS;
pub use io::image::cfa::{CfaImage, CfaType};
pub use io::image::error::ImageError;
pub use io::image::fits::options::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsHduSelector, FitsLoadOptions,
};
pub use io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance, FitsTransferProvenance,
};
pub use io::image::image_dimensions::ImageDimensions;
pub use io::image::image_metadata::{BitPix, ImageMetadata};
pub use io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, SourceContainer,
    TransferProvenance,
};
pub use io::image::linear::LinearImage;
pub use io::image::load_context::LoadContext;
pub use io::image::preview_image::PreviewImage;
pub use io::raw::RAW_EXTENSIONS;
pub use io::raw::demosaic::bayer::CfaPattern;
pub use math::size2us::Size2us;
pub use math::vec2us::Vec2us;
pub use stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, NoiseEstimation};
pub use stacking::calibration_masters::defect_map::DefectMap;

pub use stacking::calibration_masters::{
    CalibrationComponent, CalibrationError, CalibrationMasters, CalibrationSet,
    DEFAULT_SIGMA_THRESHOLD, DefectSummary, MasterRole, stack_cfa_master,
};

pub use stacking::star_detection::config::Config as StarDetectionConfig;
pub use stacking::star_detection::config::background_config::{
    BackgroundConfig as StarDetectionBackgroundConfig, BackgroundRefinement,
};
pub use stacking::star_detection::config::detection_config::{
    Connectivity, DetectionConfig as StarDetectionCandidateConfig,
};
pub use stacking::star_detection::config::filter_config::FilterConfig as StarDetectionFilterConfig;
pub use stacking::star_detection::config::fwhm_config::FwhmConfig as StarDetectionFwhmConfig;
pub use stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig as StarDetectionMeasurementConfig,
    NoiseModel,
};
pub use stacking::star_detection::detector::{
    DetectionResult as StarDetectionResult, Diagnostics as StarDetectionDiagnostics, FwhmSource,
    QualityFilterDiagnostics as StarDetectionQualityFilterDiagnostics, StarDetector,
};
pub use stacking::star_detection::roundness::Roundness;
pub use stacking::star_detection::star::Star;

pub use stacking::registration::config::{
    Config as RegistrationConfig, InterpolationMethod, RegistrationMatchingConfig, WarpParams,
};
pub use stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
pub use stacking::registration::ransac::RansacConfig;
pub use stacking::registration::register;
pub use stacking::registration::resample::{WarpResult, warp};
pub use stacking::registration::result::{
    RansacFailureReason, RegistrationCatalog, RegistrationError, RegistrationResult, StarMatch,
};
pub use stacking::registration::transform::{
    Transform, TransformModel, TransformType, WarpTransform,
};
pub use stacking::registration::triangle::TriangleConfig;

pub use stacking::combine::cache_config::CacheConfig;
pub use stacking::combine::config::{CombineMethod, Normalization, SmallN, StackConfig, Weighting};
pub use stacking::combine::error::{Error as StackError, StackConfigError};
pub use stacking::combine::rejection::Rejection;
pub use stacking::combine::rejection::gesd_config::GesdConfig;
pub use stacking::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
pub use stacking::combine::rejection::percentile_clip_config::PercentileClipConfig;
pub use stacking::combine::rejection::sigma_clip_config::SigmaClipConfig;
pub use stacking::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
pub use stacking::combine::stack::{StackFrame, stack, stack_images};
pub use stacking::frame_store::FramePlane;
pub use stacking::frame_store::error::FrameStoreError;
pub use stacking::progress::{ProgressCallback, StackingProgress, StackingStage};
pub use stacking::stack_product::StackProduct;
pub use stacking::stack_product::coverage::Coverage;
pub use stacking::stack_product::quality_map::QualityMap;
pub use stacking::stack_product::quality_planes::QualityPlanes;

pub use stacking::pipeline::align::align_and_stack;
pub use stacking::pipeline::calibrate::calibrate_align_stack;
pub use stacking::pipeline::config::{AlignStackConfig, Reference};
pub use stacking::pipeline::result::{
    AlignStackResult, AlignmentSummary, Error as AlignStackError,
};

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

pub use image_ops::error::OpError;

#[cfg(feature = "ml")]
pub use image_ops::ml::backend::{MlError, TiledOnnxConfig};
#[cfg(feature = "ml")]
pub use image_ops::ml::denoise::MlDenoise;
#[cfg(feature = "ml")]
pub use image_ops::ml::star_removal::{RemoveStars, StarRemovalResult};
