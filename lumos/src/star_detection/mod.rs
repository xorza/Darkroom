//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Algorithm Overview
//!
//! 1. **Background estimation**: Divide image into tiles, compute sigma-clipped
//!    median per tile, then bilinearly interpolate to create a smooth background map.
//!
//! 2. **Star detection**: Threshold pixels above background + k×σ, then use
//!    connected component labeling to group pixels into candidate stars.
//!
//! 3. **Filtering**: Reject candidates that are too small, too large, elongated,
//!    near edges, or saturated.
//!
//! 4. **Sub-pixel centroid**: Compute precise centroid using iterative weighted
//!    centroid algorithm (achieves ~0.05 pixel accuracy).
//!
//! 5. **Quality metrics**: Compute FWHM, SNR, and eccentricity for each star.

mod background;
mod centroid;
mod convolution;
mod cosmic_ray;
mod deblend;
mod detection;
mod gaussian_fit;
mod median_filter;
mod moffat_fit;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod visual_tests;

#[cfg(feature = "bench")]
pub mod bench {
    #[allow(unused_imports)]
    pub use super::background::bench as background;
    #[allow(unused_imports)]
    pub use super::median_filter::bench as median_filter;
}

// Public API exports - used by external consumers of the library
#[allow(unused_imports)]
pub use background::{
    IterativeBackgroundConfig, estimate_background, estimate_background_iterative,
};
pub use centroid::compute_centroid;
pub use convolution::matched_filter;
pub use detection::{detect_stars, detect_stars_filtered};
#[allow(unused_imports)]
pub use gaussian_fit::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use median_filter::median_filter_3x3;
#[allow(unused_imports)]
pub use moffat_fit::{
    MoffatFitConfig, MoffatFitResult, alpha_beta_to_fwhm, fit_moffat_2d, fwhm_beta_to_alpha,
};

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy)]
pub struct Star {
    /// X coordinate (sub-pixel accurate).
    pub x: f32,
    /// Y coordinate (sub-pixel accurate).
    pub y: f32,
    /// Total flux (sum of background-subtracted pixel values).
    pub flux: f32,
    /// Full Width at Half Maximum in pixels.
    pub fwhm: f32,
    /// Eccentricity (0 = circular, 1 = elongated). Used to reject non-stellar objects.
    pub eccentricity: f32,
    /// Signal-to-noise ratio.
    pub snr: f32,
    /// Peak pixel value (for saturation detection).
    pub peak: f32,
    /// Sharpness metric (peak / flux_in_core). Cosmic rays have high sharpness (>0.8),
    /// real stars have lower sharpness (typically 0.2-0.6 depending on seeing).
    pub sharpness: f32,
    /// Roundness based on marginal Gaussian fits (DAOFIND GROUND).
    /// (Hx - Hy) / (Hx + Hy) where Hx, Hy are heights of marginal x and y fits.
    /// Circular sources → 0, x-extended → negative, y-extended → positive.
    pub roundness1: f32,
    /// Roundness based on symmetry (DAOFIND SROUND).
    /// Measures bilateral vs four-fold symmetry. Circular → 0, asymmetric → non-zero.
    pub roundness2: f32,
    /// L.A.Cosmic Laplacian SNR metric for cosmic ray detection.
    /// Cosmic rays have very sharp edges (high Laplacian), stars have smooth edges.
    /// Values > 50 typically indicate cosmic rays. Based on van Dokkum 2001.
    pub laplacian_snr: f32,
}

impl Star {
    /// Check if star is likely saturated.
    ///
    /// Stars with peak values near the maximum (>0.95 for normalized data)
    /// have unreliable centroids.
    pub fn is_saturated(&self) -> bool {
        self.peak > 0.95
    }

    /// Check if star is likely a cosmic ray (very sharp, single-pixel spike).
    ///
    /// Cosmic rays typically have sharpness > 0.7, while real stars are 0.2-0.5.
    pub fn is_cosmic_ray(&self, max_sharpness: f32) -> bool {
        self.sharpness > max_sharpness
    }

    /// Check if star is likely a cosmic ray using L.A.Cosmic Laplacian metric.
    ///
    /// Based on van Dokkum 2001: cosmic rays have very sharp edges that produce
    /// high Laplacian values. Stars, being smoothed by the PSF, have lower values.
    /// Threshold of ~50 is typical for rejecting cosmic rays.
    pub fn is_cosmic_ray_laplacian(&self, max_laplacian_snr: f32) -> bool {
        self.laplacian_snr > max_laplacian_snr
    }

