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
mod detection;
mod median_filter;

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

pub use background::estimate_background;
pub use centroid::compute_centroid;
pub use detection::detect_stars;
pub use median_filter::median_filter_3x3;

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
}

impl Star {
    /// Check if star is likely saturated.
    ///
    /// Stars with peak values near the maximum (>0.95 for normalized data)
    /// have unreliable centroids.
    pub fn is_saturated(&self) -> bool {
        self.peak > 0.95
    }

    /// Check if star is usable for registration.
    ///
    /// Filters out saturated, elongated, and low-SNR stars.
    pub fn is_usable(&self, min_snr: f32, max_eccentricity: f32) -> bool {
        !self.is_saturated() && self.snr >= min_snr && self.eccentricity <= max_eccentricity
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
        }
    }
}

/// Detect stars in an image.
///
/// Returns a list of detected stars sorted by flux (brightest first).
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
) -> Vec<Star> {
    assert_eq!(pixels.len(), width * height, "Pixel count mismatch");

    // Step 0: Apply 3x3 median filter to remove Bayer pattern artifacts
    // This smooths out alternating-row sensitivity differences from CFA sensors
    let smoothed = median_filter_3x3(pixels, width, height);

    // Step 1: Estimate background
    let background = estimate_background(&smoothed, width, height, config.background_tile_size);

    // Step 2: Detect star candidates (use smoothed for detection)
    let candidates = detect_stars(&smoothed, width, height, &background, config);
    tracing::debug!("Detected {} star candidates", candidates.len());

    // Step 3: Compute precise centroids and filter
    let mut stars: Vec<Star> = candidates
        .into_iter()
        .filter_map(|candidate| {
            compute_centroid(pixels, width, height, &background, &candidate, config)
        })
        .filter(|star| star.is_usable(config.min_snr, config.max_eccentricity))
        .collect();

    // Sort by flux (brightest first)
    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

    // Filter FWHM outliers - spurious detections often have abnormally large FWHM
    let removed = filter_fwhm_outliers(&mut stars, config.max_fwhm_deviation);
    if removed > 0 {
        tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
    }

    // Remove duplicate detections - keep only the brightest star within min_separation pixels
    // Use half the typical FWHM as separation threshold
    let min_separation = 8.0f32;
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

    let dedup_count = kept.iter().filter(|&&k| !k).count();
    if dedup_count > 0 {
        tracing::debug!("Removed {} duplicate star detections", dedup_count);
    }

    stars
        .into_iter()
        .enumerate()
        .filter(|(i, _)| kept[*i])
        .map(|(_, s)| s)
        .collect()
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
