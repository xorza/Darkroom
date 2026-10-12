//! Lumos - Astronomical image processing library.
//!
//! The pipeline stages, in the order a set of sub-exposures passes through them:
//!
//! - RAW and FITS decode into linear planar images ([`LinearImage`], [`CfaImage`]).
//! - Master dark, flat and bias, defect maps, and per-frame calibration ([`CalibrationMasters`]).
//! - Sub-pixel star detection that feeds registration ([`StarDetector`]).
//! - Star-pattern alignment and the warp into a common frame ([`RegistrationConfig::register`],
//!   [`WarpTransform::warp`]).
//! - Statistical per-pixel combination, with rejection, normalization and weighting
//!   ([`StackConfig`]).
//! - Fruchter & Hook variable-pixel reconstruction of dithered sets ([`DrizzleConfig`]).
//! - End-to-end runs ([`AlignStackConfig`]).
//! - Non-linear operations on the stacked master, strictly after the linear stages ([`Stretch`],
//!   [`Denoise`] and the others).
//!
//! What the stages share: RAM and memory-mapped frame storage, the plan that keeps a run inside
//! its memory, the combined image and the per-pixel planes beside it ([`StackProduct`]), and
//! progress ([`ProgressCallback`]).
//!
//! # Quick Start
//!
//! ```no_run
//! use lumos::{LinearImage, LoadContext, StarDetectionConfig, StarDetector};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Load an astronomical image
//! let image = LinearImage::from_file("linear_light_001.fits", &LoadContext::default())?;
//!
//! // Detect stars
//! let config = StarDetectionConfig::default();
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! # Ok(())
//! # }
//! ```

mod background_mesh;
mod bit_buffer2;
mod calibration;
mod combine;
mod concurrency;
mod drizzle;
mod error;
mod frame;
mod frame_store;
mod image;
mod image_ops;
mod ingest;
mod io;
mod math;
mod memory;
mod pipeline;
mod platform;
mod progress;
mod registration;
mod scratch_pool;

mod simd;
mod stack_product;
mod star_detection;

pub use calibration::cosmic_ray::cosmic_ray_config::{CosmicRayConfig, NoiseEstimation};
pub use calibration::cosmic_ray::error::CosmicRayError;
pub use calibration::error::{
    BundleError, CalibrationError, DarkMismatch, DefectTableError, MasterBuildError,
    SubtractionError,
};
pub use calibration::temperature_difference::TemperatureDifference;
pub use calibration::temperature_source::TemperatureSource;
pub use error::{FrameDimensionMismatch, InvalidConfigField};
pub use frame::unverified_conditions::UnverifiedConditions;
pub use image::calibration_state::CalibrationState;
pub use image::cfa::cfa_colour::CfaColour;
pub use image::cfa::cfa_image::CfaImage;
pub use image::cfa::cfa_pattern::CfaPattern;
pub use image::cfa::cfa_type::CfaType;
pub use image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
pub use image::cfa::xtrans_pattern::XTransPattern;
pub use image::error::XTransPatternError;
pub use image::flat_gain::FlatGain;
pub use image::image_dimensions::ImageDimensions;
pub use image::image_metadata::ImageMetadata;
pub use image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
pub use image::linear_image::LinearImage;
pub use image::pixel_flags::PixelFlags;
pub use image::preview_image::{PreviewImage, PreviewPixels};
pub use image::quality_flags::QualityFlags;
pub use image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
pub use io::PREVIEW_IMAGE_EXTENSIONS;
pub use io::error::ImageError;
pub use io::fits::options::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsFloatScale, FitsHduSelector, FitsLoadOptions,
    FitsNullPolicy,
};
pub use io::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance, FitsTransferProvenance,
};
pub use io::load_context::LoadContext;
pub use io::raw::RAW_EXTENSIONS;
pub use io::raw::error::{BlackLevelError, LibrawCode, RawError, RawScanError};
pub use io::raw::raw_files::raw_files;
pub use math::noise::mosaic_noise::MosaicNoise;
pub use math::size2us::Size2us;
pub use math::vec2us::Vec2us;

pub use calibration::calibration_component::CalibrationComponent;
pub use calibration::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
pub use calibration::calibration_outcome::CalibrationOutcome;
pub use calibration::calibration_set::CalibrationSet;
pub use calibration::dark_scale::{DarkScale, DarkScaleSource};
pub use calibration::defect_map::defect_summary::{ColourDefects, DefectSummary};
pub use calibration::master_cache::{
    CachedMaster, MasterCache, MasterInputs, MasterRecipe, NoMasterCache, SubtractedFrames,
};
pub use calibration::master_role::MasterRole;