    /// Check if star passes roundness filters.
    ///
    /// Both roundness metrics should be close to zero for circular sources.
    pub fn is_round(&self, max_roundness: f32) -> bool {
        self.roundness1.abs() <= max_roundness && self.roundness2.abs() <= max_roundness
    }

    /// Check if star is usable for registration.
    ///
    /// Filters out saturated, elongated, low-SNR stars, cosmic rays, and non-round objects.
    pub fn is_usable(
        &self,
        min_snr: f32,
        max_eccentricity: f32,
        max_sharpness: f32,
        max_roundness: f32,
    ) -> bool {
        !self.is_saturated()
            && self.snr >= min_snr
            && self.eccentricity <= max_eccentricity
            && !self.is_cosmic_ray(max_sharpness)
            && self.is_round(max_roundness)
    }
}

/// Configuration for star detection.
#[derive(Debug, Clone)]
pub struct StarDetectionConfig {
    /// Detection threshold in sigma above background (typically 3.0-5.0).
    pub detection_sigma: f32,
    /// Minimum star area in pixels.
    pub min_area: usize,
    /// Maximum star area in pixels.
    pub max_area: usize,
    /// Maximum eccentricity (0-1, higher = more elongated allowed).
    pub max_eccentricity: f32,
    /// Edge margin in pixels (stars too close to edge are rejected).
    pub edge_margin: usize,
    /// Minimum SNR for a star to be considered valid.
    pub min_snr: f32,
    /// Tile size for background estimation.
    pub background_tile_size: usize,
    /// Maximum FWHM deviation from median in MAD (median absolute deviation) units.
    /// Stars with FWHM > median + max_fwhm_deviation * MAD are rejected as spurious.
    /// Typical value is 3.0-5.0 (similar to sigma clipping). Set to 0.0 to disable.
    pub max_fwhm_deviation: f32,
    /// Expected FWHM of stars in pixels for matched filtering.
    /// The matched filter (Gaussian convolution) dramatically improves detection of
    /// faint stars by boosting SNR. Set to 0.0 to disable matched filtering.
    /// Typical values are 2.0-6.0 pixels depending on seeing and sampling.
    pub expected_fwhm: f32,
    /// Maximum sharpness for a star to be considered valid.
    /// Sharpness = peak_value / flux_in_3x3_core. Cosmic rays have very high sharpness
    /// (>0.7) because most flux is in a single pixel. Real stars spread flux across
    /// multiple pixels due to PSF, giving sharpness 0.2-0.5. Set to 1.0 to disable.
    pub max_sharpness: f32,
    /// Minimum separation between peaks for deblending star pairs (in pixels).
    /// Peaks closer than this are merged. Set to 0 to disable deblending.
    pub deblend_min_separation: usize,
    /// Minimum peak prominence for deblending (0.0-1.0).
    /// Secondary peaks must be at least this fraction of the primary peak to be
    /// considered for deblending. Prevents noise spikes from causing false splits.
    pub deblend_min_prominence: f32,
    /// Minimum separation between stars for duplicate removal (in pixels).
    /// Stars closer than this are considered duplicates; only brightest is kept.
    pub duplicate_min_separation: f32,
    /// Maximum roundness for a star to be considered valid.
    /// Roundness metrics (GROUND and SROUND from DAOFIND) measure asymmetry.
    /// Circular sources have roundness near 0. Cosmic rays, satellite trails,
    /// and galaxies have higher absolute roundness. Set to 1.0 to disable.
    pub max_roundness: f32,
    /// Enable multi-threshold deblending (SExtractor-style).
    /// When enabled, uses tree-based deblending with multiple threshold levels
    /// instead of simple local maxima detection. More accurate for crowded fields
    /// but slower. Set to true for better crowded field handling.
    pub multi_threshold_deblend: bool,
    /// Number of deblending sub-thresholds for multi-threshold deblending.
    /// Higher values give finer deblending resolution but use more CPU.
    /// SExtractor default: 32. Typical range: 16-64.
    pub deblend_nthresh: usize,
    /// Minimum contrast for multi-threshold deblending (0.0-1.0).
    /// A branch is considered a separate object only if its flux is
    /// at least this fraction of the total flux. Lower values deblend more aggressively.
    /// SExtractor default: 0.005. Set to 1.0 to disable deblending.
    pub deblend_min_contrast: f32,
}

