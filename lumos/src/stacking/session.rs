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
use super::weighted::FrameQuality;
use crate::AstroImage;
use crate::star_detection::{StarDetectionConfig, StarDetectionResult, find_stars};

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
    /// * `config` - Star detection configuration
    ///
    /// # Returns
    /// * `Ok(Self)` - Session with quality metrics populated
    /// * `Err` - If frame loading or star detection fails
    pub fn assess_quality(mut self, config: &StarDetectionConfig) -> anyhow::Result<Self> {
        if self.frame_paths.is_empty() {
            return Ok(self);
        }

        let mut frame_qualities = Vec::with_capacity(self.frame_paths.len());

        for path in &self.frame_paths {
            let image = AstroImage::from_file(path)?;
            let result = find_stars(&image, config);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
