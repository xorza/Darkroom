//! Multi-session stacking support.
//!
//! This module provides types and functions for integrating astrophotography data
//! from multiple imaging sessions. Each session may have different:
//! - Sky conditions (transparency, seeing)
//! - Light pollution levels
//! - Temperature (affecting noise)
//! - Equipment configuration
//!
//! The workflow follows PixInsight's best practices:
//! 1. Per-session calibration (bias, dark, flat)
//! 2. Per-session quality assessment
//! 3. Cross-session normalization
//! 4. Session-weighted integration
//!
//! # Example
//!
//! ```ignore
//! use lumos::stacking::session::{Session, SessionConfig, MultiSessionStack};
//!
//! // Create sessions from calibrated frames
//! let session1 = Session::new("2024-01-15")
//!     .with_frames(&night1_paths)
//!     .assess_quality(&star_detection_config)?;
//!
//! let session2 = Session::new("2024-01-16")
//!     .with_frames(&night2_paths)
//!     .assess_quality(&star_detection_config)?;
//!
//! // Stack with session-aware normalization
//! let config = SessionConfig::default();
//! let result = MultiSessionStack::new(vec![session1, session2])
//!     .with_config(config)
//!     .process()?;
//! ```

use std::path::{Path, PathBuf};

use super::RejectionMethod;
use super::local_normalization::{
    LocalNormalizationConfig, LocalNormalizationMap, TileNormalizationStats,
};
use super::weighted::FrameQuality;
use crate::common::Buffer2;
use crate::star_detection::{StarDetectionResult, StarDetector};
use crate::{AstroImage, ImageDimensions};

/// Unique identifier for a session.
pub type SessionId = String;

/// Quality metrics for an entire imaging session.
///
/// Aggregates frame-level quality metrics to characterize the session as a whole.
/// Used for session-level weighting during integration.
#[derive(Debug, Clone, Default)]
pub struct SessionQuality {
    /// Median FWHM across all frames (seeing quality).
    pub median_fwhm: f32,
    /// Median SNR across all frames.
    pub median_snr: f32,
    /// Median eccentricity across all frames (tracking quality).
    pub median_eccentricity: f32,
    /// Median background noise level.
    pub median_noise: f32,
    /// Total number of frames in session.
    pub frame_count: usize,
    /// Number of usable frames (after quality filtering).
    pub usable_frame_count: usize,
    /// Per-frame quality metrics.
    pub frame_qualities: Vec<FrameQuality>,
}

impl SessionQuality {
    /// Compute session quality from individual frame qualities.
    ///
    /// Uses median of frame metrics for robustness to outliers.
    pub fn from_frame_qualities(qualities: &[FrameQuality]) -> Self {
        if qualities.is_empty() {
            return Self::default();
        }

        let frame_count = qualities.len();

        // Compute medians of each metric
        let mut fwhms: Vec<f32> = qualities.iter().map(|q| q.fwhm).collect();
        let mut snrs: Vec<f32> = qualities.iter().map(|q| q.snr).collect();
        let mut eccs: Vec<f32> = qualities.iter().map(|q| q.eccentricity).collect();
        let mut noises: Vec<f32> = qualities.iter().map(|q| q.noise).collect();

        fwhms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        snrs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        eccs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        noises.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let median_idx = frame_count / 2;

        Self {
            median_fwhm: fwhms[median_idx],
            median_snr: snrs[median_idx],
            median_eccentricity: eccs[median_idx],
            median_noise: noises[median_idx],
            frame_count,
            usable_frame_count: frame_count, // Initially all frames are usable
            frame_qualities: qualities.to_vec(),
        }
    }

    /// Compute a session-level weight for integration.
    ///
    /// Higher weight = better quality session.
    /// Formula: weight = (SNR² × (1/FWHM)² × (1/eccentricity)) / noise × √frame_count
    ///
    /// The √frame_count factor gives more weight to sessions with more data,
    /// but with diminishing returns (not linear).
    pub fn compute_weight(&self) -> f32 {
        if self.usable_frame_count == 0 {
            return 0.0;
        }

        let snr_factor = self.median_snr.max(0.1).powi(2);
        let fwhm_factor = (1.0 / self.median_fwhm.max(0.1)).powi(2);
        let ecc_factor = 1.0 / self.median_eccentricity.max(0.1);
        let noise_factor = 1.0 / self.median_noise.max(0.001);
        let count_factor = (self.usable_frame_count as f32).sqrt();

        snr_factor * fwhm_factor * ecc_factor * noise_factor * count_factor
    }

    /// Filter frames by quality threshold.
    ///
    /// Returns indices of frames that pass the quality threshold.
    /// Threshold is relative to session median (e.g., 0.5 means keep frames
    /// with weight >= 50% of median frame weight).
    pub fn filter_by_threshold(&mut self, threshold: f32) -> Vec<usize> {
        if self.frame_qualities.is_empty() {
            return vec![];
        }

        // Compute weights for all frames
        let weights: Vec<f32> = self
            .frame_qualities
            .iter()
            .map(|q| q.compute_weight())
            .collect();

        // Find median weight
        let mut sorted_weights = weights.clone();
        sorted_weights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median_weight = sorted_weights[sorted_weights.len() / 2];

        // Keep frames above threshold
        let cutoff = median_weight * threshold;
        let passing_indices: Vec<usize> = weights
            .iter()
            .enumerate()
            .filter(|(_, w)| **w >= cutoff)
            .map(|(i, _)| i)
            .collect();

        self.usable_frame_count = passing_indices.len();
        passing_indices
    }
}

/// Represents a single imaging session.
///
/// A session is a contiguous set of frames taken under similar conditions,
/// typically during a single night. Each session should be calibrated with
/// its own calibration frames (bias, dark, flat) before being added here.
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique identifier for this session (e.g., date, "2024-01-15").
    pub id: SessionId,
    /// Paths to calibrated light frames.
    pub frame_paths: Vec<PathBuf>,
    /// Quality metrics for this session.
    pub quality: SessionQuality,
    /// Optional reference frame index for registration.
    pub reference_frame: Option<usize>,
}

impl Session {
    /// Create a new session with the given identifier.
    ///
    /// The identifier should be unique and descriptive (e.g., date string).
    pub fn new(id: impl Into<SessionId>) -> Self {
        Self {
            id: id.into(),
            frame_paths: Vec::new(),
            quality: SessionQuality::default(),
            reference_frame: None,
        }
    }

    /// Add frame paths to this session.
    pub fn with_frames<P: AsRef<Path>>(mut self, paths: &[P]) -> Self {
        self.frame_paths = paths.iter().map(|p| p.as_ref().to_path_buf()).collect();
        self
    }

    /// Set the reference frame index for registration.
    pub fn with_reference_frame(mut self, index: usize) -> Self {
        self.reference_frame = Some(index);
        self
    }

    /// Assess quality of all frames in this session.
    ///
    /// Loads each frame, runs star detection, and computes quality metrics.
    /// This is computationally expensive but only needs to be done once per session.
    ///
    /// # Arguments
    /// * `detector` - Star detector for finding stars
    ///
    /// # Returns
    /// * `Ok(Self)` - Session with quality metrics populated
    /// * `Err` - If frame loading or star detection fails
    pub fn assess_quality(mut self, detector: &mut StarDetector) -> anyhow::Result<Self> {
        if self.frame_paths.is_empty() {
            return Ok(self);
        }

        let mut frame_qualities = Vec::with_capacity(self.frame_paths.len());

        for path in &self.frame_paths {
            let image = AstroImage::from_file(path)?;
            let result = detector.detect(&image);
            let quality = FrameQuality::from_detection_result(&result);
            frame_qualities.push(quality);
        }

        self.quality = SessionQuality::from_frame_qualities(&frame_qualities);
        Ok(self)
    }

    /// Assess quality using pre-computed star detection results.
    ///
    /// Use this when you've already run star detection for registration purposes.
    pub fn with_detection_results(mut self, results: &[StarDetectionResult]) -> Self {
        let frame_qualities: Vec<FrameQuality> = results
            .iter()
            .map(FrameQuality::from_detection_result)
            .collect();
        self.quality = SessionQuality::from_frame_qualities(&frame_qualities);
        self
    }

    /// Get the number of frames in this session.
    pub fn frame_count(&self) -> usize {
        self.frame_paths.len()
    }

    /// Get the number of usable frames (after quality filtering).
    pub fn usable_frame_count(&self) -> usize {
        self.quality.usable_frame_count
    }

    /// Filter frames by quality and return paths to usable frames.
    pub fn filter_frames(&mut self, threshold: f32) -> Vec<PathBuf> {
        let indices = self.quality.filter_by_threshold(threshold);
        indices
            .iter()
            .map(|&i| self.frame_paths[i].clone())
            .collect()
    }

    /// Select the best frame as reference for registration.
    ///
    /// Chooses the frame with the highest quality weight.
    pub fn select_best_reference(&mut self) -> Option<usize> {
        if self.quality.frame_qualities.is_empty() {
            return None;
        }

        let best_idx = self
            .quality
            .frame_qualities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.compute_weight()
                    .partial_cmp(&b.compute_weight())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);

        self.reference_frame = best_idx;
        best_idx
    }
}

/// Configuration for multi-session stacking.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Minimum quality threshold for frame inclusion (0.0-1.0).
    /// Frames with weight below threshold × median_weight are excluded.
    /// Default: 0.5 (keep frames with >= 50% of median quality).
    pub quality_threshold: f32,

    /// Whether to use session-level weighting during integration.
    /// When true, better sessions contribute more to the final stack.
    /// Default: true.
    pub use_session_weights: bool,

    /// Rejection method for pixel-level outlier removal.
    /// Default: SigmaClip with sigma=2.5, iterations=3.
    pub rejection: RejectionMethod,

    /// Whether to use local normalization.
    /// Recommended for multi-session data to handle gradient differences.
    /// Default: true.
    pub use_local_normalization: bool,

    /// Tile size for local normalization (if enabled).
    /// Default: 128 pixels.
    pub normalization_tile_size: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            quality_threshold: 0.5,
            use_session_weights: true,
            rejection: RejectionMethod::default(),
            use_local_normalization: true,
            normalization_tile_size: 128,
        }
    }
}

