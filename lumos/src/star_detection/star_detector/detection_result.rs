//! [`DetectionResult`]: the stars a detection found, and what each of its stages decided.

use serde::{Deserialize, Serialize};

use crate::star_detection::star::Star;

/// Result of star detection with diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: DetectionDiagnostics,
}

/// Rejection counts produced by the quality-filtering stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityFilterDiagnostics {
    /// Number of stars rejected as saturated.
    pub saturated: usize,
    /// Number of stars rejected for low SNR.
    pub low_snr: usize,
    /// Number of stars rejected for high eccentricity.
    pub high_eccentricity: usize,
    /// Number of stars rejected as cosmic rays.
    pub cosmic_rays: usize,
    /// Number of stars rejected for non-circular shape.
    pub roundness: usize,
    /// Number of stars rejected for abnormal FWHM.
    pub fwhm_outliers: usize,
    /// Number of duplicate detections removed.
    pub duplicates: usize,
}

/// Diagnostic information from star detection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectionDiagnostics {
    /// Number of pixels above detection threshold.
    pub pixels_above_threshold: usize,
    /// Number of connected components found.
    pub connected_components: usize,
    /// Number of candidates after size/edge filtering.
    pub candidates_after_filtering: usize,
    /// Number of candidates that were deblended into multiple stars.
    pub deblended_components: usize,
    /// Number of stars after centroid computation (before quality filtering).
    pub stars_after_centroid: usize,
    /// Rejections produced by the quality-filtering stage.
    pub quality_filter: QualityFilterDiagnostics,
    /// Median FWHM of the detected stars in pixels, `None` when none was detected.
    pub median_fwhm: Option<f32>,
    /// Median SNR of the detected stars, `None` when none was detected.
    pub median_snr: Option<f32>,
    /// Where the matched filter's FWHM came from.
    pub fwhm: FwhmSource,
}

/// Where the FWHM the detector ran with came from — the three states the matched-filter stage can
/// end in, as one value rather than an `f32` plus a count plus a flag derived from the count, which
/// admits combinations none of the three states describes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum FwhmSource {
    /// Matched filtering was off: auto-estimation disabled and no configured FWHM.
    #[default]
    Disabled,
    /// Taken from configuration, or from the built-in fallback when too few stars passed to
    /// estimate one. Nothing was measured from this frame.
    Configured(f32),
    /// Measured from this frame's own stars.
    Estimated {
        fwhm: f32,
        /// Stars that contributed to the estimate; always non-zero.
        stars_used: usize,
    },
}

impl FwhmSource {
    /// The FWHM the detector ran with, or `None` when matched filtering was off.
    pub const fn value(&self) -> Option<f32> {
        match self {
            FwhmSource::Disabled => None,
            FwhmSource::Configured(fwhm) | FwhmSource::Estimated { fwhm, .. } => Some(*fwhm),
        }
    }
}
