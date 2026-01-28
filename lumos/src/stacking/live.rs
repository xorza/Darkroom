//! Live stacking for real-time preview during imaging sessions.
//!
//! This module provides incremental stacking capabilities for real-time visualization
//! during astrophotography sessions. Frames can be added one at a time, and the
//! current stack can be previewed at any point.
//!
//! # Features
//!
//! - **Incremental accumulation**: Add frames one at a time with O(1) memory overhead
//! - **Quality-based weighting**: Optionally weight frames by SNR, FWHM, etc.
//! - **Per-pixel rejection**: Sigma clipping for outlier rejection (requires history)
//! - **Real-time preview**: Get current stack state at any time
//! - **Quality metrics**: Track overall stack quality metrics
//!
//! # Example
//!
//! ```ignore
//! use lumos::stacking::{LiveStackConfig, LiveStackAccumulator, LiveStackMode};
//!
//! // Create accumulator with running mean (minimal memory)
//! let config = LiveStackConfig {
//!     mode: LiveStackMode::RunningMean,
//!     ..Default::default()
//! };
//! let mut stack = LiveStackAccumulator::new(1024, 1024, 1, config);
//!
//! // Add frames as they arrive
//! for frame in incoming_frames {
//!     let quality = compute_frame_quality(&frame);
//!     stack.add_frame(frame, quality)?;
//!
//!     // Show preview to user
//!     let preview = stack.preview();
//!     display(&preview);
//! }
//!
//! // Get final result
//! let result = stack.finalize();
//! ```

use crate::ImageDimensions;
use crate::astro_image::AstroImage;
use std::fmt;

/// Configuration for live stacking.
#[derive(Debug, Clone)]
pub struct LiveStackConfig {
    /// Stacking mode (determines memory usage and capabilities)
    pub mode: LiveStackMode,

    /// Normalization strength (0.0 = none, 1.0 = full)
    /// When enabled, incoming frames are normalized to match the first frame's statistics
    pub normalize: bool,

    /// Target channel for preview auto-stretch (or None for all channels)
    pub preview_channel: Option<usize>,

    /// Whether to track variance for quality estimation
    pub track_variance: bool,
}

impl Default for LiveStackConfig {
    fn default() -> Self {
        Self {
            mode: LiveStackMode::RunningMean,
            normalize: true,
            preview_channel: None,
            track_variance: true,
        }
    }
}

impl LiveStackConfig {
    /// Create a new builder for LiveStackConfig.
    ///
    /// # Example
    /// ```rust,ignore
    /// use lumos::stacking::{LiveStackConfig, LiveStackConfigBuilder};
    ///
    /// let config = LiveStackConfig::builder()
    ///     .weighted_mean()
    ///     .normalize(true)
    ///     .track_variance(false)
    ///     .build();
    /// ```
    pub fn builder() -> LiveStackConfigBuilder {
        LiveStackConfigBuilder::new()
    }
}

/// Builder for `LiveStackConfig`.
///
/// Provides a fluent interface for constructing live stack configurations.
///
/// # Example
/// ```rust,ignore
/// use lumos::stacking::LiveStackConfig;
///
/// // Simple running mean
/// let config = LiveStackConfig::builder()
///     .running_mean()
///     .build();
///
/// // Weighted mean with normalization
/// let config = LiveStackConfig::builder()
///     .weighted_mean()
///     .normalize(true)
///     .build();
///
/// // Rolling sigma clip for satellite rejection
/// let config = LiveStackConfig::builder()
///     .rolling_sigma_clip(20, 2.5)
///     .track_variance(true)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct LiveStackConfigBuilder {
    mode: LiveStackMode,
    normalize: bool,
    preview_channel: Option<usize>,
    track_variance: bool,
}