impl SessionConfig {
    /// Create config with custom quality threshold.
    pub fn with_quality_threshold(mut self, threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Quality threshold must be between 0.0 and 1.0"
        );
        self.quality_threshold = threshold;
        self
    }

    /// Disable session-level weighting (all sessions contribute equally).
    pub fn without_session_weights(mut self) -> Self {
        self.use_session_weights = false;
        self
    }

    /// Set rejection method.
    pub fn with_rejection(mut self, rejection: RejectionMethod) -> Self {
        self.rejection = rejection;
        self
    }

    /// Disable local normalization (use global normalization instead).
    pub fn without_local_normalization(mut self) -> Self {
        self.use_local_normalization = false;
        self
    }

    /// Set tile size for local normalization.
    pub fn with_normalization_tile_size(mut self, size: usize) -> Self {
        assert!(size >= 32, "Tile size must be at least 32");
        self.normalization_tile_size = size;
        self
    }
}

/// Multi-session stacking orchestrator.
///
/// Combines frames from multiple imaging sessions with proper normalization
/// and quality-based weighting.
#[derive(Debug)]
pub struct MultiSessionStack {
    /// Sessions to be stacked.
    pub sessions: Vec<Session>,
    /// Stacking configuration.
    pub config: SessionConfig,
}

impl MultiSessionStack {
    /// Create a new multi-session stack.
    pub fn new(sessions: Vec<Session>) -> Self {
        Self {
            sessions,
            config: SessionConfig::default(),
        }
    }

    /// Set the stacking configuration.
    pub fn with_config(mut self, config: SessionConfig) -> Self {
        self.config = config;
        self
    }

    /// Get total frame count across all sessions.
    pub fn total_frame_count(&self) -> usize {
        self.sessions.iter().map(|s| s.frame_count()).sum()
    }

    /// Get total usable frame count across all sessions.
    pub fn total_usable_frame_count(&self) -> usize {
        self.sessions.iter().map(|s| s.usable_frame_count()).sum()
    }

    /// Compute normalized session weights.
    ///
    /// Returns weights for each session, normalized to sum to 1.0.
    /// If session weighting is disabled, returns equal weights.
    pub fn compute_session_weights(&self) -> Vec<f32> {
        if !self.config.use_session_weights {
            let n = self.sessions.len();
            return vec![1.0 / n as f32; n];
        }

        let weights: Vec<f32> = self
            .sessions
            .iter()
            .map(|s| s.quality.compute_weight())
            .collect();

        let sum: f32 = weights.iter().sum();
        if sum > f32::EPSILON {
            weights.iter().map(|w| w / sum).collect()
        } else {
            let n = self.sessions.len();
            vec![1.0 / n as f32; n]
        }
    }

    /// Compute per-frame weights combining session and frame quality.
    ///
    /// Each frame's weight = session_weight × normalized_frame_weight_within_session
    pub fn compute_frame_weights(&self) -> Vec<f32> {
        let session_weights = self.compute_session_weights();
        let mut all_weights = Vec::new();

        for (session, &session_weight) in self.sessions.iter().zip(&session_weights) {
            // Compute frame weights within this session
            let frame_weights: Vec<f32> = session
                .quality
                .frame_qualities
                .iter()
                .map(|q| q.compute_weight())
                .collect();

            // Normalize frame weights within session
            let sum: f32 = frame_weights.iter().sum();
            let normalized: Vec<f32> = if sum > f32::EPSILON {
                frame_weights.iter().map(|w| w / sum).collect()
            } else {
                vec![1.0 / frame_weights.len() as f32; frame_weights.len()]
            };

            // Combine with session weight
            for fw in normalized {
                all_weights.push(session_weight * fw);
            }
        }

        // Final normalization
        let total: f32 = all_weights.iter().sum();
        if total > f32::EPSILON {
            all_weights.iter_mut().for_each(|w| *w /= total);
        }

        all_weights
    }

    /// Filter frames across all sessions by quality threshold.
    ///
    /// Returns paths to all frames that pass the quality threshold.
    pub fn filter_all_frames(&mut self) -> Vec<PathBuf> {
        let mut all_paths = Vec::new();
        for session in &mut self.sessions {
            let paths = session.filter_frames(self.config.quality_threshold);
            all_paths.extend(paths);
        }
        all_paths
    }

    /// Get all frame paths across sessions (without filtering).
    pub fn all_frame_paths(&self) -> Vec<PathBuf> {
        self.sessions
            .iter()
            .flat_map(|s| s.frame_paths.clone())
            .collect()
    }

    /// Get a summary of the multi-session stack.
    pub fn summary(&self) -> MultiSessionSummary {
        let session_weights = self.compute_session_weights();

        let session_summaries: Vec<SessionSummary> = self
            .sessions
            .iter()
            .zip(&session_weights)
            .map(|(s, &weight)| SessionSummary {
                id: s.id.clone(),
                frame_count: s.frame_count(),
                usable_frame_count: s.usable_frame_count(),
                median_fwhm: s.quality.median_fwhm,
                median_snr: s.quality.median_snr,
                weight,
            })
            .collect();

        MultiSessionSummary {
            total_sessions: self.sessions.len(),
            total_frames: self.total_frame_count(),
            total_usable_frames: self.total_usable_frame_count(),
            sessions: session_summaries,
        }
    }
}

/// Summary of a single session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: SessionId,
    /// Total frame count.
    pub frame_count: usize,
    /// Usable frame count after quality filtering.
    pub usable_frame_count: usize,
    /// Median FWHM (seeing quality).
    pub median_fwhm: f32,
    /// Median SNR.
    pub median_snr: f32,
    /// Normalized session weight.
    pub weight: f32,
}

/// Summary of a multi-session stack.
#[derive(Debug, Clone)]
pub struct MultiSessionSummary {
    /// Total number of sessions.
    pub total_sessions: usize,
    /// Total frame count across all sessions.
    pub total_frames: usize,
    /// Total usable frames after quality filtering.
    pub total_usable_frames: usize,
    /// Per-session summaries.
    pub sessions: Vec<SessionSummary>,
}

impl std::fmt::Display for MultiSessionSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Multi-Session Stack Summary")?;
        writeln!(f, "===========================")?;
        writeln!(
            f,
            "Total: {} sessions, {} frames ({} usable)",
            self.total_sessions, self.total_frames, self.total_usable_frames
        )?;
        writeln!(f)?;
        writeln!(
            f,
            "{:<12} {:>6} {:>8} {:>8} {:>8} {:>8}",
            "Session", "Frames", "Usable", "FWHM", "SNR", "Weight"
        )?;
        writeln!(f, "{:-<58}", "")?;
        for s in &self.sessions {
            writeln!(
                f,
                "{:<12} {:>6} {:>8} {:>8.2} {:>8.1} {:>8.3}",
                s.id, s.frame_count, s.usable_frame_count, s.median_fwhm, s.median_snr, s.weight
            )?;
        }
        Ok(())
    }
}

// ============================================================================
// Session-Aware Local Normalization
// ============================================================================

/// Session-aware local normalization for multi-session stacking.
///
/// Holds the reference frame statistics and can normalize frames from any session
/// to match the reference. The reference is typically selected from the best
/// quality session.
///
/// # Example
///
/// ```ignore
/// use lumos::stacking::session::{MultiSessionStack, SessionNormalization};
///
/// let stack = MultiSessionStack::new(sessions);
///
/// // Create normalizer from best session's reference frame
/// let normalizer = stack.create_session_normalizer(&reference_pixels, width, height)?;
///
/// // Normalize a frame from any session
/// let normalized = normalizer.normalize_frame(&frame_pixels);
/// ```
#[derive(Debug, Clone)]
pub struct SessionNormalization {
    /// Tile statistics from the reference frame.
    reference_stats: TileNormalizationStats,
    /// Local normalization configuration.
    config: LocalNormalizationConfig,
}

impl SessionNormalization {
    /// Create a new session normalizer from a reference frame.
    ///
    /// The reference frame should be from the best quality session (typically
    /// the frame with the best FWHM/SNR combination).
    ///
    /// # Arguments
    /// * `reference_pixels` - Pixel buffer from the reference frame
    /// * `config` - Local normalization configuration
    ///
    /// # Panics
    ///
    /// Panics if the image is smaller than the configured tile size.
    pub fn new(reference_pixels: &Buffer2<f32>, config: LocalNormalizationConfig) -> Self {
        let reference_stats = TileNormalizationStats::compute(reference_pixels, &config);

        Self {
            reference_stats,
            config,
        }
    }

    /// Create from pre-computed reference statistics.
    ///
    /// Use this when you want to reuse previously computed statistics.
    pub fn from_stats(
        reference_stats: TileNormalizationStats,
        config: LocalNormalizationConfig,
    ) -> Self {
        Self {
            reference_stats,
            config,
        }
    }

    /// Get the reference tile statistics.
    pub fn reference_stats(&self) -> &TileNormalizationStats {
        &self.reference_stats
    }

    /// Get the normalization configuration.
    pub fn config(&self) -> &LocalNormalizationConfig {
        &self.config
    }

    /// Get image dimensions.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.reference_stats.width, self.reference_stats.height)
    }

    /// Compute a normalization map for a target frame.
    ///
    /// The map can be applied to the frame to normalize it to match the reference.
    ///
    /// # Arguments
    /// * `target_pixels` - Pixel buffer from the frame to normalize
    ///
    /// # Panics
    ///
    /// Panics if the target frame dimensions don't match the reference.
    pub fn compute_map(&self, target_pixels: &Buffer2<f32>) -> LocalNormalizationMap {
        let target_stats = TileNormalizationStats::compute(target_pixels, &self.config);
        LocalNormalizationMap::compute(&self.reference_stats, &target_stats)
    }

    /// Normalize a frame to match the reference.
    ///
    /// This is a convenience method that computes the normalization map and applies it.
    ///
    /// # Arguments
    /// * `target_pixels` - Pixel buffer from the frame to normalize
    ///
    /// # Returns
    /// Normalized pixel buffer.
    ///
    /// # Panics
    ///
    /// Panics if the target frame dimensions don't match the reference.
    pub fn normalize_frame(&self, target_pixels: &Buffer2<f32>) -> Buffer2<f32> {
        let map = self.compute_map(target_pixels);
        map.apply_to_new(target_pixels)
    }

    /// Normalize a frame in-place.
    ///
    /// # Arguments
    /// * `target_pixels` - Pixel buffer to normalize (modified in-place)
    ///
    /// # Panics
    ///
    /// Panics if the target frame dimensions don't match the reference.
    pub fn normalize_frame_in_place(&self, target_pixels: &mut Buffer2<f32>) {
        let map = self.compute_map(target_pixels);
        map.apply(target_pixels);
    }
}

