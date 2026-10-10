//! Lumos - Astronomical image processing library.
//!
//! The pipeline stages, in the order a set of sub-exposures passes through them:
//!
//! - RAW and FITS decode into linear planar images ([`LinearImage`], [`CfaImage`]).
//! - Master dark, flat and bias, defect maps, and per-frame calibration ([`CalibrationMasters`]).
//! - Sub-pixel star detection that feeds registration ([`detection`]).
//! - Star-pattern alignment and the warp into a common frame ([`register`], [`warp`]).
//! - Statistical per-pixel combination, with rejection, normalization and weighting
//!   ([`StackConfig`]).
//! - Fruchter & Hook variable-pixel reconstruction of dithered sets ([`drizzle_stack`]).
//! - End-to-end runs ([`align_and_stack`], [`calibrate_align_stack`]).
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
//! use lumos::detection::{self, StarDetector};
//! use lumos::{LinearImage, LoadContext};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Load an astronomical image
//! let image = LinearImage::from_file("linear_light_001.fits", &LoadContext::default())?;
//!
//! // Detect stars
//! let config = detection::Config::default();
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! # Ok(())
//! # }
//! ```

mod background_mesh;
mod bit_buffer2;
mod buffer_pool;
mod calibration_masters;
mod combine;
mod concurrency;
mod drizzle;
mod error;
mod frame_store;
mod image_ops;
mod ingest;
mod io;
mod math;
mod memory;
mod mount_table;
mod pipeline;
mod progress;
mod registration;
mod run_report;
mod simd;
mod stack_product;
mod star_detection;

pub use calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
pub use calibration_masters::cosmic_ray::error::UnknownAdcStep;
pub use calibration_masters::defect_map::DefectMap;
pub use calibration_masters::error::{CalibrationError, DarkMismatch};
pub use error::{FrameDimensionMismatch, InvalidConfigField};
pub use io::image::PREVIEW_IMAGE_EXTENSIONS;
pub use io::image::calibration_state::CalibrationState;
pub use io::image::cfa::{CfaImage, CfaType};
pub use io::image::error::ImageError;
pub use io::image::fits::options::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsFloatScale, FitsHduSelector, FitsLoadOptions,
    FitsNullPolicy,
};
pub use io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance, FitsTransferProvenance,
};
pub use io::image::image_dimensions::ImageDimensions;
pub use io::image::image_metadata::ImageMetadata;
pub use io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
pub use io::image::linear::LinearImage;
pub use io::image::load_context::LoadContext;
pub use io::image::mosaic_noise::MosaicNoise;
pub use io::image::pixel_flags::{PixelFlags, QualityFlags};
pub use io::image::preview_image::{PreviewImage, PreviewPixels};
pub use io::image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
pub use io::image::unverified_conditions::UnverifiedConditions;
pub use io::raw::RAW_EXTENSIONS;
pub use io::raw::demosaic::bayer::CfaPattern;
pub use io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
pub use io::raw::demosaic::xtrans::xtrans_pattern::{XTransPattern, XTransPatternError};
pub use io::raw::raw_files::raw_files;
pub use math::size2us::Size2us;
pub use math::vec2us::Vec2us;

pub use calibration_masters::calibration_component::CalibrationComponent;
pub use calibration_masters::calibration_set::CalibrationSet;
pub use calibration_masters::master_role::MasterRole;
pub use calibration_masters::master_subtraction::Subtractor;
pub use calibration_masters::{
    CalibrationMasters, DEFAULT_SIGMA_THRESHOLD, DefectSummary, stack_cfa_master,
};

/// Star detection: the detector, its configuration, and the stars and diagnostics it returns, each
/// under its own name.
pub mod detection {
    pub use crate::star_detection::config::Config;
    pub use crate::star_detection::config::background_config::{
        BackgroundConfig, BackgroundRefinement,
    };
    pub use crate::star_detection::config::detection_config::{
        Connectivity, Deblend, DetectionConfig,
    };
    pub use crate::star_detection::config::filter_config::FilterConfig;
    pub use crate::star_detection::config::fwhm_config::{FwhmConfig, FwhmMode};
    pub use crate::star_detection::config::measurement_config::{
        CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
    };
    pub use crate::star_detection::detector::{
        DetectionResult, Diagnostics, FwhmSource, QualityFilterDiagnostics, StarDetector,
    };
    pub use crate::star_detection::roundness::Roundness;
    pub use crate::star_detection::star::Star;
}

pub use registration::distortion::sip::{SipConfig, SipPolynomial};
pub use registration::ransac::config::RansacConfig;
pub use registration::register;
pub use registration::registration_config::{
    InterpolationMethod, RegistrationConfig, RegistrationMatchingConfig, WarpParams,
};
pub use registration::resample::{WarpResult, warp};
pub use registration::result::{
    FailedModel, RansacFailureReason, RegistrationCatalog, RegistrationError, RegistrationResult,
    StarMatch,
};
pub use registration::transform::inverse_warp::{InverseMapped, InverseWarp};
pub use registration::transform::{Transform, TransformModel, TransformType, WarpTransform};
pub use registration::triangle::TriangleConfig;
pub use registration::triangle::voting::MatchIndices;

pub use combine::config::{Combine, CombineMethod, Normalization, SmallN, StackConfig, Weighting};
pub use combine::error::{StackConfigError, StackError};
pub use combine::rejection::Rejection;
pub use combine::rejection::gesd_config::GesdConfig;
pub use combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
pub use combine::rejection::rejection_scale::RejectionScale;
pub use combine::rejection::sigma_clip_config::SigmaClipConfig;
pub use combine::rejection::trim_config::TrimConfig;
pub use combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
pub use combine::stack::{StackFrame, stack, stack_images};
pub use frame_store::capture_conditions::CaptureCondition;
pub use frame_store::error::{ConditionMismatch, FrameStoreError};
pub use frame_store::frame_quality::FramePlane;
pub use ingest::ingest_config::IngestConfig;
pub use progress::progress_callback::ProgressCallback;
pub use progress::stacking_progress::{StackingProgress, StackingStage};
pub use run_report::{FlagCounts, RunReport};
pub use stack_product::StackProduct;
pub use stack_product::coverage::Coverage;
pub use stack_product::quality_map::QualityMap;
pub use stack_product::quality_planes::QualityPlanes;

pub use pipeline::align::align_and_stack;
pub use pipeline::calibrate::calibrate_align_stack;
pub use pipeline::config::{AlignStackConfig, Reference};
pub use pipeline::error::AlignStackError;
pub use pipeline::frame_registration::FrameRegistration;
pub use pipeline::result::{AlignStackResult, AlignmentSummary};

pub use drizzle::accumulator::{DrizzleAccumulator, DrizzleFrame};
pub use drizzle::config::{DrizzleConfig, DrizzleKernel};
pub use drizzle::drizzle_result::DrizzleResult;
pub use drizzle::error::{DrizzleConfigError, DrizzleError};
pub use drizzle::stack::{drizzle_images, drizzle_stack};

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

#[cfg(test)]
mod internals;