impl Default for LiveStackConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveStackConfigBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        Self {
            mode: LiveStackMode::RunningMean,
            normalize: true,
            preview_channel: None,
            track_variance: true,
        }
    }

    /// Set the stacking mode to running mean.
    ///
    /// Running mean uses O(pixels) memory and provides no outlier rejection.
    /// Best for quick previews and memory-constrained situations.
    pub fn running_mean(mut self) -> Self {
        self.mode = LiveStackMode::RunningMean;
        self
    }

    /// Set the stacking mode to weighted mean.
    ///
    /// Weighted mean uses O(pixels) memory and weights frames by quality.
    /// Best for real-time sessions where frame quality varies.
    pub fn weighted_mean(mut self) -> Self {
        self.mode = LiveStackMode::WeightedMean;
        self
    }

    /// Set the stacking mode to rolling sigma clip.
    ///
    /// Rolling sigma clip keeps the last N frames for sigma-clipped averaging,
    /// using O(N × pixels) memory. Best for satellite/airplane rejection.
    ///
    /// # Arguments
    /// * `window_size` - Number of recent frames to keep (typical: 10-30, minimum: 3)
    /// * `sigma` - Sigma threshold for clipping (typical: 2.0-3.0)
    pub fn rolling_sigma_clip(mut self, window_size: usize, sigma: f32) -> Self {
        self.mode = LiveStackMode::RollingSigmaClip { window_size, sigma };
        self
    }

    /// Set whether to normalize incoming frames.
    ///
    /// When enabled (default: true), incoming frames are normalized to match
    /// the first frame's statistics (median and MAD).
    pub fn normalize(mut self, normalize: bool) -> Self {
        self.normalize = normalize;
        self
    }

    /// Set whether to track per-pixel variance.
    ///
    /// When enabled (default: true), variance is tracked for quality estimation.
    /// This adds slight memory overhead but provides useful statistics.
    pub fn track_variance(mut self, track_variance: bool) -> Self {
        self.track_variance = track_variance;
        self
    }

    /// Set the target channel for preview auto-stretch.
    ///
    /// When set, auto-stretch uses only this channel's histogram.
    /// When None (default), all channels are considered.
    pub fn preview_channel(mut self, channel: Option<usize>) -> Self {
        self.preview_channel = channel;
        self
    }

    /// Build the configuration.
    pub fn build(self) -> LiveStackConfig {
        LiveStackConfig {
            mode: self.mode,
            normalize: self.normalize,
            preview_channel: self.preview_channel,
            track_variance: self.track_variance,
        }
    }
}

/// Live stacking mode determining memory usage and capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LiveStackMode {
    /// Running mean: O(pixels) memory, no rejection possible
    /// Best for: Quick previews, memory-constrained situations
    #[default]
    RunningMean,

    /// Weighted running mean: O(pixels) memory, quality-weighted averaging
    /// Best for: Real-time sessions where frame quality varies
    WeightedMean,

    /// Keep last N frames for rolling sigma-clipped mean: O(N × pixels) memory
    /// Best for: Satellite/airplane rejection, high-quality final stacks
    RollingSigmaClip {
        /// Number of recent frames to keep (typical: 10-30)
        window_size: usize,
        /// Sigma threshold for clipping (typical: 2.0-3.0)
        sigma: f32,
    },
}

/// Quality metrics for a single frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveFrameQuality {
    /// Signal-to-noise ratio (higher = better)
    pub snr: f32,

    /// Full width at half maximum in pixels (lower = better seeing)
    pub fwhm: f32,

    /// Star eccentricity (1.0 = round, higher = elongated/trailing)
    pub eccentricity: f32,

    /// Number of detected stars
    pub star_count: usize,
}

impl LiveFrameQuality {
    /// Compute a weight from quality metrics.
    ///
    /// Higher SNR, lower FWHM, and rounder stars result in higher weight.
    /// Formula: (SNR × (1/FWHM)² × (1/eccentricity)) / noise
    pub fn compute_weight(&self) -> f32 {
        if self.fwhm <= 0.0 || self.eccentricity <= 0.0 {
            return 1.0; // Default weight if metrics are invalid
        }

        let fwhm_factor = 1.0 / (self.fwhm * self.fwhm);
        let ecc_factor = 1.0 / self.eccentricity;

        (self.snr * fwhm_factor * ecc_factor).max(0.001)
    }

    /// Create quality metrics indicating an unknown/default quality.
    pub fn unknown() -> Self {
        Self {
            snr: 1.0,
            fwhm: 2.0,
            eccentricity: 1.0,
            star_count: 0,
        }
    }
}

/// Accumulated statistics for the live stack.
#[derive(Debug, Clone)]
pub struct LiveStackStats {
    /// Number of frames integrated
    pub frame_count: usize,

    /// Total weight accumulated
    pub total_weight: f64,

    /// Average frame quality (weighted)
    pub mean_snr: f32,
    pub mean_fwhm: f32,
    pub mean_eccentricity: f32,

    /// Estimated stack SNR improvement factor (√N for equal weights)
    pub snr_improvement: f32,

    /// Per-pixel variance (if tracked)
    pub mean_variance: Option<f32>,
}

impl Default for LiveStackStats {
    fn default() -> Self {
        Self {
            frame_count: 0,
            total_weight: 0.0,
            mean_snr: 0.0,
            mean_fwhm: 0.0,
            mean_eccentricity: 0.0,
            snr_improvement: 1.0,
            mean_variance: None,
        }
    }
}