impl MultiSessionStack {
    /// Select the best session for providing the reference frame.
    ///
    /// Returns the index of the session with the highest quality weight.
    /// This session's best frame should be used as the global reference for
    /// cross-session normalization.
    pub fn select_best_session(&self) -> Option<usize> {
        if self.sessions.is_empty() {
            return None;
        }

        self.sessions
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.quality
                    .compute_weight()
                    .partial_cmp(&b.quality.compute_weight())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// Select the global reference frame for normalization.
    ///
    /// Returns (session_index, frame_index) of the best frame in the best session.
    /// This frame should be loaded and used to create a `SessionNormalization`.
    ///
    /// # Returns
    /// * `Some((session_idx, frame_idx))` - Indices of the best frame
    /// * `None` - If no sessions or no frames with quality data
    pub fn select_global_reference(&self) -> Option<(usize, usize)> {
        let best_session_idx = self.select_best_session()?;
        let session = &self.sessions[best_session_idx];

        // Find best frame in best session
        if session.quality.frame_qualities.is_empty() {
            // No quality data, use first frame
            if session.frame_paths.is_empty() {
                return None;
            }
            return Some((best_session_idx, 0));
        }

        let best_frame_idx = session
            .quality
            .frame_qualities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.compute_weight()
                    .partial_cmp(&b.compute_weight())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)?;

        Some((best_session_idx, best_frame_idx))
    }

    /// Get the path to the global reference frame.
    ///
    /// # Returns
    /// * `Some(path)` - Path to the best frame for normalization reference
    /// * `None` - If no suitable reference frame found
    pub fn global_reference_path(&self) -> Option<&Path> {
        let (session_idx, frame_idx) = self.select_global_reference()?;
        self.sessions
            .get(session_idx)?
            .frame_paths
            .get(frame_idx)
            .map(|p| p.as_path())
    }

    /// Create a session normalizer from the automatically selected reference frame.
    ///
    /// Loads the best frame from the best session and creates a `SessionNormalization`
    /// that can be used to normalize all other frames.
    ///
    /// # Returns
    /// * `Ok(SessionNormalization)` - Normalizer ready to use
    /// * `Err` - If no suitable reference frame or image loading fails
    pub fn create_session_normalizer(&self) -> anyhow::Result<SessionNormalization> {
        let reference_path = self
            .global_reference_path()
            .ok_or_else(|| anyhow::anyhow!("No suitable reference frame found"))?;

        let reference_image = AstroImage::from_file(reference_path)?;
        let width = reference_image.width();
        let height = reference_image.height();

        // Extract the first channel (luminance for grayscale, red for RGB)
        let reference_pixels = Buffer2::new(width, height, reference_image.channel(0).to_vec());

        let config = LocalNormalizationConfig::new(self.config.normalization_tile_size);

        Ok(SessionNormalization::new(&reference_pixels, config))
    }

    /// Create a session normalizer from a specific reference frame.
    ///
    /// Use this when you want to manually specify the reference frame rather than
    /// using automatic selection.
    ///
    /// # Arguments
    /// * `reference_pixels` - Pixel data from the reference frame
    /// * `width` - Image width
    /// * `height` - Image height
    pub fn create_session_normalizer_from_pixels(
        &self,
        reference_pixels: &Buffer2<f32>,
    ) -> SessionNormalization {
        let config = LocalNormalizationConfig::new(self.config.normalization_tile_size);
        SessionNormalization::new(reference_pixels, config)
    }
}

/// Reference frame information for session normalization.
#[derive(Debug, Clone)]
pub struct GlobalReferenceInfo {
    /// Session index containing the reference frame.
    pub session_idx: usize,
    /// Frame index within the session.
    pub frame_idx: usize,
    /// Session identifier.
    pub session_id: SessionId,
    /// Path to the reference frame.
    pub path: PathBuf,
}

impl MultiSessionStack {
    /// Get detailed information about the global reference frame.
    ///
    /// Returns information about which frame was selected as the global reference
    /// for normalization, useful for logging and debugging.
    pub fn global_reference_info(&self) -> Option<GlobalReferenceInfo> {
        let (session_idx, frame_idx) = self.select_global_reference()?;
        let session = &self.sessions[session_idx];
        let path = session.frame_paths.get(frame_idx)?.clone();

        Some(GlobalReferenceInfo {
            session_idx,
            frame_idx,
            session_id: session.id.clone(),
            path,
        })
    }

    /// Perform session-weighted integration.
    ///
    /// Stacks all frames from all sessions using session-weighted integration:
    /// 1. Optionally normalizes each frame to match the global reference
    /// 2. Computes per-frame weights combining session and frame quality
    /// 3. Uses weighted mean stacking with the configured rejection method
    ///
    /// # Returns
    /// * `Ok(SessionWeightedStackResult)` - Stacked image with metadata
    /// * `Err` - If no frames, image loading fails, or dimensions mismatch
    ///
    /// # Example
    ///
    /// ```ignore
    /// use lumos::stacking::session::{MultiSessionStack, SessionConfig};
    ///
    /// let stack = MultiSessionStack::new(vec![session1, session2])
    ///     .with_config(SessionConfig::default());
    ///
    /// let result = stack.stack_session_weighted()?;
    /// println!("Stacked {} frames from {} sessions", result.total_frames, result.session_count);
    /// ```
    pub fn stack_session_weighted(&self) -> anyhow::Result<SessionWeightedStackResult> {
        use super::weighted::{WeightedConfig, stack_weighted_from_paths};
        use super::{FrameType, ProgressCallback};

        // Get all frame paths
        let all_paths = self.all_frame_paths();
        if all_paths.is_empty() {
            anyhow::bail!("No frames to stack");
        }

        // Compute per-frame weights
        let frame_weights = self.compute_frame_weights();
        if frame_weights.len() != all_paths.len() {
            anyhow::bail!(
                "Weight count ({}) doesn't match frame count ({})",
                frame_weights.len(),
                all_paths.len()
            );
        }

        // Get session weights for reporting
        let session_weights = self.compute_session_weights();

        // Load and normalize frames if local normalization is enabled
        let stacked_image = if self.config.use_local_normalization {
            self.stack_with_normalization(&all_paths, &frame_weights)?
        } else {
            // Stack directly without normalization
            let config = WeightedConfig::with_weights(frame_weights.clone())
                .with_rejection(self.config.rejection.clone());

            stack_weighted_from_paths(
                &all_paths,
                FrameType::Light,
                &config,
                ProgressCallback::default(),
            )?
        };

        Ok(SessionWeightedStackResult {
            image: stacked_image,
            session_count: self.sessions.len(),
            total_frames: all_paths.len(),
            session_weights,
            frame_weights,
            reference_info: self.global_reference_info(),
            used_normalization: self.config.use_local_normalization,
        })
    }

    /// Stack frames with local normalization applied.
    ///
    /// This loads each frame, normalizes it to match the global reference,
    /// then performs weighted mean stacking on the normalized data.
    fn stack_with_normalization(
        &self,
        paths: &[PathBuf],
        weights: &[f32],
    ) -> anyhow::Result<AstroImage> {
        use rayon::prelude::*;

        if paths.is_empty() {
            anyhow::bail!("No frames to stack");
        }

        // Create session normalizer from the best reference frame
        let normalizer = self.create_session_normalizer()?;
        let (width, height) = normalizer.dimensions();

        // Load the first frame to get channel count
        let first_image = AstroImage::from_file(&paths[0])?;
        let channels = first_image.channels();

        // Verify dimensions match
        if first_image.width() != width || first_image.height() != height {
            anyhow::bail!(
                "First frame dimensions ({}, {}) don't match reference ({}, {})",
                first_image.width(),
                first_image.height(),
                width,
                height
            );
        }

        // Load and normalize all frames in parallel
        let normalized_frames: Vec<Vec<f32>> = paths
            .par_iter()
            .map(|path| {
                let image = AstroImage::from_file(path).expect("Failed to load image");
                let w = image.width();
                let h = image.height();

                // For multi-channel images, normalize each channel separately
                if channels == 1 {
                    // Single channel - normalize directly
                    let channel_buf = Buffer2::new(w, h, image.channel(0).to_vec());
                    normalizer.normalize_frame(&channel_buf).to_vec()
                } else {
                    // Multi-channel - normalize each channel using planar access
                    let pixel_count = w * h;
                    let mut normalized = vec![0.0f32; pixel_count * channels];

                    for c in 0..channels {
                        // Normalize channel directly from planar storage
                        let channel_buf = Buffer2::new(w, h, image.channel(c).to_vec());
                        let normalized_channel = normalizer.normalize_frame(&channel_buf);

                        // Write back interleaved
                        for (i, &val) in normalized_channel.iter().enumerate() {
                            normalized[i * channels + c] = val;
                        }
                    }

                    normalized
                }
            })
            .collect();

        // Perform weighted mean stacking on normalized frames
        let pixel_count = width * height * channels;
        let mut result_pixels = vec![0.0f32; pixel_count];

        // For each pixel position, compute weighted mean
        for i in 0..pixel_count {
            let mut weighted_sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            for (frame_idx, frame) in normalized_frames.iter().enumerate() {
                let weight = weights[frame_idx];
                weighted_sum += frame[i] * weight;
                weight_sum += weight;
            }

            result_pixels[i] = if weight_sum > f32::EPSILON {
                weighted_sum / weight_sum
            } else {
                // Fallback to simple mean
                normalized_frames.iter().map(|f| f[i]).sum::<f32>() / normalized_frames.len() as f32
            };
        }

        Ok(AstroImage::from_pixels(
            ImageDimensions::new(width, height, channels),
            result_pixels,
        ))
    }
}