pub use star_detection::roundness::Roundness;
pub use star_detection::star::Star;
pub use star_detection::star_detection_config::StarDetectionConfig;
pub use star_detection::star_detection_config::background_config::{
    BackgroundConfig, BackgroundRefinement,
};
pub use star_detection::star_detection_config::extraction_config::{
    Connectivity, Deblend, ExtractionConfig,
};
pub use star_detection::star_detection_config::filter_config::FilterConfig;
pub use star_detection::star_detection_config::fwhm_config::{FwhmConfig, FwhmMode};
pub use star_detection::star_detection_config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};
pub use star_detection::star_detector::StarDetector;
pub use star_detection::star_detector::detection_result::{
    DetectionDiagnostics, DetectionResult, FwhmSource, QualityFilterDiagnostics,
};

pub use registration::distortion::sip::{SipConfig, SipPolynomial};
pub use registration::error::{
    FailedModel, RansacFailureReason, RegistrationCatalog, RegistrationError,
};
pub use registration::ransac::ransac_config::RansacConfig;
pub use registration::registration_config::{RegistrationConfig, RegistrationMatchingConfig};
pub use registration::registration_result::{RegistrationResult, StarMatch};
pub use registration::resample::WarpResult;
pub use registration::resample::warp_params::{InterpolationMethod, WarpParams};
pub use registration::transform::inverse_warp::{InverseMapped, InverseWarp};
pub use registration::transform::warp_transform::WarpTransform;
pub use registration::transform::{Transform, TransformModel, TransformType};
pub use registration::triangle::triangle_config::TriangleConfig;
pub use registration::triangle::voting::MatchIndices;

pub use combine::error::{StackConfigError, StackError};
pub use combine::rejection::Rejection;
pub use combine::rejection::gesd_config::GesdConfig;
pub use combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
pub use combine::rejection::rejection_scale::RejectionScale;
pub use combine::rejection::sigma_bounds::SigmaBounds;
pub use combine::rejection::sigma_clip_config::SigmaClipConfig;
pub use combine::rejection::trim_config::TrimConfig;
pub use combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
pub use combine::stack::StackFrame;
pub use combine::stack_config::{
    Combine, CombineMethod, Normalization, SmallN, StackConfig, Weighting,
};
pub use frame::capture_conditions::CaptureCondition;
pub use frame::frame_quality::FramePlane;
pub use frame_store::error::{ConditionMismatch, FrameStoreError};
pub use image::flag_counts::FlagCounts;
pub use ingest::error::AdmissionError;
pub use ingest::ingest_config::IngestConfig;
pub use progress::progress_callback::ProgressCallback;
pub use progress::stacking_progress::{StackingProgress, StackingStage};
pub use stack_product::StackProduct;
pub use stack_product::coverage::Coverage;
pub use stack_product::quality_map::QualityMap;
pub use stack_product::quality_planes::QualityPlanes;
pub use stack_product::run_report::RunReport;

pub use pipeline::align_stack_config::{AlignStackConfig, Reference};
pub use pipeline::align_stack_result::{AlignStackResult, AlignmentSummary};
pub use pipeline::error::AlignStackError;
pub use pipeline::frame_registration::FrameRegistration;

pub use drizzle::drizzle_config::{DrizzleConfig, DrizzleKernel};
pub use drizzle::drizzle_frame::DrizzleFrame;
pub use drizzle::drizzle_result::DrizzleResult;
pub use drizzle::error::{DrizzleConfigError, DrizzleError};

pub use image_ops::stretch::{ColorMode, Stretch, StretchMethod};

pub use image_ops::color_calibration::neutralize_background::NeutralizeBackground;
pub use image_ops::color_calibration::scnr::Scnr;

pub use image_ops::extract_background::{BackgroundMode, ExtractBackground};

pub use image_ops::denoise::{Denoise, Threshold};

pub use image_ops::local_contrast::LocalContrast;

pub use image_ops::hdr::Hdr;

pub use image_ops::error::OpError;

#[cfg(feature = "ml")]
pub use image_ops::error::MlError;
#[cfg(feature = "ml")]
pub use image_ops::ml::backend::TiledOnnxConfig;
#[cfg(feature = "ml")]
pub use image_ops::ml::ml_denoise::MlDenoise;
#[cfg(feature = "ml")]
pub use image_ops::ml::remove_stars::{RemoveStars, StarRemovalResult};

#[cfg(feature = "bench")]
pub mod bench;

#[cfg(any(test, feature = "internals"))]
mod internals;