impl fmt::Display for LiveStackStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LiveStackStats {{ frames: {}, SNR×{:.1}, mean_fwhm: {:.2}, mean_ecc: {:.2} }}",
            self.frame_count, self.snr_improvement, self.mean_fwhm, self.mean_eccentricity
        )
    }
}

/// Error type for live stacking operations.
#[derive(Debug, Clone)]
pub enum LiveStackError {
    /// Frame dimensions don't match accumulator
    DimensionMismatch {
        expected: (usize, usize, usize),
        got: (usize, usize, usize),
    },
    /// No frames have been added yet
    NoFrames,
    /// Invalid configuration
    InvalidConfig(String),
}

impl fmt::Display for LiveStackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiveStackError::DimensionMismatch { expected, got } => {
                write!(
                    f,
                    "Frame dimensions mismatch: expected {}×{}×{}, got {}×{}×{}",
                    expected.0, expected.1, expected.2, got.0, got.1, got.2
                )
            }
            LiveStackError::NoFrames => write!(f, "No frames have been added to the stack"),
            LiveStackError::InvalidConfig(msg) => write!(f, "Invalid configuration: {}", msg),
        }
    }
}

impl std::error::Error for LiveStackError {}

/// Accumulator for live/real-time image stacking.
///
/// Provides incremental stacking with real-time preview capability.
#[derive(Debug)]
pub struct LiveStackAccumulator {
    /// Accumulated sum (or weighted sum)
    sum: Vec<f64>,

    /// Accumulated weight per pixel
    weights: Vec<f64>,

    /// Sum of squared values (for variance tracking)
    sum_sq: Option<Vec<f64>>,

    /// Rolling buffer for sigma clipping mode
    rolling_buffer: Option<RollingBuffer>,

    /// Reference statistics for normalization (from first frame)
    reference_stats: Option<NormalizationStats>,

    /// Dimensions
    width: usize,
    height: usize,
    channels: usize,

    /// Configuration
    config: LiveStackConfig,

    /// Accumulated statistics
    stats: LiveStackStats,

    /// Frame quality history (for weighted averaging of stats)
    quality_history: Vec<(f32, LiveFrameQuality)>, // (weight, quality)
}

/// Rolling buffer for sigma clipping mode
#[derive(Debug)]
struct RollingBuffer {
    /// Ring buffer of recent frames
    frames: Vec<Vec<f32>>,
    /// Ring buffer of weights
    frame_weights: Vec<f32>,
    /// Current write position
    write_pos: usize,
    /// Number of frames currently in buffer
    count: usize,
    /// Sigma threshold for clipping
    sigma: f32,
}

impl RollingBuffer {
    fn new(window_size: usize, pixel_count: usize, sigma: f32) -> Self {
        Self {
            frames: (0..window_size).map(|_| vec![0.0; pixel_count]).collect(),
            frame_weights: vec![0.0; window_size],
            write_pos: 0,
            count: 0,
            sigma,
        }
    }

    fn add_frame(&mut self, pixels: &[f32], weight: f32) {
        let idx = self.write_pos;
        self.frames[idx].copy_from_slice(pixels);
        self.frame_weights[idx] = weight;
        self.write_pos = (self.write_pos + 1) % self.frames.len();
        if self.count < self.frames.len() {
            self.count += 1;
        }
    }

    fn compute_sigma_clipped(&self, pixel_idx: usize) -> f32 {
        if self.count == 0 {
            return 0.0;
        }

        // Gather values for this pixel
        let mut values: Vec<(f32, f32)> = (0..self.count)
            .map(|i| (self.frames[i][pixel_idx], self.frame_weights[i]))
            .collect();

        // Simple iterative sigma clipping
        for _ in 0..3 {
            if values.len() < 3 {
                break;
            }

            // Compute weighted mean and std
            let total_weight: f32 = values.iter().map(|(_, w)| w).sum();
            if total_weight <= 0.0 {
                break;
            }

            let mean: f32 = values.iter().map(|(v, w)| v * w).sum::<f32>() / total_weight;

            let variance: f32 = values
                .iter()
                .map(|(v, w)| w * (v - mean).powi(2))
                .sum::<f32>()
                / total_weight;
            let std = variance.sqrt().max(1e-6);

            // Remove outliers
            let threshold = self.sigma * std;
            values.retain(|(v, _)| (*v - mean).abs() <= threshold);
        }

        // Compute final weighted mean
        let total_weight: f32 = values.iter().map(|(_, w)| w).sum();
        if total_weight <= 0.0 {
            0.0
        } else {
            values.iter().map(|(v, w)| v * w).sum::<f32>() / total_weight
        }
    }
}