/// Result of session-weighted integration.
#[derive(Debug)]
pub struct SessionWeightedStackResult {
    /// The stacked image.
    pub image: AstroImage,
    /// Number of sessions that contributed to the stack.
    pub session_count: usize,
    /// Total number of frames stacked.
    pub total_frames: usize,
    /// Normalized session weights used.
    pub session_weights: Vec<f32>,
    /// Per-frame weights used (already normalized).
    pub frame_weights: Vec<f32>,
    /// Information about the reference frame used for normalization.
    pub reference_info: Option<GlobalReferenceInfo>,
    /// Whether local normalization was applied.
    pub used_normalization: bool,
}

impl SessionWeightedStackResult {
    /// Get the stacked image.
    pub fn image(&self) -> &AstroImage {
        &self.image
    }

    /// Consume the result and return just the image.
    pub fn into_image(self) -> AstroImage {
        self.image
    }

    /// Get the total weight contribution from each session.
    ///
    /// Returns a vector of (session_index, total_weight) pairs.
    pub fn session_contributions(&self, stack: &MultiSessionStack) -> Vec<(usize, f32)> {
        let mut contributions = Vec::new();
        let mut frame_idx = 0;

        for (session_idx, session) in stack.sessions.iter().enumerate() {
            let frame_count = session.frame_count();
            let session_total: f32 = self.frame_weights[frame_idx..frame_idx + frame_count]
                .iter()
                .sum();
            contributions.push((session_idx, session_total));
            frame_idx += frame_count;
        }

        contributions
    }

    /// Remove gradient from the stacked image.
    ///
    /// This is a post-stack processing step that removes sky gradients caused by
    /// light pollution, moon glow, or twilight. The gradient is estimated from
    /// background samples and either subtracted (additive gradients) or divided
    /// (multiplicative effects like vignetting).
    ///
    /// For multi-channel images, gradient removal is applied to each channel
    /// independently.
    ///
    /// # Arguments
    /// * `config` - Gradient removal configuration
    ///
    /// # Returns
    /// * `Ok(())` - Gradient successfully removed (image modified in-place)
    /// * `Err` - If gradient removal fails (e.g., insufficient samples)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use lumos::stacking::session::{MultiSessionStack, SessionConfig};
    /// use lumos::stacking::gradient_removal::GradientRemovalConfig;
    ///
    /// let stack = MultiSessionStack::new(vec![session1, session2])
    ///     .with_config(SessionConfig::default());
    ///
    /// let mut result = stack.stack_session_weighted()?;
    ///
    /// // Remove linear gradient (degree 1)
    /// let config = GradientRemovalConfig::polynomial(1);
    /// result.remove_gradient(&config)?;
    /// ```
    pub fn remove_gradient(
        &mut self,
        config: &super::gradient_removal::GradientRemovalConfig,
    ) -> Result<(), super::gradient_removal::GradientRemovalError> {
        let width = self.image.width();
        let height = self.image.height();
        let channels = self.image.channels();

        // Process each channel using planar access
        for c in 0..channels {
            let channel_pixels = self.image.channel(c);
            let corrected = super::gradient_removal::remove_gradient_simple(
                channel_pixels,
                width,
                height,
                config,
            )?;
            self.image.channel_mut(c).copy_from_slice(&corrected);
        }

        Ok(())
    }
}

