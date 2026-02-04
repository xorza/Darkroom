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

use super::buffer_pool::BufferPool;
use super::candidate_detection::{self, detect_stars};
use super::centroid::compute_centroid;
use super::config::Config;
use super::convolution::matched_filter;
use super::fwhm_estimation;
use super::image_stats::ImageStats;
use super::stages;
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

/// Star detector with reusable buffer pool.
#[derive(Debug)]
pub struct StarDetector {
    config: Config,
    /// Buffer pool for reusing allocations across detections.
    buffer_pool: Option<BufferPool>,
}

impl Default for StarDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StarDetector {
    /// Create a new star detector with default configuration.
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            buffer_pool: None,
        }
    }

    /// Create a star detector from an existing configuration.
    pub fn from_config(config: Config) -> Self {
        Self {
            config,
            buffer_pool: None,
        }
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Clear the buffer pool, freeing all cached memory.
    pub fn clear_buffer_pool(&mut self) {
        self.buffer_pool = None;
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &AstroImage) -> StarDetectionResult {
        self.config.validate();

        let width = image.width();
        let height = image.height();

        // Initialize or reset buffer pool for current dimensions
        let pool = self
            .buffer_pool
            .get_or_insert_with(|| BufferPool::new(width, height));
        pool.reset(width, height);

        // Step 0: Image preparation (grayscale, defect correction, CFA filter)
        let grayscale_image =
            stages::prepare::prepare(image, self.config.defect_map.as_ref(), pool);

        // Step 1: Estimate background and noise
        let background =
            stages::background::estimate_background(&grayscale_image, &self.config, pool);

        // Step 2: Determine effective FWHM (manual > auto-estimate > disabled)
        let effective_fwhm =
            stages::fwhm::estimate_fwhm(&grayscale_image, &background, &self.config, pool);

        // Step 3: Detect star candidates (with optional matched filter)
        let candidates = {
            let pool = self.buffer_pool.as_mut().unwrap();

            let mut scratch = pool.acquire_f32();

            let filtered: Option<&Buffer2<f32>> = if let Some(fwhm) = effective_fwhm.fwhm() {
                tracing::debug!(
                    "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
                    fwhm,
                    self.config.psf_axis_ratio,
                    self.config.psf_angle.to_degrees()
                );

                let mut convolution_scratch = pool.acquire_f32();
                let mut convolution_temp = pool.acquire_f32();
                matched_filter(
                    &grayscale_image,
                    &background.background,
                    fwhm,
                    self.config.psf_axis_ratio,
                    self.config.psf_angle,
                    &mut scratch,
                    &mut convolution_scratch,
                    &mut convolution_temp,
                );
                pool.release_f32(convolution_temp);
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

        // Release image buffers back to pool
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

        let removed = filter_fwhm_outliers(&mut stars, self.config.max_fwhm_deviation);
        diagnostics.rejected_fwhm_outliers = removed;
        if removed > 0 {
            tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
        }

        let removed = remove_duplicate_stars(&mut stars, self.config.duplicate_min_separation);
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
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Compute centroids for star candidates in parallel.
fn compute_centroids(
    candidates: Vec<candidate_detection::StarCandidate>,
    pixels: &Buffer2<f32>,
    background: &ImageStats,
    config: &Config,
) -> Vec<Star> {
    use rayon::prelude::*;

    candidates
        .into_par_iter()
        .filter_map(|candidate| compute_centroid(pixels, background, &candidate, config))
        .collect()
}

/// Apply quality filters to stars, returning rejection counts.
fn apply_quality_filters(stars: &mut Vec<Star>, config: &Config) -> QualityFilterStats {
    let mut stats = QualityFilterStats::default();

    stars.retain(|star| {
        if star.is_saturated() {
            stats.saturated += 1;
            false
        } else if star.snr < config.min_snr {
            stats.low_snr += 1;
            false
        } else if star.eccentricity > config.max_eccentricity {
            stats.high_eccentricity += 1;
            false
        } else if star.is_cosmic_ray(config.max_sharpness) {
            stats.cosmic_rays += 1;
            false
        } else if !star.is_round(config.max_roundness) {
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
pub(crate) fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    if stars.len() < 100 {
        return remove_duplicate_stars_simple(stars, min_separation);
    }

    let min_sep_sq = (min_separation * min_separation) as f64;

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for star in stars.iter() {
        min_x = min_x.min(star.pos.x);
        min_y = min_y.min(star.pos.y);
        max_x = max_x.max(star.pos.x);
        max_y = max_y.max(star.pos.y);
    }

    let cell_size = min_separation as f64;
    let grid_width = ((max_x - min_x) / cell_size).ceil() as usize + 1;
    let grid_height = ((max_y - min_y) / cell_size).ceil() as usize + 1;

    let mut grid: Vec<smallvec::SmallVec<[usize; 4]>> =
        vec![smallvec::SmallVec::new(); grid_width * grid_height];

    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        let star = &stars[i];
        let cell_x = ((star.pos.x - min_x) / cell_size) as usize;
        let cell_y = ((star.pos.y - min_y) / cell_size) as usize;

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
                    let ddx = star.pos.x - other.pos.x;
                    let ddy = star.pos.y - other.pos.y;
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
            let cell_idx = cell_y * grid_width + cell_x;
            grid[cell_idx].push(i);
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

/// Simple O(n²) duplicate removal for small star counts.
fn remove_duplicate_stars_simple(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    let min_sep_sq = (min_separation * min_separation) as f64;
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].pos.x - stars[j].pos.x;
            let dy = stars[i].pos.y - stars[j].pos.y;
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