/// Statistics for frame normalization
#[derive(Debug, Clone)]
struct NormalizationStats {
    median: f32,
    mad: f32,
}

impl LiveStackAccumulator {
    /// Create a live stack accumulator from a reference image.
    ///
    /// This is a convenience wrapper that extracts dimensions from the reference image.
    /// The reference image itself is not added to the stack - call `add_frame()` separately
    /// if you want to include it.
    ///
    /// # Arguments
    /// * `reference` - Reference image to derive dimensions from
    /// * `config` - Stacking configuration
    ///
    /// # Example
    /// ```rust,ignore
    /// use lumos::{AstroImage, LiveStackAccumulator, LiveStackConfig};
    ///
    /// let reference = AstroImage::from_file("first_frame.fits")?;
    /// let config = LiveStackConfig::default();
    /// let mut stack = LiveStackAccumulator::from_reference(&reference, config)?;
    /// ```
    pub fn from_reference(
        reference: &AstroImage,
        config: LiveStackConfig,
    ) -> Result<Self, LiveStackError> {
        Self::new(
            reference.width(),
            reference.height(),
            reference.channels(),
            config,
        )
    }

    /// Create a new live stack accumulator.
    ///
    /// # Arguments
    /// * `width` - Image width in pixels
    /// * `height` - Image height in pixels
    /// * `channels` - Number of color channels (1 for mono, 3 for RGB)
    /// * `config` - Stacking configuration
    pub fn new(
        width: usize,
        height: usize,
        channels: usize,
        config: LiveStackConfig,
    ) -> Result<Self, LiveStackError> {
        let pixel_count = width * height * channels;

        // Validate config
        if let LiveStackMode::RollingSigmaClip { window_size, sigma } = &config.mode {
            if *window_size < 3 {
                return Err(LiveStackError::InvalidConfig(
                    "Rolling sigma clip window size must be at least 3".to_string(),
                ));
            }
            if *sigma <= 0.0 {
                return Err(LiveStackError::InvalidConfig(
                    "Sigma must be positive".to_string(),
                ));
            }
        }

        let rolling_buffer = match config.mode {
            LiveStackMode::RollingSigmaClip { window_size, sigma } => {
                Some(RollingBuffer::new(window_size, pixel_count, sigma))
            }
            _ => None,
        };

        let sum_sq = if config.track_variance {
            Some(vec![0.0; pixel_count])
        } else {
            None
        };

        Ok(Self {
            sum: vec![0.0; pixel_count],
            weights: vec![0.0; pixel_count],
            sum_sq,
            rolling_buffer,
            reference_stats: None,
            width,
            height,
            channels,
            config,
            stats: LiveStackStats::default(),
            quality_history: Vec::new(),
        })
    }

    /// Add a frame to the stack.
    ///
    /// # Arguments
    /// * `frame` - The image frame to add
    /// * `quality` - Quality metrics for the frame (or `LiveFrameQuality::unknown()`)
    ///
    /// # Returns
    /// Updated statistics after adding the frame
    pub fn add_frame(
        &mut self,
        frame: AstroImage,
        quality: LiveFrameQuality,
    ) -> Result<&LiveStackStats, LiveStackError> {
        // Check dimensions
        let (fw, fh, fc) = (frame.width(), frame.height(), frame.channels());
        if fw != self.width || fh != self.height || fc != self.channels {
            return Err(LiveStackError::DimensionMismatch {
                expected: (self.width, self.height, self.channels),
                got: (fw, fh, fc),
            });
        }

        // Compute weight
        let weight = match self.config.mode {
            LiveStackMode::RunningMean => 1.0,
            LiveStackMode::WeightedMean | LiveStackMode::RollingSigmaClip { .. } => {
                quality.compute_weight()
            }
        };

        // Get pixels, optionally normalized
        let pixels = frame.into_interleaved_pixels();
        let pixels_to_use: Vec<f32> = if self.config.normalize {
            // Compute or use reference stats
            if self.reference_stats.is_none() {
                self.reference_stats = Some(Self::compute_stats(&pixels));
            }

            self.normalize_frame(&pixels)
        } else {
            pixels
        };

        // Update accumulator based on mode
        match &mut self.rolling_buffer {
            Some(buffer) => {
                // Rolling sigma clip mode - add to ring buffer
                buffer.add_frame(&pixels_to_use, weight);
            }
            None => {
                // Running mean or weighted mean mode
                for (i, &pixel) in pixels_to_use.iter().enumerate() {
                    let weighted_pixel = pixel as f64 * weight as f64;
                    self.sum[i] += weighted_pixel;
                    self.weights[i] += weight as f64;

                    if let Some(sum_sq) = &mut self.sum_sq {
                        sum_sq[i] += (pixel as f64).powi(2) * weight as f64;
                    }
                }
            }
        }

        // Update statistics
        self.stats.frame_count += 1;
        self.stats.total_weight += weight as f64;
        self.quality_history.push((weight, quality));

        self.update_aggregate_stats();

        Ok(&self.stats)
    }

