//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

// =============================================================================
// Imports
// =============================================================================

use crate::astro_image::AstroImage;
use crate::common::Buffer2;

use super::background::{BackgroundConfig, BackgroundMap};
use super::buffer_pool::BufferPool;
use super::candidate_detection::{self, detect_stars};
use super::centroid::compute_centroid;
use super::config::StarDetectionConfig;
use super::convolution::matched_filter;
use super::fwhm_estimation;
use super::median_filter::median_filter_3x3;
use super::star::Star;

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
    /// Estimated FWHM from auto-estimation (0.0 if not used or disabled).
    pub estimated_fwhm: f32,
    /// Number of stars used for FWHM estimation.
    pub fwhm_estimation_star_count: usize,
    /// Whether FWHM was auto-estimated (true) or manual/disabled (false).
    pub fwhm_was_auto_estimated: bool,
}

// =============================================================================
// StarDetector
// =============================================================================

/// Star detector with builder pattern for convenient star detection.
///
/// Wraps [`StarDetectionConfig`] and provides methods for detecting stars
/// in single images or batches. Includes a buffer pool for efficient memory
/// reuse across multiple detections.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::{StarDetector, AstroImage};
///
/// // Simple usage with defaults
/// let mut detector = StarDetector::new();
/// let result = detector.detect(&image);
///
/// // With custom configuration
/// let config = StarDetectionConfig {
///     psf: PsfConfig {
///         expected_fwhm: 4.0,
///         ..Default::default()
///     },
///     filtering: FilteringConfig {
///         min_snr: 15.0,
///         edge_margin: 20,
///         ..Default::default()
///     },
///     ..Default::default()
/// };
/// let mut detector = StarDetector::from_config(config);
/// let result = detector.detect(&image);
///
/// // Batch detection reuses buffers automatically
/// for image in images {
///     let result = detector.detect(&image);
/// }
/// ```
#[derive(Debug, Default)]
pub struct StarDetector {
    config: StarDetectionConfig,
    /// Buffer pool for reusing allocations across detections.
    /// Lazily initialized on first detect() call.
    buffer_pool: Option<BufferPool>,
}

impl StarDetector {
    /// Create a new star detector with default configuration.
    pub fn new() -> Self {
        Self {
            config: StarDetectionConfig::default(),
            buffer_pool: None,
        }
    }

