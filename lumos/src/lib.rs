//! Lumos - Astronomical image processing library.
//!
//! The pipeline stages, in the order a set of sub-exposures passes through them:
//!
//! - RAW and FITS decode into linear planar images ([`LinearImage`], [`CfaImage`]).
//! - Master dark, flat and bias, defect maps, and per-frame calibration ([`CalibrationMasters`]).
//! - Sub-pixel star detection that feeds registration ([`StarDetector`]).
//! - Star-pattern alignment and the warp into a common frame ([`Registrar::register`],
//!   [`WarpConfig::warp`]).
//! - Statistical per-pixel combination, with rejection, normalization and weighting
//!   ([`StackConfig`]).
//! - Fruchter & Hook variable-pixel reconstruction of dithered sets ([`DrizzleConfig`]).
//! - End-to-end runs ([`AlignStackConfig`]).
//! - Non-linear operations on the stacked master, strictly after the linear stages ([`Stretch`],
//!   [`Denoise`] and the others).
//!
//! What the stages share: RAM and memory-mapped frame storage, the plan that keeps a run inside
//! its memory, the combined image and the per-pixel planes beside it ([`StackProduct`]), and
//! the progress and cancel a caller follows and stops a run by ([`ProgressMonitor`]).
//!
//! # Quick Start
//!
//! ```no_run
//! use lumos::{LoadCtx, StarDetectionConfig, StarDetector};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Load an astronomical image
//! let image = LoadCtx::default().linear("linear_light_001.fits")?;
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

mod bg_mesh;
mod bit_buffer2;
mod calibration;
mod combine;
mod concurrency;
mod config_field;
mod decode;
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
mod noise;
mod pipeline;
mod progress;
mod recycled;
mod registration;
mod scratch_pool;
mod simd;
mod stack_product;
mod star_detection;

pub use calibration::cosmic_ray::cosmic_ray_config::{CosmicRayConfig, NoiseEstimation};
pub use calibration::cosmic_ray::error::CosmicRayError;
pub use calibration::error::{
    CalibrationError, DarkMismatch, MasterBuildError, MasterStackError, SubtractionError,
};
pub use error::InvalidConfigField;
pub use image::bg_split::BgSplit;
pub use image::calibration_state::CalibrationState;
pub use image::cfa::cfa_color::CfaColor;
pub use image::cfa::cfa_image::CfaImage;
pub use image::cfa::cfa_pattern::CfaPattern;
pub use image::cfa::cfa_type::CfaType;
pub use image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
pub use image::cfa::mosaic::Mosaic;
pub use image::cfa::xtrans_pattern::XTransPattern;
pub use image::channel_layout::ChannelLayout;
pub use image::dark_comparison::DarkComparison;
pub use image::derived_facts::DerivedFacts;
pub use image::error::{DimensionsError, FrameDimensionMismatch, XTransPatternError};
pub use image::fits_checksum_provenance::{FitsChecksumProvenance, FitsChecksumState};
pub use image::fits_transfer_provenance::{FitsHduProvenance, FitsTransferProvenance};
pub use image::flat_gain::FlatGain;
pub use image::flat_gain::frame_gain::FrameGain;
pub use image::image_dimensions::ImageDimensions;
pub use image::image_metadata::ImageMetadata;
pub use image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RasterFormat,
    RasterTransfer, RowOrder, SourceProvenance,
};
pub use image::linear_image::LinearImage;
pub use image::mosaic_noise::MosaicNoise;
pub use image::observation_header::ObservationHeader;
pub use image::pixel_flags::PixelFlags;
pub use image::preview_image::{PreviewImage, PreviewPixels};
pub use image::quality_flags::QualityFlags;
pub use image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
pub use image::temperature_difference::TemperatureDifference;
pub use image::temperature_source::TemperatureSource;
pub use image::unverified_conditions::UnverifiedConditions;
pub use io::error::{ImageError, ScientificRefusal, WriteError};
pub use io::fits::calibration_bundle::error::{BundleError, DefectTableError};
pub use io::fits::error::{FitsAllocation, FitsRefusal};
pub use io::fits::fits_load_config::{
    FitsChecksumPolicy, FitsCubeInterpretation, FitsFloatScale, FitsHduSelector, FitsLoadConfig,
};
pub use io::fits::lumos_extension::LumosExtension;
pub use io::input_format::{PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS};
pub use io::load_ctx::LoadCtx;
pub use io::null_policy::NullPolicy;
pub use io::raw::error::{BlackLevelError, LibrawCode, RawError, RawScanError};
pub use io::raw::raw_files::raw_files;
pub use io::writer;
pub use math::size2us::Size2us;
pub use math::vec2us::Vec2us;

pub use calibration::calibration_component::CalibrationComponent;
pub use calibration::calibration_masters::{CalibrationMasters, MasterBuild};
pub use calibration::calibration_outcome::CalibrationOutcome;
pub use calibration::calibration_set::CalibrationSet;
pub use calibration::dark_scale::{DarkScale, DarkScaleSource};
pub use calibration::defect_map::defect_summary::{ColorDefects, ColorShares, DefectSummary};
pub use calibration::defect_map::sigma_threshold::SigmaThreshold;
pub use calibration::master_cache::{
    CachedMaster, MasterCache, MasterInputs, NoMasterCache, SubtractedFrames,
};
pub use calibration::master_recipe::MasterRecipe;
pub use calibration::master_role::MasterRole;