    /// Get a preview of the current stack.
    ///
    /// This is efficient and can be called frequently for real-time display.
    pub fn preview(&self) -> Result<AstroImage, LiveStackError> {
        if self.stats.frame_count == 0 {
            return Err(LiveStackError::NoFrames);
        }

        let pixels = self.compute_result_pixels();
        Ok(AstroImage::from_pixels(
            ImageDimensions::new(self.width, self.height, self.channels),
            pixels,
        ))
    }

    /// Finalize the stack and return the result.
    ///
    /// This consumes the accumulator.
    pub fn finalize(self) -> Result<LiveStackResult, LiveStackError> {
        if self.stats.frame_count == 0 {
            return Err(LiveStackError::NoFrames);
        }

        let pixels = self.compute_result_pixels();
        let image = AstroImage::from_pixels(
            ImageDimensions::new(self.width, self.height, self.channels),
            pixels,
        );

        Ok(LiveStackResult {
            image,
            stats: self.stats,
        })
    }

    /// Get current statistics.
    pub fn stats(&self) -> &LiveStackStats {
        &self.stats
    }

    /// Get the number of frames in the stack.
    pub fn frame_count(&self) -> usize {
        self.stats.frame_count
    }

    /// Check if the accumulator is empty.
    pub fn is_empty(&self) -> bool {
        self.stats.frame_count == 0
    }

    /// Reset the accumulator to its initial state.
    pub fn reset(&mut self) {
        self.sum.fill(0.0);
        self.weights.fill(0.0);
        if let Some(sum_sq) = &mut self.sum_sq {
            sum_sq.fill(0.0);
        }
        if let Some(buffer) = &mut self.rolling_buffer {
            buffer.count = 0;
            buffer.write_pos = 0;
        }
        self.reference_stats = None;
        self.stats = LiveStackStats::default();
        self.quality_history.clear();
    }

    // Private helper methods

    fn compute_stats(pixels: &[f32]) -> NormalizationStats {
        // Sample pixels for speed (every 100th pixel)
        let sample: Vec<f32> = pixels.iter().step_by(100).copied().collect();
        if sample.is_empty() {
            return NormalizationStats {
                median: 0.0,
                mad: 1.0,
            };
        }

        let mut sorted = sample.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let median = sorted[sorted.len() / 2];

        // Median Absolute Deviation
        let mut deviations: Vec<f32> = sorted.iter().map(|&v| (v - median).abs()).collect();
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mad = deviations[deviations.len() / 2].max(0.001);

        NormalizationStats { median, mad }
    }

    fn normalize_frame(&self, pixels: &[f32]) -> Vec<f32> {
        let ref_stats = self.reference_stats.as_ref().unwrap();
        let frame_stats = Self::compute_stats(pixels);

        // Scale and offset to match reference
        let scale = ref_stats.mad / frame_stats.mad.max(0.001);
        let offset = ref_stats.median - frame_stats.median * scale;

        pixels.iter().map(|&v| v * scale + offset).collect()
    }

    fn compute_result_pixels(&self) -> Vec<f32> {
        match &self.rolling_buffer {
            Some(buffer) => {
                // Sigma-clipped result from rolling buffer
                (0..self.sum.len())
                    .map(|i| buffer.compute_sigma_clipped(i))
                    .collect()
            }
            None => {
                // Weighted mean from accumulators
                self.sum
                    .iter()
                    .zip(self.weights.iter())
                    .map(|(&s, &w)| if w > 0.0 { (s / w) as f32 } else { 0.0 })
                    .collect()
            }
        }
    }