    /// Create a star detector from an existing configuration.
    pub fn from_config(config: StarDetectionConfig) -> Self {
        Self {
            config,
            buffer_pool: None,
        }
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &StarDetectionConfig {
        &self.config
    }

    /// Clear the buffer pool, freeing all cached memory.
    ///
    /// The pool will be recreated on the next `detect()` call.
    pub fn clear_buffer_pool(&mut self) {
        self.buffer_pool = None;
    }

    /// Detect stars in a single image.
    ///
    /// Uses an internal buffer pool to reuse allocations across multiple calls.
    /// For best performance when processing multiple images of the same size,
    /// reuse the same `StarDetector` instance.
    pub fn detect(&mut self, image: &AstroImage) -> StarDetectionResult {
        self.config.validate();

        let width = image.width();
        let height = image.height();

        // Initialize or reset buffer pool for current dimensions
        let pool = self
            .buffer_pool
            .get_or_insert_with(|| BufferPool::new(width, height));
        pool.reset(width, height);

        // Acquire buffers from pool
        let mut grayscale_image = pool.acquire_f32();
        image.into_grayscale_buffer(&mut grayscale_image);
        let mut scratch = pool.acquire_f32();

        // Step 0a: Apply defect mask if provided
        if let Some(defect_map) = self.config.defect_map.as_ref()
            && !defect_map.is_empty()
        {
            defect_map.apply(&grayscale_image, &mut scratch);
            std::mem::swap(&mut grayscale_image, &mut scratch);
        }

        // Step 0b: Apply 3x3 median filter to remove Bayer pattern artifacts
        // Only applied for CFA sensors; skip for monochrome (~6ms faster on 4K images)
        if image.metadata.is_cfa {
            median_filter_3x3(&grayscale_image, &mut scratch);
            std::mem::swap(&mut grayscale_image, &mut scratch);
        }

        // Step 1: Estimate background using pooled buffers
        let background = self.estimate_background(&grayscale_image);

        // Step 2: Determine effective FWHM (manual > auto-estimate > disabled)
        let effective_fwhm = self.determine_effective_fwhm(&grayscale_image, &background);

        // Step 3: Detect star candidates (with optional matched filter)
        let candidates = {
            let pool = self.buffer_pool.as_mut().unwrap();

            let filtered: Option<&Buffer2<f32>> = if let Some(fwhm) = effective_fwhm.fwhm() {
                tracing::debug!(
                    "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
                    fwhm,
                    self.config.psf.axis_ratio,
                    self.config.psf.angle.to_degrees()
                );

                let mut convolution_scratch = pool.acquire_f32();
                matched_filter(
                    &grayscale_image,
                    &background.background,
                    fwhm,
                    self.config.psf.axis_ratio,
                    self.config.psf.angle,
                    &mut scratch,
                    &mut convolution_scratch,
                );
                pool.release_f32(convolution_scratch);

                Some(&scratch)
            } else {
                None
            };

            let candidates =
                detect_stars(&grayscale_image, filtered, &background, &self.config, pool);

            pool.release_f32(scratch);

            candidates
        };

        let fwhm_estimate = effective_fwhm.estimate();
        let mut diagnostics = StarDetectionDiagnostics {
            candidates_after_filtering: candidates.len(),
            estimated_fwhm: fwhm_estimate.map_or(0.0, |e| e.fwhm),
            fwhm_estimation_star_count: fwhm_estimate.map_or(0, |e| e.star_count),
            fwhm_was_auto_estimated: fwhm_estimate.is_some_and(|e| e.is_estimated),
            ..Default::default()
        };
        tracing::debug!("Detected {} star candidates", candidates.len());

        // Step 4: Compute precise centroids (parallel)
        let mut stars = compute_centroids(candidates, &grayscale_image, &background, &self.config);
        diagnostics.stars_after_centroid = stars.len();

        // Release image buffers back to pool (no longer needed after centroid computation)
        let pool = self.buffer_pool.as_mut().unwrap();
        background.release_to_pool(pool);
        pool.release_f32(grayscale_image);

        // Step 5: Apply quality filters
        let filter_stats = apply_quality_filters(&mut stars, &self.config);
        diagnostics.rejected_saturated = filter_stats.saturated;
        diagnostics.rejected_low_snr = filter_stats.low_snr;
        diagnostics.rejected_high_eccentricity = filter_stats.high_eccentricity;
        diagnostics.rejected_cosmic_rays = filter_stats.cosmic_rays;
        diagnostics.rejected_roundness = filter_stats.roundness;

        // Step 6: Post-processing
        sort_by_flux(&mut stars);

        let removed = filter_fwhm_outliers(&mut stars, self.config.filtering.max_fwhm_deviation);
        diagnostics.rejected_fwhm_outliers = removed;
        if removed > 0 {
            tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
        }

        let removed =
            remove_duplicate_stars(&mut stars, self.config.filtering.duplicate_min_separation);
        diagnostics.rejected_duplicates = removed;
        if removed > 0 {
            tracing::debug!("Removed {} duplicate star detections", removed);
        }

        // Step 7: Compute final statistics
        diagnostics.final_star_count = stars.len();
        if !stars.is_empty() {
            let mut buf: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
            diagnostics.median_fwhm = crate::math::median_f32_mut(&mut buf);

            buf.clear();
            buf.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = crate::math::median_f32_mut(&mut buf);
        }

        StarDetectionResult { stars, diagnostics }
    }

    /// Estimate background from the image using pooled buffers.
    ///
    /// Performs initial estimation and optional iterative refinement.
    fn estimate_background(&mut self, pixels: &Buffer2<f32>) -> BackgroundMap {
        let iterations = self.config.background.refinement.iterations();

        let pool = self.buffer_pool.as_mut().unwrap();
        let mut background = BackgroundMap::from_pool(pool, self.config.background.clone());
        background.estimate(pixels);

        // Refine background if using iterative refinement
        if iterations > 0 {
            let pool = self.buffer_pool.as_mut().unwrap();
            let mut scratch1 = pool.acquire_bit();
            let mut scratch2 = pool.acquire_bit();

            background.refine(pixels, &mut scratch1, &mut scratch2);

            let pool = self.buffer_pool.as_mut().unwrap();
            pool.release_bit(scratch1);
            pool.release_bit(scratch2);
        }

        background
    }

    /// Determine effective FWHM for matched filtering.
    ///
    /// Priority: manual expected_fwhm > auto-estimate > disabled
    fn determine_effective_fwhm(
        &mut self,
        pixels: &Buffer2<f32>,
        background: &BackgroundMap,
    ) -> fwhm_estimation::EffectiveFwhm {
        if self.config.psf.expected_fwhm > f32::EPSILON {
            return fwhm_estimation::EffectiveFwhm::Manual(self.config.psf.expected_fwhm);
        }

        if self.config.psf.auto_estimate {
            let estimate = self.estimate_fwhm_from_bright_stars(pixels, background);
            return fwhm_estimation::EffectiveFwhm::Estimated(estimate);
        }

        fwhm_estimation::EffectiveFwhm::Disabled
    }

    /// Perform first-pass detection and estimate FWHM from bright stars.
    fn estimate_fwhm_from_bright_stars(
        &mut self,
        pixels: &Buffer2<f32>,
        background: &BackgroundMap,
    ) -> fwhm_estimation::FwhmEstimate {
        // Create a modified config for first-pass detection:
        // - Higher sigma threshold: detect only the brightest stars for reliable FWHM measurement
        // - No matched filter (expected_fwhm=0): we don't know the FWHM yet
        // - Smaller min_area: more permissive to catch small bright stars
        // - Higher min_snr: ensure high-quality stars for accurate estimation
        let first_pass_config = StarDetectionConfig {
            background: BackgroundConfig {
                sigma_threshold: self.config.background.sigma_threshold
                    * self.config.psf.estimation_sigma_factor,
                ..self.config.background.clone()
            },
            psf: super::config::PsfConfig {
                expected_fwhm: 0.0,
                ..self.config.psf
            },
            filtering: super::config::FilteringConfig {
                min_area: 3,
                min_snr: self.config.filtering.min_snr * 2.0,
                ..self.config.filtering
            },
            ..self.config.clone()
        };

        let pool = self.buffer_pool.as_mut().unwrap();
        let candidates = detect_stars(pixels, None, background, &first_pass_config, pool);
        tracing::debug!(
            "FWHM estimation: first pass detected {} bright star candidates",
            candidates.len()
        );

        let stars = compute_centroids(candidates, pixels, background, &first_pass_config);

        fwhm_estimation::estimate_fwhm(
            &stars,
            self.config.psf.min_stars_for_estimation,
            4.0,
            self.config.filtering.max_eccentricity,
            self.config.filtering.max_sharpness,
        )
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Compute centroids for star candidates in parallel.
fn compute_centroids(
    candidates: Vec<candidate_detection::StarCandidate>,
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<Star> {
    use rayon::prelude::*;

    candidates
        .into_par_iter()
        .filter_map(|candidate| compute_centroid(pixels, background, &candidate, config))
        .collect()
}

/// Apply quality filters to stars, returning rejection counts.
fn apply_quality_filters(
    stars: &mut Vec<Star>,
    config: &StarDetectionConfig,
) -> QualityFilterStats {
    let mut stats = QualityFilterStats::default();

    stars.retain(|star| {
        if star.is_saturated() {
            stats.saturated += 1;
            false
        } else if star.snr < config.filtering.min_snr {
            stats.low_snr += 1;
            false
        } else if star.eccentricity > config.filtering.max_eccentricity {
            stats.high_eccentricity += 1;
            false
        } else if star.is_cosmic_ray(config.filtering.max_sharpness) {
            stats.cosmic_rays += 1;
            false
        } else if !star.is_round(config.filtering.max_roundness) {
            stats.roundness += 1;
            false
        } else {
            true
        }
    });

    stats
}

#[derive(Debug, Default)]
struct QualityFilterStats {
    saturated: usize,
    low_snr: usize,
    high_eccentricity: usize,
    cosmic_rays: usize,
    roundness: usize,
}

/// Sort stars by flux (brightest first).
fn sort_by_flux(stars: &mut [Star]) {
    stars.sort_by(|a, b| {
        b.flux
            .partial_cmp(&a.flux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Remove duplicate star detections that are too close together.
///
/// Uses spatial hashing for O(n) average case instead of O(n²).
/// Keeps the brightest star (by flux) within `min_separation` pixels.
/// Stars must be sorted by flux (brightest first) before calling.
pub(crate) fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    // For small star counts, O(n²) is faster due to lower overhead
    if stars.len() < 100 {
        return remove_duplicate_stars_simple(stars, min_separation);
    }

    let min_sep_sq = min_separation * min_separation;

    // Find bounding box
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for star in stars.iter() {
        min_x = min_x.min(star.x);
        min_y = min_y.min(star.y);
        max_x = max_x.max(star.x);
        max_y = max_y.max(star.y);
    }

    // Cell size = min_separation ensures we only need to check 3x3 neighborhood
    let cell_size = min_separation;
    let grid_width = ((max_x - min_x) / cell_size).ceil() as usize + 1;
    let grid_height = ((max_y - min_y) / cell_size).ceil() as usize + 1;

    // Grid stores indices of kept stars in each cell
    // Use SmallVec to avoid heap allocation for sparse cells
    let mut grid: Vec<smallvec::SmallVec<[usize; 4]>> =
        vec![smallvec::SmallVec::new(); grid_width * grid_height];

    let mut kept = vec![true; stars.len()];

    // Process stars in flux order (brightest first, already sorted)
    for i in 0..stars.len() {
        let star = &stars[i];
        let cell_x = ((star.x - min_x) / cell_size) as usize;
        let cell_y = ((star.y - min_y) / cell_size) as usize;

        // Check 3x3 neighborhood for duplicates
        let mut is_duplicate = false;
        'outer: for dy in 0..3 {
            let ny = cell_y.wrapping_add(dy).wrapping_sub(1);
            if ny >= grid_height {
                continue;
            }
            for dx in 0..3 {
                let nx = cell_x.wrapping_add(dx).wrapping_sub(1);
                if nx >= grid_width {
                    continue;
                }
                let cell_idx = ny * grid_width + nx;
                for &other_idx in &grid[cell_idx] {
                    let other = &stars[other_idx];
                    let ddx = star.x - other.x;
                    let ddy = star.y - other.y;
                    if ddx * ddx + ddy * ddy < min_sep_sq {
                        is_duplicate = true;
                        break 'outer;
                    }
                }
            }
        }

        if is_duplicate {
            kept[i] = false;
        } else {
            // Add to grid
            let cell_idx = cell_y * grid_width + cell_x;
            grid[cell_idx].push(i);
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

    // Compact the array
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

/// Simple O(n²) duplicate removal for small star counts.
fn remove_duplicate_stars_simple(stars: &mut Vec<Star>, min_separation: f32) -> usize {
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
                kept[j] = false;
            }
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

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

/// Filter stars by FWHM using MAD-based outlier detection.
///
/// Removes stars with FWHM > median + max_deviation × effective_mad.
/// Stars should be sorted by flux (brightest first) before calling.
pub(crate) fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let mut fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = crate::math::median_and_mad_f32_mut(&mut fwhms);

    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}