pub use star_detection::roundness::Roundness;
pub use star_detection::star::Star;
pub use star_detection::star_detection_config::StarDetectionConfig;
pub use star_detection::star_detection_config::bg_config::{
    BgConfig, BgRefinement, IterativeRefinement,
};
pub use star_detection::star_detection_config::extraction_config::{
    Connectivity, Deblend, ExtractionConfig,
};
pub use star_detection::star_detection_config::filter_config::FilterConfig;
pub use star_detection::star_detection_config::fwhm_config::{FwhmConfig, FwhmMode};
pub use star_detection::star_detection_config::measurement_config::{
    CentroidMethod, LocalBgMethod, MeasurementConfig,
};
pub use star_detection::star_detector::StarDetector;
pub use star_detection::star_detector::detection_result::{
    DetectionDiagnostics, DetectionResult, FwhmSource, QualityFilterDiagnostics,
};

pub use registration::distortion::sip::sip_order::SipOrder;
pub use registration::distortion::sip::{SipConfig, SipPolynomial};
pub use registration::error::{
    FailedModel, RansacFailure, RegistrationCatalog, RegistrationError, StarField,
};
pub use registration::match_idxs::MatchIdxs;
pub use registration::ransac::ransac_config::{RansacConfig, ScaleRange};
pub use registration::registrar::Registrar;
pub use registration::registration_config::{RegistrationConfig, RegistrationMatchingConfig};
pub use registration::registration_result::{RegistrationResult, StarMatch};
pub use registration::resample::WarpResult;
pub use registration::resample::warp_config::{ClampingThreshold, InterpolationMethod, WarpConfig};
pub use registration::transform::linear_model::LinearModel;
pub use registration::transform::local_geometry::LocalGeometry;
pub use registration::transform::{Transform, TransformType};
pub use registration::transform_model::TransformModel;
pub use registration::triangle::triangle_config::TriangleConfig;
pub use registration::warp_transform::WarpTransform;
pub use registration::warp_transform::inverse_warp::{InverseMapped, InverseWarp};

pub use combine::error::{StackConfigError, StackError};
pub use combine::normalization::Normalization;
pub use combine::rejection::Rejection;
pub use combine::rejection::gesd_config::GesdConfig;
pub use combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
pub use combine::rejection::rejection_scale::RejectionScale;
pub use combine::rejection::sigma_bounds::SigmaBounds;
pub use combine::rejection::sigma_clip_config::SigmaClipConfig;
pub use combine::rejection::trim_config::TrimConfig;
pub use combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
pub use combine::stack_config::StackConfig;
pub use combine::stack_config::combine::Combine;
pub use combine::stack_config::combine_method::CombineMethod;
pub use combine::stack_config::small_n::SmallN;
pub use combine::stack_config::weighting::Weighting;
pub use combine::stack_frame::StackFrame;
pub use frame::capture_condition::CaptureCondition;
pub use frame::error::ConditionMismatch;
pub use frame::frame_plane::FramePlane;
pub use frame_store::error::FrameStoreError;
pub use image::flag_counts::FlagCounts;
pub use image::quality_map::QualityMap;
pub use ingest::error::AdmissionError;
pub use ingest::ingest_config::IngestConfig;
pub use progress::progress_callback::ProgressCallback;
pub use progress::progress_monitor::ProgressMonitor;
pub use progress::stacking_progress::{StackingProgress, StackingStage};
pub use stack_product::StackProduct;
pub use stack_product::coverage::Coverage;
pub use stack_product::quality_planes::QualityPlanes;
pub use stack_product::run_report::RunReport;
pub use stack_product::weight_kind::WeightKind;

pub use pipeline::align_stack_config::{AlignStackConfig, CalibratedLights, Reference};
pub use pipeline::align_stack_result::{AlignStackResult, LightOutcome};
pub use pipeline::error::{AlignStackConfigError, AlignStackError};
pub use pipeline::frame_registration::FrameRegistration;

pub use drizzle::drizzle_config::DrizzleConfig;
pub use drizzle::drizzle_frame::DrizzleFrame;
pub use drizzle::drizzle_kernel::DrizzleKernel;
pub use drizzle::error::{DrizzleConfigError, DrizzleError};

pub use image_ops::stretch::{ColorMode, Stretch, StretchMethod};

pub use image_ops::color_calibration::neutralize_bg::NeutralizeBg;
pub use image_ops::color_calibration::scnr::{Scnr, ScnrProtection};

pub use image_ops::extract_bg::{BgMode, ExtractBg};

pub use image_ops::denoise::{Denoise, Threshold};

pub use image_ops::local_contrast::LocalContrast;

pub use image_ops::hdr::Hdr;

pub use image_ops::error::{AutoStretch, OpError};

#[cfg(feature = "ml")]
pub use image_ops::error::MlError;
#[cfg(feature = "ml")]
pub use image_ops::ml::backend::OnnxModel;
#[cfg(feature = "ml")]
pub use image_ops::ml::ml_denoise::MlDenoise;
#[cfg(feature = "ml")]
pub use image_ops::ml::remove_stars::{RemoveStars, StarRemovalResult};

#[cfg(feature = "internals")]
pub mod bench;

#[cfg(feature = "internals")]
mod internals;
