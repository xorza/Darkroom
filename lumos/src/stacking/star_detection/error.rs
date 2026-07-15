//! Error types for star detection.

use thiserror::Error;

/// Invalid [`crate::StarDetectionConfig`] parameters.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StarDetectionConfigError {
    #[error("tile_size must be between 16 and 256, got {value}")]
    InvalidTileSize { value: usize },

    #[error("sigma_clip_iterations must be at most 10, got {value}")]
    ExcessiveSigmaClipIterations { value: usize },

    #[error("iterative background refinement requires at least one iteration")]
    ZeroBackgroundRefinementIterations,

    #[error("background refinement iterations must be at most 10, got {value}")]
    ExcessiveBackgroundRefinementIterations { value: usize },

    #[error("bg_mask_dilation must be at most 50, got {value}")]
    ExcessiveBackgroundMaskDilation { value: usize },

    #[error("sigma_threshold must be finite and positive, got {value}")]
    InvalidSigmaThreshold { value: f32 },

    #[error("expected_fwhm must be finite and non-negative, got {value}")]
    InvalidExpectedFwhm { value: f32 },

    #[error("psf_axis_ratio must be finite and in (0, 1], got {value}")]
    InvalidPsfAxisRatio { value: f32 },

    #[error("psf_angle must be finite, got {value}")]
    InvalidPsfAngle { value: f32 },

    #[error("min_stars_for_fwhm must be at least 5, got {value}")]
    TooFewStarsForFwhm { value: usize },

    #[error("fwhm_estimation_sigma_factor must be finite and at least 1, got {value}")]
    InvalidFwhmEstimationSigmaFactor { value: f32 },

    #[error("deblend_min_separation must be at least 1, got {value}")]
    InvalidDeblendMinSeparation { value: usize },

    #[error("deblend_min_prominence must be finite and in [0, 1], got {value}")]
    InvalidDeblendMinProminence { value: f32 },

    #[error("deblend_n_thresholds must be 0 or between 2 and {maximum}, got {value}")]
    InvalidDeblendThresholdCount { value: usize, maximum: usize },

    #[error("deblend_min_contrast must be finite and in [0, 1], got {value}")]
    InvalidDeblendMinContrast { value: f32 },

    #[error("min_area must be at least 1")]
    ZeroMinArea,

    #[error("max_area {max_area} must be at least min_area {min_area}")]
    MaxAreaBelowMin { min_area: usize, max_area: usize },

    #[error("Moffat beta must be finite and in (0, 10], got {value}")]
    InvalidMoffatBeta { value: f32 },

    #[error("min_snr must be finite and positive, got {value}")]
    InvalidMinSnr { value: f32 },

    #[error("max_eccentricity must be finite and in [0, 1], got {value}")]
    InvalidMaxEccentricity { value: f32 },

    #[error("max_sharpness must be finite and in (0, 1], got {value}")]
    InvalidMaxSharpness { value: f32 },

    #[error("max_roundness must be finite and in (0, 1], got {value}")]
    InvalidMaxRoundness { value: f32 },

    #[error("max_fwhm_deviation must be finite and non-negative, got {value}")]
    InvalidMaxFwhmDeviation { value: f32 },

    #[error("duplicate_min_separation must be finite and non-negative, got {value}")]
    InvalidDuplicateMinSeparation { value: f32 },

    #[error("noise-model gain must be finite and positive, got {value}")]
    InvalidGain { value: f32 },

    #[error("noise-model read_noise must be finite and non-negative, got {value}")]
    InvalidReadNoise { value: f32 },
}