    fn update_aggregate_stats(&mut self) {
        if self.quality_history.is_empty() {
            return;
        }

        // Compute weighted averages of quality metrics
        let total_weight: f32 = self.quality_history.iter().map(|(w, _)| w).sum();
        if total_weight <= 0.0 {
            return;
        }

        let (sum_snr, sum_fwhm, sum_ecc) =
            self.quality_history
                .iter()
                .fold((0.0, 0.0, 0.0), |acc, (w, q)| {
                    (
                        acc.0 + w * q.snr,
                        acc.1 + w * q.fwhm,
                        acc.2 + w * q.eccentricity,
                    )
                });

        self.stats.mean_snr = sum_snr / total_weight;
        self.stats.mean_fwhm = sum_fwhm / total_weight;
        self.stats.mean_eccentricity = sum_ecc / total_weight;

        // SNR improvement: √(Σw²) for weighted, √N for unweighted
        let effective_n = match self.config.mode {
            LiveStackMode::RunningMean => self.stats.frame_count as f32,
            _ => {
                let sum_w_sq: f32 = self.quality_history.iter().map(|(w, _)| w * w).sum();
                if sum_w_sq > 0.0 {
                    (total_weight * total_weight) / sum_w_sq
                } else {
                    self.stats.frame_count as f32
                }
            }
        };
        self.stats.snr_improvement = effective_n.sqrt();

        // Mean variance if tracked
        if let Some(sum_sq) = &self.sum_sq {
            let mut total_variance = 0.0f64;
            let pixel_count = self.sum.len();
            for ((&s, &w), &sq) in self.sum.iter().zip(self.weights.iter()).zip(sum_sq.iter()) {
                if w > 0.0 {
                    let mean = s / w;
                    let mean_sq = sq / w;
                    total_variance += (mean_sq - mean * mean).max(0.0);
                }
            }
            self.stats.mean_variance = Some((total_variance / pixel_count as f64) as f32);
        }
    }
}

/// Result of live stacking.
#[derive(Debug)]
pub struct LiveStackResult {
    /// The stacked image
    pub image: AstroImage,

    /// Statistics about the stack
    pub stats: LiveStackStats,
}

impl fmt::Display for LiveStackResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "LiveStackResult {{ {}×{}, {} frames, SNR×{:.1} }}",
            self.image.dimensions().width,
            self.image.dimensions().height,
            self.stats.frame_count,
            self.stats.snr_improvement
        )
    }
}

/// Streaming quality metrics for real-time display.
#[derive(Debug, Clone, Default)]
pub struct LiveQualityStream {
    /// Recent frame SNR values (for plotting)
    pub snr_history: Vec<f32>,

    /// Recent frame FWHM values (for plotting)
    pub fwhm_history: Vec<f32>,

    /// Recent frame eccentricity values (for plotting)
    pub eccentricity_history: Vec<f32>,

    /// Running estimate of final stack quality
    pub estimated_final_snr: f32,

    /// Maximum history size
    max_history: usize,
}

impl LiveQualityStream {
    /// Create a new quality stream with specified history size.
    pub fn new(max_history: usize) -> Self {
        Self {
            snr_history: Vec::with_capacity(max_history),
            fwhm_history: Vec::with_capacity(max_history),
            eccentricity_history: Vec::with_capacity(max_history),
            estimated_final_snr: 0.0,
            max_history,
        }
    }

    /// Update with a new frame's quality metrics.
    pub fn update(&mut self, quality: &LiveFrameQuality, stack_stats: &LiveStackStats) {
        // Update histories
        if self.snr_history.len() >= self.max_history {
            self.snr_history.remove(0);
            self.fwhm_history.remove(0);
            self.eccentricity_history.remove(0);
        }

        self.snr_history.push(quality.snr);
        self.fwhm_history.push(quality.fwhm);
        self.eccentricity_history.push(quality.eccentricity);

        // Update estimated final SNR
        self.estimated_final_snr = stack_stats.mean_snr * stack_stats.snr_improvement;
    }