impl Default for StarDetectionConfig {
    fn default() -> Self {
        Self {
            detection_sigma: 4.0,
            min_area: 5,
            max_area: 500,
            max_eccentricity: 0.6,
            edge_margin: 10,
            min_snr: 10.0,
            background_tile_size: 64,
            max_fwhm_deviation: 3.0,
            expected_fwhm: 4.0,
            max_sharpness: 0.7,
            deblend_min_separation: 3,
            deblend_min_prominence: 0.3,
            duplicate_min_separation: 8.0,
            max_roundness: 1.0, // Disabled by default (accept all roundness values)
            multi_threshold_deblend: false, // Use simpler local maxima by default
            deblend_nthresh: 32,
            deblend_min_contrast: 0.005,
        }
    }
}

/// Result of star detection with diagnostics.
#[derive(Debug, Clone)]
pub struct StarDetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: StarDetectionDiagnostics,
}

/// Diagnostic information from star detection.
///
/// Contains statistics and counts from each stage of the detection pipeline
/// for debugging and tuning purposes.
#[derive(Debug, Clone, Default)]
pub struct StarDetectionDiagnostics {
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
    /// Number of stars rejected for low SNR.
    pub rejected_low_snr: usize,
    /// Number of stars rejected for high eccentricity.
    pub rejected_high_eccentricity: usize,
    /// Number of stars rejected as cosmic rays (high sharpness).
    pub rejected_cosmic_rays: usize,
    /// Number of stars rejected as saturated.
    pub rejected_saturated: usize,
    /// Number of stars rejected for non-circular shape (roundness).
    pub rejected_roundness: usize,
    /// Number of stars rejected for abnormal FWHM.
    pub rejected_fwhm_outliers: usize,
    /// Number of duplicate detections removed.
    pub rejected_duplicates: usize,
    /// Final number of stars returned.
    pub final_star_count: usize,
    /// Median FWHM of detected stars (pixels).
    pub median_fwhm: f32,
    /// Median SNR of detected stars.
    pub median_snr: f32,
    /// Background level statistics (min, max, mean).
    pub background_stats: (f32, f32, f32),
    /// Noise level statistics (min, max, mean).
    pub noise_stats: (f32, f32, f32),
}