impl std::fmt::Display for SessionWeightedStackResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Session-Weighted Stack Result")?;
        writeln!(f, "==============================")?;
        writeln!(
            f,
            "Image: {}x{} ({} channels)",
            self.image.width(),
            self.image.height(),
            self.image.channels()
        )?;
        writeln!(
            f,
            "Sessions: {}, Total frames: {}",
            self.session_count, self.total_frames
        )?;
        writeln!(
            f,
            "Used normalization: {}",
            if self.used_normalization { "yes" } else { "no" }
        )?;
        if let Some(ref info) = self.reference_info {
            writeln!(
                f,
                "Reference: session '{}', frame {}",
                info.session_id, info.frame_idx
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::synthetic::patterns;

    // ========== SessionQuality Tests ==========

    #[test]
    fn test_session_quality_default() {
        let quality = SessionQuality::default();
        assert_eq!(quality.frame_count, 0);
        assert_eq!(quality.usable_frame_count, 0);
    }

    #[test]
    fn test_session_quality_from_frame_qualities() {
        let qualities = vec![
            FrameQuality {
                snr: 50.0,
                fwhm: 2.5,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
            FrameQuality {
                snr: 60.0,
                fwhm: 2.8,
                eccentricity: 0.15,
                noise: 0.012,
                star_count: 120,
            },
            FrameQuality {
                snr: 40.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                noise: 0.015,
                star_count: 80,
            },
        ];

        let session_quality = SessionQuality::from_frame_qualities(&qualities);

        assert_eq!(session_quality.frame_count, 3);
        assert_eq!(session_quality.usable_frame_count, 3);
        // Median values (middle of sorted arrays)
        assert!((session_quality.median_fwhm - 2.8).abs() < f32::EPSILON);
        assert!((session_quality.median_snr - 50.0).abs() < f32::EPSILON);
        assert!((session_quality.median_eccentricity - 0.15).abs() < f32::EPSILON);
        assert!((session_quality.median_noise - 0.012).abs() < f32::EPSILON);
    }

    #[test]
    fn test_session_quality_from_empty() {
        let qualities: Vec<FrameQuality> = vec![];
        let session_quality = SessionQuality::from_frame_qualities(&qualities);
        assert_eq!(session_quality.frame_count, 0);
    }

    #[test]
    fn test_session_quality_compute_weight() {
        let quality = SessionQuality {
            median_fwhm: 2.5,
            median_snr: 50.0,
            median_eccentricity: 0.15,
            median_noise: 0.01,
            frame_count: 10,
            usable_frame_count: 10,
            frame_qualities: vec![],
        };

        let weight = quality.compute_weight();
        assert!(weight > 0.0);

        // Better seeing (lower FWHM) should give higher weight
        let better_seeing = SessionQuality {
            median_fwhm: 2.0,
            ..quality.clone()
        };
        assert!(better_seeing.compute_weight() > weight);

        // More frames should give higher weight
        let more_frames = SessionQuality {
            usable_frame_count: 20,
            ..quality.clone()
        };
        assert!(more_frames.compute_weight() > weight);
    }

    #[test]
    fn test_session_quality_zero_usable_frames_zero_weight() {
        let quality = SessionQuality {
            median_fwhm: 2.5,
            median_snr: 50.0,
            median_eccentricity: 0.15,
            median_noise: 0.01,
            frame_count: 10,
            usable_frame_count: 0,
            frame_qualities: vec![],
        };

        assert!((quality.compute_weight() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_session_quality_filter_by_threshold() {
        let frame_qualities = vec![
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
            FrameQuality {
                snr: 50.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                noise: 0.02,
                star_count: 50,
            },
            FrameQuality {
                snr: 10.0,
                fwhm: 5.0,
                eccentricity: 0.5,
                noise: 0.05,
                star_count: 10,
            },
        ];

        let mut quality = SessionQuality::from_frame_qualities(&frame_qualities);

        // Threshold of 0.5 should keep at least the best frames
        let passing = quality.filter_by_threshold(0.5);
        assert!(!passing.is_empty());
        assert!(passing.len() <= 3);
        assert!(quality.usable_frame_count <= 3);
    }

    // ========== Session Tests ==========

    #[test]
    fn test_session_new() {
        let session = Session::new("2024-01-15");
        assert_eq!(session.id, "2024-01-15");
        assert!(session.frame_paths.is_empty());
        assert!(session.reference_frame.is_none());
    }

    #[test]
    fn test_session_with_frames() {
        let paths = [PathBuf::from("/a.fits"), PathBuf::from("/b.fits")];
        let session = Session::new("test").with_frames(&paths);
        assert_eq!(session.frame_paths.len(), 2);
    }

    #[test]
    fn test_session_with_reference_frame() {
        let session = Session::new("test").with_reference_frame(5);
        assert_eq!(session.reference_frame, Some(5));
    }

    #[test]
    fn test_session_frame_count() {
        let paths = [PathBuf::from("/a.fits"), PathBuf::from("/b.fits")];
        let session = Session::new("test").with_frames(&paths);
        assert_eq!(session.frame_count(), 2);
    }

    #[test]
    fn test_session_select_best_reference() {
        let frame_qualities = vec![
            FrameQuality {
                snr: 50.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                noise: 0.02,
                star_count: 50,
            },
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
            FrameQuality {
                snr: 30.0,
                fwhm: 4.0,
                eccentricity: 0.3,
                noise: 0.03,
                star_count: 30,
            },
        ];

        let paths = [
            PathBuf::from("/a.fits"),
            PathBuf::from("/b.fits"),
            PathBuf::from("/c.fits"),
        ];

        let quality = SessionQuality::from_frame_qualities(&frame_qualities);
        let mut session = Session {
            id: "test".into(),
            frame_paths: paths.to_vec(),
            quality,
            reference_frame: None,
        };

        let best = session.select_best_reference();
        assert_eq!(best, Some(1)); // Frame 1 has best quality
        assert_eq!(session.reference_frame, Some(1));
    }

    // ========== SessionConfig Tests ==========

    #[test]
    fn test_session_config_default() {
        let config = SessionConfig::default();
        assert!((config.quality_threshold - 0.5).abs() < f32::EPSILON);
        assert!(config.use_session_weights);
        assert!(config.use_local_normalization);
        assert_eq!(config.normalization_tile_size, 128);
    }

    #[test]
    fn test_session_config_with_quality_threshold() {
        let config = SessionConfig::default().with_quality_threshold(0.3);
        assert!((config.quality_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "Quality threshold must be between")]
    fn test_session_config_invalid_threshold_panics() {
        SessionConfig::default().with_quality_threshold(1.5);
    }

    #[test]
    fn test_session_config_without_session_weights() {
        let config = SessionConfig::default().without_session_weights();
        assert!(!config.use_session_weights);
    }

    #[test]
    fn test_session_config_without_local_normalization() {
        let config = SessionConfig::default().without_local_normalization();
        assert!(!config.use_local_normalization);
    }

    #[test]
    fn test_session_config_with_normalization_tile_size() {
        let config = SessionConfig::default().with_normalization_tile_size(64);
        assert_eq!(config.normalization_tile_size, 64);
    }

    #[test]
    #[should_panic(expected = "Tile size must be at least 32")]
    fn test_session_config_tile_size_too_small_panics() {
        SessionConfig::default().with_normalization_tile_size(16);
    }

    // ========== MultiSessionStack Tests ==========

    #[test]
    fn test_multi_session_stack_new() {
        let sessions = vec![Session::new("s1"), Session::new("s2")];
        let stack = MultiSessionStack::new(sessions);
        assert_eq!(stack.sessions.len(), 2);
    }

    #[test]
    fn test_multi_session_stack_with_config() {
        let stack = MultiSessionStack::new(vec![])
            .with_config(SessionConfig::default().without_session_weights());
        assert!(!stack.config.use_session_weights);
    }

    #[test]
    fn test_multi_session_stack_total_frame_count() {
        let paths1 = [PathBuf::from("/a.fits"), PathBuf::from("/b.fits")];
        let paths2 = [
            PathBuf::from("/c.fits"),
            PathBuf::from("/d.fits"),
            PathBuf::from("/e.fits"),
        ];

        let sessions = vec![
            Session::new("s1").with_frames(&paths1),
            Session::new("s2").with_frames(&paths2),
        ];

        let stack = MultiSessionStack::new(sessions);
        assert_eq!(stack.total_frame_count(), 5);
    }

    #[test]
    fn test_multi_session_stack_compute_session_weights_equal() {
        let sessions = vec![Session::new("s1"), Session::new("s2")];
        let stack = MultiSessionStack::new(sessions)
            .with_config(SessionConfig::default().without_session_weights());

        let weights = stack.compute_session_weights();
        assert_eq!(weights.len(), 2);
        assert!((weights[0] - 0.5).abs() < f32::EPSILON);
        assert!((weights[1] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_multi_session_stack_compute_session_weights_quality_based() {
        let frame_qualities1 = vec![FrameQuality {
            snr: 100.0,
            fwhm: 2.0,
            eccentricity: 0.1,
            noise: 0.01,
            star_count: 100,
        }];

        let frame_qualities2 = vec![FrameQuality {
            snr: 50.0,
            fwhm: 3.0,
            eccentricity: 0.2,
            noise: 0.02,
            star_count: 50,
        }];

        let sessions = vec![
            Session {
                id: "good".into(),
                frame_paths: vec![PathBuf::from("/a.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities1),
                reference_frame: None,
            },
            Session {
                id: "worse".into(),
                frame_paths: vec![PathBuf::from("/b.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities2),
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);
        let weights = stack.compute_session_weights();

        // Good session should have higher weight
        assert!(weights[0] > weights[1]);
        // Weights should sum to 1
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_multi_session_stack_all_frame_paths() {
        let paths1 = [PathBuf::from("/a.fits"), PathBuf::from("/b.fits")];
        let paths2 = [PathBuf::from("/c.fits")];

        let sessions = vec![
            Session::new("s1").with_frames(&paths1),
            Session::new("s2").with_frames(&paths2),
        ];

        let stack = MultiSessionStack::new(sessions);
        let all_paths = stack.all_frame_paths();

        assert_eq!(all_paths.len(), 3);
    }

    #[test]
    fn test_multi_session_stack_summary() {
        let frame_qualities = vec![FrameQuality {
            snr: 50.0,
            fwhm: 2.5,
            eccentricity: 0.15,
            noise: 0.01,
            star_count: 100,
        }];

        let sessions = vec![Session {
            id: "2024-01-15".into(),
            frame_paths: vec![PathBuf::from("/a.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let summary = stack.summary();

        assert_eq!(summary.total_sessions, 1);
        assert_eq!(summary.total_frames, 1);
        assert_eq!(summary.sessions[0].id, "2024-01-15");
        assert!((summary.sessions[0].median_fwhm - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_multi_session_summary_display() {
        let frame_qualities = vec![FrameQuality {
            snr: 50.0,
            fwhm: 2.5,
            eccentricity: 0.15,
            noise: 0.01,
            star_count: 100,
        }];

        let sessions = vec![Session {
            id: "night1".into(),
            frame_paths: vec![PathBuf::from("/a.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let summary = stack.summary();
        let display = format!("{}", summary);

        assert!(display.contains("Multi-Session Stack Summary"));
        assert!(display.contains("night1"));
        assert!(display.contains("1 sessions"));
    }

    #[test]
    fn test_multi_session_stack_compute_frame_weights() {
        let frame_qualities1 = vec![
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
            FrameQuality {
                snr: 80.0,
                fwhm: 2.2,
                eccentricity: 0.12,
                noise: 0.012,
                star_count: 80,
            },
        ];

        let frame_qualities2 = vec![FrameQuality {
            snr: 50.0,
            fwhm: 3.0,
            eccentricity: 0.2,
            noise: 0.02,
            star_count: 50,
        }];

        let sessions = vec![
            Session {
                id: "s1".into(),
                frame_paths: vec![PathBuf::from("/a.fits"), PathBuf::from("/b.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities1),
                reference_frame: None,
            },
            Session {
                id: "s2".into(),
                frame_paths: vec![PathBuf::from("/c.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities2),
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);
        let weights = stack.compute_frame_weights();

        // Should have 3 weights (2 + 1 frames)
        assert_eq!(weights.len(), 3);

        // Weights should sum to 1
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);

        // All weights should be positive
        assert!(weights.iter().all(|&w| w > 0.0));
    }

    // ========== SessionNormalization Tests ==========

    #[test]
    fn test_session_normalization_new() {
        let reference = patterns::uniform(256, 256, 100.0);
        let config = LocalNormalizationConfig::new(64);

        let normalizer = SessionNormalization::new(&reference, config);

        assert_eq!(normalizer.dimensions(), (256, 256));
        assert_eq!(normalizer.config().tile_size, 64);
    }

    #[test]
    fn test_session_normalization_from_stats() {
        let reference = patterns::uniform(256, 256, 100.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&reference, &config);
        let normalizer = SessionNormalization::from_stats(stats.clone(), config.clone());

        assert_eq!(normalizer.dimensions(), (256, 256));
        assert_eq!(normalizer.reference_stats().tiles_x, stats.tiles_x);
    }

    #[test]
    fn test_session_normalization_normalize_frame() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::uniform(256, 128, 50.0); // 50 units darker
        let config = LocalNormalizationConfig::new(64);

        let normalizer = SessionNormalization::new(&reference, config);
        let normalized = normalizer.normalize_frame(&target);

        // Normalized values should be close to reference
        let avg: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (avg - 100.0).abs() < 1.0,
            "Normalized average should be ~100, got {}",
            avg
        );
    }

    #[test]
    fn test_session_normalization_normalize_frame_in_place() {
        let reference = patterns::uniform(256, 128, 100.0);
        let mut target = patterns::uniform(256, 128, 80.0);
        let config = LocalNormalizationConfig::new(64);

        let normalizer = SessionNormalization::new(&reference, config);

        // Compare in-place with new version
        let normalized_new = normalizer.normalize_frame(&target);
        normalizer.normalize_frame_in_place(&mut target);

        for (a, b) in target.iter().zip(normalized_new.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "In-place and new should give same result"
            );
        }
    }

    #[test]
    fn test_session_normalization_gradient_correction() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::horizontal_gradient(256, 128, 80.0, 120.0); // Gradient
        let config = LocalNormalizationConfig::new(64);

        let normalizer = SessionNormalization::new(&reference, config);
        let normalized = normalizer.normalize_frame(&target);

        // The gradient should be significantly reduced
        // Compute variance before and after
        let target_variance = compute_variance(&target);
        let normalized_variance = compute_variance(&normalized);

        assert!(
            normalized_variance < target_variance,
            "Normalized variance ({}) should be less than target variance ({})",
            normalized_variance,
            target_variance
        );
    }

    fn compute_variance(pixels: &[f32]) -> f32 {
        let n = pixels.len() as f32;
        let mean = pixels.iter().sum::<f32>() / n;
        pixels.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n
    }

    #[test]
    fn test_session_normalization_compute_map() {
        let reference = patterns::uniform(128, 128, 100.0);
        let target = patterns::uniform(128, 128, 50.0);
        let config = LocalNormalizationConfig::new(64);

        let normalizer = SessionNormalization::new(&reference, config);
        let map = normalizer.compute_map(&target);

        // Apply map should give same result as normalize_frame
        let normalized_direct = normalizer.normalize_frame(&target);
        let normalized_map = map.apply_to_new(&target);

        for (a, b) in normalized_direct.iter().zip(normalized_map.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "Direct and map-based normalization should match"
            );
        }
    }

    // ========== MultiSessionStack Reference Selection Tests ==========

    #[test]
    fn test_multi_session_stack_select_best_session() {
        let frame_qualities1 = vec![FrameQuality {
            snr: 100.0,
            fwhm: 2.0,
            eccentricity: 0.1,
            noise: 0.01,
            star_count: 100,
        }];

        let frame_qualities2 = vec![FrameQuality {
            snr: 50.0,
            fwhm: 3.0,
            eccentricity: 0.2,
            noise: 0.02,
            star_count: 50,
        }];

        let sessions = vec![
            Session {
                id: "good".into(),
                frame_paths: vec![PathBuf::from("/a.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities1),
                reference_frame: None,
            },
            Session {
                id: "worse".into(),
                frame_paths: vec![PathBuf::from("/b.fits")],
                quality: SessionQuality::from_frame_qualities(&frame_qualities2),
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);
        let best_idx = stack.select_best_session();

        assert_eq!(best_idx, Some(0)); // First session is better
    }

    #[test]
    fn test_multi_session_stack_select_best_session_empty() {
        let stack = MultiSessionStack::new(vec![]);
        assert_eq!(stack.select_best_session(), None);
    }

    #[test]
    fn test_multi_session_stack_select_global_reference() {
        let frame_qualities = vec![
            FrameQuality {
                snr: 50.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                noise: 0.02,
                star_count: 50,
            },
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
        ];

        let sessions = vec![Session {
            id: "test".into(),
            frame_paths: vec![PathBuf::from("/a.fits"), PathBuf::from("/b.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let (session_idx, frame_idx) = stack.select_global_reference().unwrap();

        assert_eq!(session_idx, 0);
        assert_eq!(frame_idx, 1); // Second frame has better quality
    }

    #[test]
    fn test_multi_session_stack_select_global_reference_no_quality() {
        let sessions = vec![Session {
            id: "test".into(),
            frame_paths: vec![PathBuf::from("/a.fits"), PathBuf::from("/b.fits")],
            quality: SessionQuality::default(), // No frame qualities
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let (session_idx, frame_idx) = stack.select_global_reference().unwrap();

        // Falls back to first frame
        assert_eq!(session_idx, 0);
        assert_eq!(frame_idx, 0);
    }

    #[test]
    fn test_multi_session_stack_global_reference_path() {
        let frame_qualities = vec![FrameQuality {
            snr: 100.0,
            fwhm: 2.0,
            eccentricity: 0.1,
            noise: 0.01,
            star_count: 100,
        }];

        let sessions = vec![Session {
            id: "test".into(),
            frame_paths: vec![PathBuf::from("/best_frame.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let path = stack.global_reference_path().unwrap();

        assert_eq!(path, Path::new("/best_frame.fits"));
    }

    #[test]
    fn test_multi_session_stack_global_reference_info() {
        let frame_qualities = vec![
            FrameQuality {
                snr: 50.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                noise: 0.02,
                star_count: 50,
            },
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 0.1,
                noise: 0.01,
                star_count: 100,
            },
        ];

        let sessions = vec![Session {
            id: "night1".into(),
            frame_paths: vec![PathBuf::from("/a.fits"), PathBuf::from("/b.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);
        let info = stack.global_reference_info().unwrap();

        assert_eq!(info.session_idx, 0);
        assert_eq!(info.frame_idx, 1); // Best frame
        assert_eq!(info.session_id, "night1");
        assert_eq!(info.path, PathBuf::from("/b.fits"));
    }

    #[test]
    fn test_multi_session_stack_create_normalizer_from_pixels() {
        let frame_qualities = vec![FrameQuality {
            snr: 100.0,
            fwhm: 2.0,
            eccentricity: 0.1,
            noise: 0.01,
            star_count: 100,
        }];

        let sessions = vec![Session {
            id: "test".into(),
            frame_paths: vec![PathBuf::from("/a.fits")],
            quality: SessionQuality::from_frame_qualities(&frame_qualities),
            reference_frame: None,
        }];

        let stack = MultiSessionStack::new(sessions);

        let reference_pixels = patterns::uniform(256, 256, 100.0);

        let normalizer = stack.create_session_normalizer_from_pixels(&reference_pixels);

        assert_eq!(normalizer.dimensions(), (256, 256));
        assert_eq!(normalizer.config().tile_size, 128); // Default from SessionConfig
    }

    #[test]
    fn test_multi_session_stack_normalizer_with_custom_tile_size() {
        let sessions = vec![Session::new("test")];

        let config = SessionConfig::default().with_normalization_tile_size(64);
        let stack = MultiSessionStack::new(sessions).with_config(config);

        let reference_pixels = patterns::uniform(256, 256, 100.0);

        let normalizer = stack.create_session_normalizer_from_pixels(&reference_pixels);

        assert_eq!(normalizer.config().tile_size, 64);
    }

    #[test]
    fn test_session_normalization_cross_session() {
        // Simulate normalizing frames from two different sessions to a common reference
        let config = LocalNormalizationConfig::new(64);

        // Reference frame from "good" session
        let reference = patterns::uniform(256, 128, 100.0);

        // Frame from session 1: uniform but darker
        let session1_frame = patterns::uniform(256, 128, 70.0);

        // Frame from session 2: gradient (different light pollution)
        let session2_frame = patterns::horizontal_gradient(256, 128, 80.0, 120.0);

        let normalizer = SessionNormalization::new(&reference, config);

        let normalized1 = normalizer.normalize_frame(&session1_frame);
        let normalized2 = normalizer.normalize_frame(&session2_frame);

        // Both should be close to reference level
        let avg1: f32 = normalized1.iter().sum::<f32>() / normalized1.len() as f32;
        let avg2: f32 = normalized2.iter().sum::<f32>() / normalized2.len() as f32;

        assert!(
            (avg1 - 100.0).abs() < 1.0,
            "Session 1 average should be ~100, got {}",
            avg1
        );
        assert!(
            (avg2 - 100.0).abs() < 2.0,
            "Session 2 average should be ~100, got {}",
            avg2
        );
    }

    // ========== SessionWeightedStackResult Tests ==========

    #[test]
    fn test_session_weighted_stack_result_display() {
        use crate::AstroImage;

        let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![0.5f32; 100]);
        let result = SessionWeightedStackResult {
            image,
            session_count: 2,
            total_frames: 10,
            session_weights: vec![0.6, 0.4],
            frame_weights: vec![0.1; 10],
            reference_info: Some(GlobalReferenceInfo {
                session_idx: 0,
                frame_idx: 2,
                session_id: "night1".into(),
                path: PathBuf::from("/ref.fits"),
            }),
            used_normalization: true,
        };

        let display = format!("{}", result);
        assert!(display.contains("Session-Weighted Stack Result"));
        assert!(display.contains("Sessions: 2"));
        assert!(display.contains("Total frames: 10"));
        assert!(display.contains("Used normalization: yes"));
        assert!(display.contains("night1"));
    }

    #[test]
    fn test_session_weighted_stack_result_into_image() {
        use crate::AstroImage;

        let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![0.5f32; 100]);
        let result = SessionWeightedStackResult {
            image,
            session_count: 1,
            total_frames: 5,
            session_weights: vec![1.0],
            frame_weights: vec![0.2; 5],
            reference_info: None,
            used_normalization: false,
        };

        let img = result.into_image();
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
    }

    #[test]
    fn test_session_weighted_stack_result_session_contributions() {
        use crate::AstroImage;

        let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![0.5f32; 100]);

        // Create matching sessions
        let sessions = vec![
            Session {
                id: "s1".into(),
                frame_paths: vec![PathBuf::from("/a.fits"), PathBuf::from("/b.fits")],
                quality: SessionQuality::default(),
                reference_frame: None,
            },
            Session {
                id: "s2".into(),
                frame_paths: vec![
                    PathBuf::from("/c.fits"),
                    PathBuf::from("/d.fits"),
                    PathBuf::from("/e.fits"),
                ],
                quality: SessionQuality::default(),
                reference_frame: None,
            },
        ];
        let stack = MultiSessionStack::new(sessions);

        // Frame weights: session 1 has weight 0.1+0.1=0.2, session 2 has 0.2+0.2+0.2=0.6
        let result = SessionWeightedStackResult {
            image,
            session_count: 2,
            total_frames: 5,
            session_weights: vec![0.25, 0.75],
            frame_weights: vec![0.1, 0.1, 0.2, 0.2, 0.2],
            reference_info: None,
            used_normalization: false,
        };

        let contributions = result.session_contributions(&stack);
        assert_eq!(contributions.len(), 2);
        assert_eq!(contributions[0].0, 0); // Session 0
        assert!((contributions[0].1 - 0.2).abs() < f32::EPSILON); // 0.1 + 0.1
        assert_eq!(contributions[1].0, 1); // Session 1
        assert!((contributions[1].1 - 0.6).abs() < f32::EPSILON); // 0.2 + 0.2 + 0.2
    }

    #[test]
    fn test_stack_session_weighted_no_frames_error() {
        let stack = MultiSessionStack::new(vec![]);
        let result = stack.stack_session_weighted();

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No frames to stack"));
    }

    #[test]
    fn test_stack_session_weighted_empty_session_error() {
        let sessions = vec![Session::new("empty")]; // No frames
        let stack = MultiSessionStack::new(sessions);
        let result = stack.stack_session_weighted();

        assert!(result.is_err());
    }
}

/// Integration tests for multi-session stacking with synthetic data.
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::AstroImage;
    use crate::testing::synthetic::patterns;
    use imaginarium::Image;
    use tempfile::TempDir;

    // ============================================================================
    // Test Helpers
    // ============================================================================

    /// Render a Gaussian star at the given position.
    fn render_gaussian(
        pixels: &mut [f32],
        width: usize,
        cx: f32,
        cy: f32,
        sigma: f32,
        amplitude: f32,
    ) {
        let height = pixels.len() / width;
        let radius = (sigma * 4.0).ceil() as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let dist_sq = (dx as f32).powi(2) + (dy as f32).powi(2);
                    let gauss = amplitude * (-dist_sq / (2.0 * sigma * sigma)).exp();
                    pixels[y * width + x] += gauss;
                }
            }
        }
    }

    /// Create an AstroImage with random-ish synthetic star field.
    fn create_synthetic_frame(
        width: usize,
        height: usize,
        background: f32,
        noise: f32,
        star_positions: &[(f32, f32)],
        star_brightness: f32,
    ) -> AstroImage {
        let mut pixels = vec![background; width * height];

        // Add some pseudo-random noise based on position
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                // Simple deterministic "noise" based on position
                let noise_val = ((x * 7 + y * 13) % 100) as f32 / 100.0 - 0.5;
                pixels[idx] += noise * noise_val;
            }
        }

        // Render stars
        for &(cx, cy) in star_positions {
            render_gaussian(&mut pixels, width, cx, cy, 2.5, star_brightness);
        }

        // Clamp to positive values
        for p in &mut pixels {
            *p = p.max(0.0);
        }

        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels)
    }

    /// Save an AstroImage to a temporary TIFF file (as 32-bit float).
    fn save_test_image(image: &AstroImage, dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        // Save as L_F32 TIFF - this preserves the float values
        let img: Image = image.clone().into();
        img.save_file(&path).expect("Failed to save test image");
        path
    }

    /// Compute variance of a pixel array.
    fn compute_variance(pixels: &[f32]) -> f32 {
        let n = pixels.len() as f32;
        let mean = pixels.iter().sum::<f32>() / n;
        pixels.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n
    }

    // ============================================================================
    // Integration Tests
    // ============================================================================

    #[test]
    fn test_integration_multi_session_stacking_synthetic_data() {
        // Create temporary directory for test images
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let width = 64;
        let height = 64;
        let star_positions = vec![(20.0, 20.0), (40.0, 30.0), (15.0, 45.0), (50.0, 50.0)];

        // Session 1: Good seeing (low FWHM), high SNR
        // Background level: 100, stars brighter
        let session1_frames: Vec<PathBuf> = (0..3)
            .map(|i| {
                let frame =
                    create_synthetic_frame(width, height, 100.0, 5.0, &star_positions, 500.0);
                save_test_image(&frame, &temp_dir, &format!("session1_frame{}.tiff", i))
            })
            .collect();

        // Session 2: Poorer conditions (different background)
        // Background level: 120 (more light pollution), slightly dimmer stars
        let session2_frames: Vec<PathBuf> = (0..2)
            .map(|i| {
                let frame =
                    create_synthetic_frame(width, height, 120.0, 8.0, &star_positions, 400.0);
                save_test_image(&frame, &temp_dir, &format!("session2_frame{}.tiff", i))
            })
            .collect();

        // Create sessions with manually set quality metrics
        // (In real usage, assess_quality would compute these from star detection)
        let quality1 = SessionQuality {
            median_fwhm: 2.5,
            median_snr: 100.0,
            median_eccentricity: 0.1,
            median_noise: 5.0,
            frame_count: 3,
            usable_frame_count: 3,
            frame_qualities: vec![
                FrameQuality {
                    fwhm: 2.4,
                    snr: 95.0,
                    eccentricity: 0.1,
                    noise: 5.0,
                    star_count: 4,
                },
                FrameQuality {
                    fwhm: 2.5,
                    snr: 100.0,
                    eccentricity: 0.1,
                    noise: 5.0,
                    star_count: 4,
                },
                FrameQuality {
                    fwhm: 2.6,
                    snr: 105.0,
                    eccentricity: 0.1,
                    noise: 5.0,
                    star_count: 4,
                },
            ],
        };

        let quality2 = SessionQuality {
            median_fwhm: 3.2,
            median_snr: 70.0,
            median_eccentricity: 0.15,
            median_noise: 8.0,
            frame_count: 2,
            usable_frame_count: 2,
            frame_qualities: vec![
                FrameQuality {
                    fwhm: 3.0,
                    snr: 65.0,
                    eccentricity: 0.15,
                    noise: 8.0,
                    star_count: 4,
                },
                FrameQuality {
                    fwhm: 3.4,
                    snr: 75.0,
                    eccentricity: 0.15,
                    noise: 8.0,
                    star_count: 4,
                },
            ],
        };

        let session1 = Session {
            id: "night1".into(),
            frame_paths: session1_frames,
            quality: quality1,
            reference_frame: Some(1),
        };

        let session2 = Session {
            id: "night2".into(),
            frame_paths: session2_frames,
            quality: quality2,
            reference_frame: Some(0),
        };

        // Configure stacking (disable local normalization since frames have same dimensions)
        let config = SessionConfig::default()
            .with_quality_threshold(0.3)
            .without_local_normalization(); // Disable to simplify test

        let stack = MultiSessionStack::new(vec![session1, session2]).with_config(config);

        // Verify session weights
        let session_weights = stack.compute_session_weights();
        assert_eq!(session_weights.len(), 2);
        // Session 1 should have higher weight (better quality)
        assert!(
            session_weights[0] > session_weights[1],
            "Session 1 (better quality) should have higher weight: {} vs {}",
            session_weights[0],
            session_weights[1]
        );

        // Verify frame weights
        let frame_weights = stack.compute_frame_weights();
        assert_eq!(frame_weights.len(), 5); // 3 + 2 frames
        let weight_sum: f32 = frame_weights.iter().sum();
        assert!(
            (weight_sum - 1.0).abs() < 1e-5,
            "Frame weights should sum to 1.0, got {}",
            weight_sum
        );

        // Perform stacking
        let result = stack
            .stack_session_weighted()
            .expect("Stacking should succeed");

        // Verify result
        assert_eq!(result.session_count, 2);
        assert_eq!(result.total_frames, 5);
        assert!(!result.used_normalization); // We disabled it
        assert_eq!(result.image.width(), width);
        assert_eq!(result.image.height(), height);

        // Verify the stacked image has reasonable values
        let stacked_pixels = result.image.clone().into_interleaved_pixels();
        let min_val = stacked_pixels.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_val = stacked_pixels
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        let mean_val: f32 = stacked_pixels.iter().sum::<f32>() / stacked_pixels.len() as f32;

        // Background should be weighted average (~100*0.6 + 120*0.4 = 108)
        // But weights are frame-based, so it depends on frame count too
        assert!(
            mean_val > 90.0 && mean_val < 130.0,
            "Mean background should be reasonable: {}",
            mean_val
        );
        assert!(
            max_val > mean_val + 100.0,
            "Max (star peak) should be significantly above mean: max={}, mean={}",
            max_val,
            mean_val
        );
        assert!(
            min_val >= 0.0,
            "Min value should be non-negative: {}",
            min_val
        );

        // Clean up happens automatically via TempDir Drop
    }

    #[test]
    fn test_integration_session_normalization_corrects_gradient() {
        let width = 128;
        let height = 128;

        // Reference frame: uniform background at 100
        let reference_pixels = patterns::uniform(width, height, 100.0);

        // Target frame: has a gradient (simulating light pollution gradient)
        let target_pixels = patterns::horizontal_gradient(width, height, 80.0, 120.0);

        // Before normalization: target has high variance (gradient)
        let target_variance_before = compute_variance(&target_pixels);

        // Create normalizer and normalize
        let config = LocalNormalizationConfig::new(64); // Minimum tile size is 64
        let normalizer = SessionNormalization::new(&reference_pixels, config);
        let normalized = normalizer.normalize_frame(&target_pixels);

        // After normalization: variance should be much lower (gradient removed)
        let target_variance_after = compute_variance(&normalized);

        assert!(
            target_variance_after < target_variance_before * 0.1,
            "Normalization should reduce variance by >90%: before={}, after={}",
            target_variance_before,
            target_variance_after
        );

        // Normalized mean should be close to reference
        let normalized_mean: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (normalized_mean - 100.0).abs() < 2.0,
            "Normalized mean should be ~100: {}",
            normalized_mean
        );
    }

    #[test]
    fn test_integration_gradient_removal_on_stacked_result() {
        // Create a stacked result with a gradient
        let width = 128;
        let height = 128;

        // Create image with linear gradient (simulating residual light pollution)
        // Use a stronger gradient for clearer effect
        let mut pixels = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let gradient = 100.0 + (x as f32) * 1.0 + (y as f32) * 0.8;
                pixels[y * width + x] = gradient;
            }
        }

        // Add some smaller stars that won't dominate the sample rejection
        render_gaussian(&mut pixels, width, 30.0, 30.0, 2.0, 50.0);
        render_gaussian(&mut pixels, width, 90.0, 80.0, 2.0, 40.0);

        let variance_before = compute_variance(&pixels);

        let image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels);

        let mut result = SessionWeightedStackResult {
            image,
            session_count: 1,
            total_frames: 5,
            session_weights: vec![1.0],
            frame_weights: vec![0.2; 5],
            reference_info: None,
            used_normalization: false,
        };

        // Apply gradient removal with denser sampling
        let config = super::super::gradient_removal::GradientRemovalConfig::polynomial(1)
            .with_samples_per_line(16)
            .with_min_samples(16)
            .with_brightness_tolerance(5.0); // More tolerant of stars

        result
            .remove_gradient(&config)
            .expect("Gradient removal should succeed");

        // After gradient removal, variance should be much lower
        let variance_after = compute_variance(&result.image.clone().into_interleaved_pixels());

        // The linear polynomial should remove most of the linear gradient
        // Stars will add some residual variance, so we expect ~90% reduction
        assert!(
            variance_after < variance_before * 0.2,
            "Gradient removal should reduce variance significantly: before={}, after={}",
            variance_before,
            variance_after
        );
    }

    #[test]
    fn test_integration_cross_session_normalization() {
        // Simulate normalizing frames from two different sessions

        // Reference from "good" session: uniform, bright
        let reference = patterns::uniform(128, 128, 100.0);

        // Frame from session 1: uniform but darker (different sky)
        let session1_frame = patterns::uniform(128, 128, 70.0);

        // Frame from session 2: has gradient (different light pollution direction)
        let session2_frame = patterns::horizontal_gradient(128, 128, 85.0, 115.0);

        let config = LocalNormalizationConfig::new(64);
        let normalizer = SessionNormalization::new(&reference, config);

        let normalized1 = normalizer.normalize_frame(&session1_frame);
        let normalized2 = normalizer.normalize_frame(&session2_frame);

        // Both should be close to reference level (100)
        let avg1: f32 = normalized1.iter().sum::<f32>() / normalized1.len() as f32;
        let avg2: f32 = normalized2.iter().sum::<f32>() / normalized2.len() as f32;

        assert!(
            (avg1 - 100.0).abs() < 2.0,
            "Session 1 average should be ~100, got {}",
            avg1
        );
        assert!(
            (avg2 - 100.0).abs() < 2.0,
            "Session 2 average should be ~100, got {}",
            avg2
        );

        // Session 2 should have reduced variance after normalization
        let session2_variance_before = compute_variance(&session2_frame);
        let session2_variance_after = compute_variance(&normalized2);

        assert!(
            session2_variance_after < session2_variance_before * 0.5,
            "Session 2 gradient should be reduced: before={}, after={}",
            session2_variance_before,
            session2_variance_after
        );
    }

    #[test]
    fn test_integration_global_reference_selection() {
        // Create sessions with different quality levels
        let quality_good = SessionQuality {
            median_fwhm: 2.0,
            median_snr: 150.0,
            median_eccentricity: 0.08,
            median_noise: 3.0,
            frame_count: 5,
            usable_frame_count: 5,
            frame_qualities: vec![
                FrameQuality {
                    fwhm: 2.2,
                    snr: 140.0,
                    eccentricity: 0.12,
                    noise: 3.5,
                    star_count: 50,
                },
                FrameQuality {
                    fwhm: 1.8,
                    snr: 180.0,
                    eccentricity: 0.06,
                    noise: 2.2,
                    star_count: 60,
                }, // Best frame: lowest FWHM, highest SNR, lowest ecc, lowest noise
                FrameQuality {
                    fwhm: 2.2,
                    snr: 150.0,
                    eccentricity: 0.09,
                    noise: 3.2,
                    star_count: 48,
                },
                FrameQuality {
                    fwhm: 2.1,
                    snr: 145.0,
                    eccentricity: 0.08,
                    noise: 3.1,
                    star_count: 52,
                },
                FrameQuality {
                    fwhm: 2.3,
                    snr: 155.0,
                    eccentricity: 0.10,
                    noise: 3.0,
                    star_count: 50,
                },
            ],
        };

        let quality_poor = SessionQuality {
            median_fwhm: 4.0,
            median_snr: 50.0,
            median_eccentricity: 0.25,
            median_noise: 10.0,
            frame_count: 3,
            usable_frame_count: 3,
            frame_qualities: vec![
                FrameQuality {
                    fwhm: 3.8,
                    snr: 45.0,
                    eccentricity: 0.25,
                    noise: 10.0,
                    star_count: 20,
                },
                FrameQuality {
                    fwhm: 4.0,
                    snr: 55.0,
                    eccentricity: 0.24,
                    noise: 9.5,
                    star_count: 22,
                },
                FrameQuality {
                    fwhm: 4.2,
                    snr: 50.0,
                    eccentricity: 0.26,
                    noise: 10.5,
                    star_count: 18,
                },
            ],
        };

        let sessions = vec![
            Session {
                id: "poor_night".into(),
                frame_paths: vec![
                    PathBuf::from("/poor_a.fits"),
                    PathBuf::from("/poor_b.fits"),
                    PathBuf::from("/poor_c.fits"),
                ],
                quality: quality_poor,
                reference_frame: None,
            },
            Session {
                id: "good_night".into(),
                frame_paths: vec![
                    PathBuf::from("/good_a.fits"),
                    PathBuf::from("/good_b.fits"),
                    PathBuf::from("/good_c.fits"),
                    PathBuf::from("/good_d.fits"),
                    PathBuf::from("/good_e.fits"),
                ],
                quality: quality_good,
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);

        // Should select the good session
        let best_session = stack.select_best_session();
        assert_eq!(best_session, Some(1), "Should select the good session");

        // Should select the best frame within the good session
        let (session_idx, frame_idx) = stack.select_global_reference().unwrap();
        assert_eq!(session_idx, 1, "Reference should be from good session");
        assert_eq!(
            frame_idx, 1,
            "Should select frame 1 (best quality in good session)"
        );

        // Verify reference info
        let info = stack.global_reference_info().unwrap();
        assert_eq!(info.session_id, "good_night");
        assert_eq!(info.frame_idx, 1);
        assert_eq!(info.path, PathBuf::from("/good_b.fits"));
    }

    #[test]
    fn test_integration_summary_display_format() {
        // Create sessions and verify summary display
        let quality = SessionQuality {
            median_fwhm: 2.5,
            median_snr: 100.0,
            median_eccentricity: 0.12,
            median_noise: 5.0,
            frame_count: 10,
            usable_frame_count: 8,
            frame_qualities: vec![],
        };

        let sessions = vec![
            Session {
                id: "2024-01-15".into(),
                frame_paths: (0..10)
                    .map(|i| PathBuf::from(format!("/n1/{}.fits", i)))
                    .collect(),
                quality: quality.clone(),
                reference_frame: None,
            },
            Session {
                id: "2024-01-16".into(),
                frame_paths: (0..5)
                    .map(|i| PathBuf::from(format!("/n2/{}.fits", i)))
                    .collect(),
                quality: SessionQuality {
                    frame_count: 5,
                    usable_frame_count: 5,
                    ..quality
                },
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);
        let summary = stack.summary();

        // Test summary values
        assert_eq!(summary.total_sessions, 2);
        assert_eq!(summary.total_frames, 15);

        // Test Display implementation
        let display = format!("{}", summary);
        assert!(
            display.contains("Multi-Session Stack Summary"),
            "Display should contain header"
        );
        assert!(
            display.contains("2024-01-15"),
            "Display should show session 1 ID"
        );
        assert!(
            display.contains("2024-01-16"),
            "Display should show session 2 ID"
        );
        assert!(
            display.contains("2 sessions"),
            "Display should show session count"
        );
    }

    #[test]
    fn test_integration_quality_based_frame_filtering() {
        // Create a session with varied quality frames
        let frame_qualities = vec![
            // Excellent frame
            FrameQuality {
                fwhm: 2.0,
                snr: 200.0,
                eccentricity: 0.05,
                noise: 2.0,
                star_count: 100,
            },
            // Good frame
            FrameQuality {
                fwhm: 2.5,
                snr: 150.0,
                eccentricity: 0.08,
                noise: 3.0,
                star_count: 90,
            },
            // Average frame
            FrameQuality {
                fwhm: 3.0,
                snr: 100.0,
                eccentricity: 0.12,
                noise: 5.0,
                star_count: 70,
            },
            // Poor frame
            FrameQuality {
                fwhm: 5.0,
                snr: 30.0,
                eccentricity: 0.30,
                noise: 15.0,
                star_count: 20,
            },
            // Very poor frame
            FrameQuality {
                fwhm: 7.0,
                snr: 15.0,
                eccentricity: 0.50,
                noise: 25.0,
                star_count: 5,
            },
        ];

        let mut quality = SessionQuality::from_frame_qualities(&frame_qualities);
        assert_eq!(quality.usable_frame_count, 5);

        // Filter with 50% threshold (should remove worst frames)
        let passing = quality.filter_by_threshold(0.5);

        // Best 3 should pass (threshold relative to median weight)
        assert!(
            passing.len() >= 2 && passing.len() <= 4,
            "Should filter out worst frames"
        );
        assert!(
            quality.usable_frame_count < 5,
            "Some frames should be rejected"
        );

        // Frame 0 (excellent) should always pass
        assert!(passing.contains(&0), "Excellent frame should pass");

        // Frame 4 (very poor) should typically be rejected
        assert!(!passing.contains(&4), "Very poor frame should be rejected");
    }

    #[test]
    fn test_integration_session_weighted_stack_result_display() {
        let image = AstroImage::from_pixels(ImageDimensions::new(100, 100, 1), vec![0.5f32; 10000]);

        let result = SessionWeightedStackResult {
            image,
            session_count: 3,
            total_frames: 25,
            session_weights: vec![0.5, 0.3, 0.2],
            frame_weights: vec![0.04; 25],
            reference_info: Some(GlobalReferenceInfo {
                session_idx: 0,
                frame_idx: 5,
                session_id: "best_night".into(),
                path: PathBuf::from("/data/best_frame.fits"),
            }),
            used_normalization: true,
        };

        let display = format!("{}", result);

        assert!(display.contains("Session-Weighted Stack Result"));
        assert!(display.contains("100x100"));
        assert!(display.contains("Sessions: 3"));
        assert!(display.contains("Total frames: 25"));
        assert!(display.contains("Used normalization: yes"));
        assert!(display.contains("best_night"));
        assert!(display.contains("frame 5"));
    }

    #[test]
    fn test_integration_frame_weights_sum_to_one() {
        // Create multiple sessions with varied frame counts and quality
        let sessions = vec![
            Session {
                id: "s1".into(),
                frame_paths: vec![
                    PathBuf::from("/a.fits"),
                    PathBuf::from("/b.fits"),
                    PathBuf::from("/c.fits"),
                ],
                quality: SessionQuality {
                    median_fwhm: 2.5,
                    median_snr: 100.0,
                    median_eccentricity: 0.1,
                    median_noise: 5.0,
                    frame_count: 3,
                    usable_frame_count: 3,
                    frame_qualities: vec![
                        FrameQuality {
                            fwhm: 2.4,
                            snr: 95.0,
                            eccentricity: 0.1,
                            noise: 5.0,
                            star_count: 50,
                        },
                        FrameQuality {
                            fwhm: 2.5,
                            snr: 100.0,
                            eccentricity: 0.1,
                            noise: 5.0,
                            star_count: 55,
                        },
                        FrameQuality {
                            fwhm: 2.6,
                            snr: 105.0,
                            eccentricity: 0.1,
                            noise: 5.0,
                            star_count: 52,
                        },
                    ],
                },
                reference_frame: None,
            },
            Session {
                id: "s2".into(),
                frame_paths: vec![PathBuf::from("/d.fits"), PathBuf::from("/e.fits")],
                quality: SessionQuality {
                    median_fwhm: 3.0,
                    median_snr: 80.0,
                    median_eccentricity: 0.15,
                    median_noise: 7.0,
                    frame_count: 2,
                    usable_frame_count: 2,
                    frame_qualities: vec![
                        FrameQuality {
                            fwhm: 2.9,
                            snr: 78.0,
                            eccentricity: 0.15,
                            noise: 7.0,
                            star_count: 40,
                        },
                        FrameQuality {
                            fwhm: 3.1,
                            snr: 82.0,
                            eccentricity: 0.15,
                            noise: 7.0,
                            star_count: 42,
                        },
                    ],
                },
                reference_frame: None,
            },
        ];

        let stack = MultiSessionStack::new(sessions);

        // With session weights enabled
        let frame_weights = stack.compute_frame_weights();
        let sum: f32 = frame_weights.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "Frame weights should sum to 1.0, got {}",
            sum
        );
        assert_eq!(frame_weights.len(), 5);
        assert!(
            frame_weights.iter().all(|&w| w > 0.0),
            "All weights should be positive"
        );

        // With session weights disabled
        let stack_equal = stack.config.clone();
        let stack_equal = MultiSessionStack::new(stack.sessions.clone())
            .with_config(stack_equal.without_session_weights());
        let frame_weights_equal = stack_equal.compute_frame_weights();
        let sum_equal: f32 = frame_weights_equal.iter().sum();
        assert!(
            (sum_equal - 1.0).abs() < 1e-5,
            "Frame weights (equal sessions) should sum to 1.0, got {}",
            sum_equal
        );
    }
}