    /// Get the current trend (positive = improving, negative = degrading).
    pub fn snr_trend(&self) -> f32 {
        if self.snr_history.len() < 10 {
            return 0.0;
        }

        let recent: f32 = self.snr_history.iter().rev().take(5).sum::<f32>() / 5.0;
        let older: f32 = self.snr_history.iter().rev().skip(5).take(5).sum::<f32>() / 5.0;

        (recent - older) / older.max(0.001)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_frame(width: usize, height: usize, channels: usize, value: f32) -> AstroImage {
        let pixels = vec![value; width * height * channels];
        AstroImage::from_pixels(ImageDimensions::new(width, height, channels), pixels)
    }

    #[test]
    fn test_running_mean_basic() {
        let config = LiveStackConfig {
            mode: LiveStackMode::RunningMean,
            normalize: false,
            ..Default::default()
        };

        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        // Add two frames with values 1.0 and 3.0
        let frame1 = create_test_frame(10, 10, 1, 1.0);
        let frame2 = create_test_frame(10, 10, 1, 3.0);

        stack
            .add_frame(frame1, LiveFrameQuality::unknown())
            .unwrap();
        stack
            .add_frame(frame2, LiveFrameQuality::unknown())
            .unwrap();

        // Mean should be 2.0
        let result = stack.preview().unwrap();
        assert!((result.channel(0)[0] - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_weighted_mean() {
        let config = LiveStackConfig {
            mode: LiveStackMode::WeightedMean,
            normalize: false,
            ..Default::default()
        };

        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        // Frame 1: value 1.0, high quality (weight ~2.0)
        let frame1 = create_test_frame(10, 10, 1, 1.0);
        let q1 = LiveFrameQuality {
            snr: 20.0,
            fwhm: 2.0,
            eccentricity: 1.0,
            star_count: 100,
        };

        // Frame 2: value 5.0, low quality (weight ~0.5)
        let frame2 = create_test_frame(10, 10, 1, 5.0);
        let q2 = LiveFrameQuality {
            snr: 5.0,
            fwhm: 4.0,
            eccentricity: 2.0,
            star_count: 50,
        };

        stack.add_frame(frame1, q1).unwrap();
        stack.add_frame(frame2, q2).unwrap();

        // Weighted mean should be closer to 1.0 (higher weight)
        let result = stack.preview().unwrap();
        assert!(
            result.channel(0)[0] < 3.0,
            "Weighted mean should favor frame 1"
        );
    }

    #[test]
    fn test_dimension_mismatch() {
        let config = LiveStackConfig::default();
        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        let frame1 = create_test_frame(10, 10, 1, 1.0);
        let frame2 = create_test_frame(20, 20, 1, 2.0);

        stack
            .add_frame(frame1, LiveFrameQuality::unknown())
            .unwrap();
        let result = stack.add_frame(frame2, LiveFrameQuality::unknown());

        assert!(matches!(
            result,
            Err(LiveStackError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_empty_stack() {
        let config = LiveStackConfig::default();
        let stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        assert!(stack.is_empty());
        assert!(matches!(stack.preview(), Err(LiveStackError::NoFrames)));
    }

    #[test]
    fn test_reset() {
        let config = LiveStackConfig::default();
        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        let frame = create_test_frame(10, 10, 1, 1.0);
        stack.add_frame(frame, LiveFrameQuality::unknown()).unwrap();

        assert_eq!(stack.frame_count(), 1);

        stack.reset();

        assert_eq!(stack.frame_count(), 0);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_stats_tracking() {
        let config = LiveStackConfig::default();
        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        for i in 0..10 {
            let frame = create_test_frame(10, 10, 1, i as f32);
            stack.add_frame(frame, LiveFrameQuality::unknown()).unwrap();
        }

        let stats = stack.stats();
        assert_eq!(stats.frame_count, 10);
        assert!(stats.snr_improvement > 1.0);
    }

    #[test]
    fn test_rolling_sigma_clip() {
        let config = LiveStackConfig {
            mode: LiveStackMode::RollingSigmaClip {
                window_size: 5,
                sigma: 1.5, // Use stricter sigma for better outlier rejection
            },
            normalize: false,
            ..Default::default()
        };

        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        // Add 4 normal frames with value 1.0
        for _ in 0..4 {
            let frame = create_test_frame(10, 10, 1, 1.0);
            stack.add_frame(frame, LiveFrameQuality::unknown()).unwrap();
        }

        // Add 1 extreme outlier frame with value 500.0
        // (Needs to be far enough that even with initial high std, it gets clipped)
        let outlier = create_test_frame(10, 10, 1, 500.0);
        stack
            .add_frame(outlier, LiveFrameQuality::unknown())
            .unwrap();

        // Result should be close to 1.0 (outlier rejected after iterative clipping)
        let result = stack.preview().unwrap();
        assert!(
            result.channel(0)[0] < 10.0,
            "Sigma clipping should reject outlier, got {}",
            result.channel(0)[0]
        );
    }

    #[test]
    fn test_quality_weight_computation() {
        // High quality
        let high_q = LiveFrameQuality {
            snr: 50.0,
            fwhm: 2.0,
            eccentricity: 1.0,
            star_count: 200,
        };

        // Low quality
        let low_q = LiveFrameQuality {
            snr: 10.0,
            fwhm: 5.0,
            eccentricity: 2.0,
            star_count: 50,
        };

        let high_weight = high_q.compute_weight();
        let low_weight = low_q.compute_weight();

        assert!(
            high_weight > low_weight * 5.0,
            "High quality should have much higher weight"
        );
    }

    #[test]
    fn test_finalize() {
        let config = LiveStackConfig::default();
        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        let frame = create_test_frame(10, 10, 1, 42.0);
        stack.add_frame(frame, LiveFrameQuality::unknown()).unwrap();

        let result = stack.finalize().unwrap();
        assert_eq!(result.stats.frame_count, 1);
        assert!((result.image.channel(0)[0] - 42.0).abs() < 0.001);
    }

    #[test]
    fn test_quality_stream() {
        let mut stream = LiveQualityStream::new(100);

        let q = LiveFrameQuality {
            snr: 20.0,
            fwhm: 2.5,
            eccentricity: 1.1,
            star_count: 100,
        };

        let stats = LiveStackStats {
            frame_count: 10,
            mean_snr: 18.0,
            snr_improvement: 3.16, // √10
            ..Default::default()
        };

        stream.update(&q, &stats);

        assert_eq!(stream.snr_history.len(), 1);
        assert!((stream.estimated_final_snr - 56.88).abs() < 1.0); // 18 * 3.16
    }

    #[test]
    fn test_invalid_config() {
        let config = LiveStackConfig {
            mode: LiveStackMode::RollingSigmaClip {
                window_size: 1, // Invalid: too small
                sigma: 2.0,
            },
            ..Default::default()
        };

        let result = LiveStackAccumulator::new(10, 10, 1, config);
        assert!(matches!(result, Err(LiveStackError::InvalidConfig(_))));
    }

    #[test]
    fn test_rgb_stacking() {
        let config = LiveStackConfig {
            mode: LiveStackMode::RunningMean,
            normalize: false,
            ..Default::default()
        };

        let mut stack = LiveStackAccumulator::new(10, 10, 3, config).unwrap();

        let frame1 = create_test_frame(10, 10, 3, 0.5);
        let frame2 = create_test_frame(10, 10, 3, 0.5);
        stack
            .add_frame(frame1, LiveFrameQuality::unknown())
            .unwrap();
        stack
            .add_frame(frame2, LiveFrameQuality::unknown())
            .unwrap();

        let result = stack.preview().unwrap();
        assert_eq!(result.dimensions().channels, 3);
        assert!((result.channel(0)[0] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_from_reference() {
        let reference = create_test_frame(100, 80, 3, 0.5);
        let config = LiveStackConfig::default();

        let stack = LiveStackAccumulator::from_reference(&reference, config).unwrap();

        // Verify dimensions match reference
        assert_eq!(stack.width, 100);
        assert_eq!(stack.height, 80);
        assert_eq!(stack.channels, 3);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_builder_running_mean() {
        let config = LiveStackConfig::builder().running_mean().build();

        assert_eq!(config.mode, LiveStackMode::RunningMean);
        assert!(config.normalize); // default true
        assert!(config.track_variance); // default true
    }

    #[test]
    fn test_builder_weighted_mean() {
        let config = LiveStackConfig::builder()
            .weighted_mean()
            .normalize(false)
            .build();

        assert_eq!(config.mode, LiveStackMode::WeightedMean);
        assert!(!config.normalize);
    }

    #[test]
    fn test_builder_rolling_sigma_clip() {
        let config = LiveStackConfig::builder()
            .rolling_sigma_clip(20, 2.5)
            .track_variance(false)
            .build();

        assert_eq!(
            config.mode,
            LiveStackMode::RollingSigmaClip {
                window_size: 20,
                sigma: 2.5
            }
        );
        assert!(!config.track_variance);
    }

    #[test]
    fn test_builder_all_options() {
        let config = LiveStackConfig::builder()
            .weighted_mean()
            .normalize(false)
            .track_variance(true)
            .preview_channel(Some(1))
            .build();

        assert_eq!(config.mode, LiveStackMode::WeightedMean);
        assert!(!config.normalize);
        assert!(config.track_variance);
        assert_eq!(config.preview_channel, Some(1));
    }

    #[test]
    fn test_builder_default() {
        let config = LiveStackConfig::builder().build();
        let default_config = LiveStackConfig::default();

        assert_eq!(config.mode, default_config.mode);
        assert_eq!(config.normalize, default_config.normalize);
        assert_eq!(config.track_variance, default_config.track_variance);
        assert_eq!(config.preview_channel, default_config.preview_channel);
    }

    #[test]
    fn test_builder_with_accumulator() {
        // Verify builder works end-to-end with accumulator
        let config = LiveStackConfig::builder()
            .weighted_mean()
            .normalize(false)
            .build();

        let mut stack = LiveStackAccumulator::new(10, 10, 1, config).unwrap();

        let frame = create_test_frame(10, 10, 1, 1.0);
        stack.add_frame(frame, LiveFrameQuality::unknown()).unwrap();

        let result = stack.preview().unwrap();
        assert!((result.channel(0)[0] - 1.0).abs() < 0.001);
    }
}