/// Detect stars in an image.
///
/// Returns detected stars sorted by flux (brightest first) along with
/// diagnostic information from the detection pipeline.
///
/// # Arguments
/// * `pixels` - Image pixel data (grayscale, normalized 0.0-1.0)
/// * `width` - Image width
/// * `height` - Image height
/// * `config` - Detection configuration
pub fn find_stars(
    pixels: &[f32],
    width: usize,
    height: usize,
    config: &StarDetectionConfig,
) -> StarDetectionResult {
    assert_eq!(pixels.len(), width * height, "Pixel count mismatch");

    let mut diagnostics = StarDetectionDiagnostics::default();

    // Step 0: Apply 3x3 median filter to remove Bayer pattern artifacts
    // This smooths out alternating-row sensitivity differences from CFA sensors
    let smoothed = median_filter_3x3(pixels, width, height);

    // Step 1: Estimate background
    let background = estimate_background(&smoothed, width, height, config.background_tile_size);

    // Collect background statistics
    let bg_min = background
        .background
        .iter()
        .fold(f32::MAX, |a, &b| a.min(b));
    let bg_max = background
        .background
        .iter()
        .fold(f32::MIN, |a, &b| a.max(b));
    let bg_mean = background.background.iter().sum::<f32>() / background.background.len() as f32;
    diagnostics.background_stats = (bg_min, bg_max, bg_mean);

    let noise_min = background.noise.iter().fold(f32::MAX, |a, &b| a.min(b));
    let noise_max = background.noise.iter().fold(f32::MIN, |a, &b| a.max(b));
    let noise_mean = background.noise.iter().sum::<f32>() / background.noise.len() as f32;
    diagnostics.noise_stats = (noise_min, noise_max, noise_mean);

    // Step 2: Detect star candidates
    let candidates = if config.expected_fwhm > 0.0 {
        // Apply matched filter (Gaussian convolution) for better faint star detection
        // This is the DAOFIND/SExtractor technique
        tracing::debug!(
            "Applying matched filter with FWHM={:.1} pixels",
            config.expected_fwhm
        );
        let filtered = matched_filter(
            &smoothed,
            width,
            height,
            &background.background,
            config.expected_fwhm,
        );
        detect_stars_filtered(&smoothed, &filtered, width, height, &background, config)
    } else {
        // No matched filter - use standard detection
        detect_stars(&smoothed, width, height, &background, config)
    };
    diagnostics.candidates_after_filtering = candidates.len();
    tracing::debug!("Detected {} star candidates", candidates.len());

    // Step 3: Compute precise centroids
    let stars_after_centroid: Vec<Star> = candidates
        .into_iter()
        .filter_map(|candidate| {
            compute_centroid(pixels, width, height, &background, &candidate, config)
        })
        .collect();
    diagnostics.stars_after_centroid = stars_after_centroid.len();

    // Step 4: Apply quality filters and count rejections
    let mut stars: Vec<Star> = Vec::with_capacity(stars_after_centroid.len());
    for star in stars_after_centroid {
        if star.is_saturated() {
            diagnostics.rejected_saturated += 1;
        } else if star.snr < config.min_snr {
            diagnostics.rejected_low_snr += 1;
        } else if star.eccentricity > config.max_eccentricity {
            diagnostics.rejected_high_eccentricity += 1;
        } else if star.is_cosmic_ray(config.max_sharpness) {
            diagnostics.rejected_cosmic_rays += 1;
        } else if !star.is_round(config.max_roundness) {
            diagnostics.rejected_roundness += 1;
        } else {
            stars.push(star);
        }
    }

    // Sort by flux (brightest first)
    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

    // Filter FWHM outliers - spurious detections often have abnormally large FWHM
    let removed = filter_fwhm_outliers(&mut stars, config.max_fwhm_deviation);
    diagnostics.rejected_fwhm_outliers = removed;
    if removed > 0 {
        tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
    }

    // Remove duplicate detections - keep only the brightest star within min_separation pixels
    let removed = remove_duplicate_stars(&mut stars, config.duplicate_min_separation);
    diagnostics.rejected_duplicates = removed;
    if removed > 0 {
        tracing::debug!("Removed {} duplicate star detections", removed);
    }

    // Compute final statistics
    diagnostics.final_star_count = stars.len();

    if !stars.is_empty() {
        let mut fwhms: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
        fwhms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        diagnostics.median_fwhm = fwhms[fwhms.len() / 2];

        let mut snrs: Vec<f32> = stars.iter().map(|s| s.snr).collect();
        snrs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        diagnostics.median_snr = snrs[snrs.len() / 2];
    }

    StarDetectionResult { stars, diagnostics }
}

/// Remove duplicate star detections that are too close together.
///
/// Keeps the brightest star (by flux) within `min_separation` pixels of each other.
/// Stars must be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    let min_sep_sq = min_separation * min_separation;
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].x - stars[j].x;
            let dy = stars[i].y - stars[j].y;
            if dx * dx + dy * dy < min_sep_sq {
                // Keep i (higher flux since sorted), mark j for removal
                kept[j] = false;
            }
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

    // Filter in place
    let mut write_idx = 0;
    for read_idx in 0..stars.len() {
        if kept[read_idx] {
            if write_idx != read_idx {
                stars[write_idx] = stars[read_idx];
            }
            write_idx += 1;
        }
    }
    stars.truncate(write_idx);

    removed_count
}

/// Compute median and MAD (median absolute deviation) for FWHM filtering.
///
/// Returns (median, mad) computed from the given FWHM values.
fn compute_fwhm_median_mad(fwhms: &[f32]) -> (f32, f32) {
    assert!(!fwhms.is_empty(), "Need at least one FWHM value");

    let mut sorted: Vec<f32> = fwhms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];

    let mut deviations: Vec<f32> = sorted.iter().map(|&f| (f - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = deviations[deviations.len() / 2];

    (median, mad)
}

/// Filter stars by FWHM using MAD-based outlier detection.
///
/// Removes stars with FWHM > median + max_deviation * effective_mad.
/// The effective_mad is max(mad, median * 0.1) to handle uniform FWHM.
///
/// Stars should be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    // Use top half for robust median/MAD estimate
    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = compute_fwhm_median_mad(&fwhms);

    // Use at least 10% of median as minimum MAD
    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}
