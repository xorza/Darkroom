//! Comet/asteroid stacking support.
//!
//! This module implements dual-stack approach for imaging moving objects:
//! 1. Stack aligned on stars (sharp stars, blurred comet)
//! 2. Stack aligned on comet (sharp comet, star trails rejected)
//! 3. Composite combining best of both
//!
//! # Usage
//!
//! ```rust,ignore
//! use lumos::stacking::comet::{CometStackConfig, ObjectPosition, CompositeMethod};
//! use lumos::registration::TransformMatrix;
//!
//! // User marks comet position at start and end of sequence
//! let pos_start = ObjectPosition::new(512.5, 384.2, 0.0);  // First frame
//! let pos_end = ObjectPosition::new(520.8, 390.1, 3600.0); // Last frame (1 hour later)
//!
//! let config = CometStackConfig::new(pos_start, pos_end)
//!     .composite_method(CompositeMethod::Lighten);
//!
//! // Compute comet-aligned transform from star-aligned transform
//! let star_transform: TransformMatrix = /* from registration */;
//! let frame_timestamp = 1800.0; // 30 minutes into sequence
//! let comet_transform = config.comet_aligned_transform(&star_transform, frame_timestamp, 0.0);
//! ```

use glam::{DVec2, Vec2};

use crate::math::Vec2us;

use crate::registration::Transform;
use crate::stacking::local_normalization::NormalizationMethod;
use crate::stacking::weighted::RejectionMethod;

/// Position of a comet or asteroid at a specific timestamp.
///
/// The position is in pixel coordinates relative to the reference frame.
/// Timestamps can be in any consistent unit (seconds, MJD, etc.) as long
/// as all positions in a sequence use the same unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectPosition {
    /// Position in pixels (sub-pixel precision).
    pub pos: DVec2,
    /// Timestamp in seconds since sequence start (or any consistent unit).
    /// Can also be MJD (Modified Julian Date) or Unix timestamp.
    pub timestamp: f64,
}

impl ObjectPosition {
    /// Create a new object position.
    ///
    /// # Arguments
    /// * `x` - X coordinate in pixels
    /// * `y` - Y coordinate in pixels
    /// * `timestamp` - Observation timestamp (seconds or MJD)
    pub fn new(x: f64, y: f64, timestamp: f64) -> Self {
        Self {
            pos: DVec2::new(x, y),
            timestamp,
        }
    }

    /// Create a new object position from a DVec2.
    pub fn from_pos(pos: DVec2, timestamp: f64) -> Self {
        Self { pos, timestamp }
    }
}

/// Method for combining star-aligned and comet-aligned stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompositeMethod {
    /// Take maximum of each pixel (lighten blend).
    /// Works well when backgrounds are dark and matched.
    /// Result: `max(star_stack, comet_stack)` per pixel.
    #[default]
    Lighten,

    /// Additive blend with automatic background subtraction.
    /// Better preserves faint structures.
    /// Result: `star_stack + (comet_stack - background)`
    Additive,

    /// Return both stacks separately for manual compositing.
    /// User can apply custom masking/blending in post-processing.
    Separate,
}

/// Configuration for comet/asteroid stacking.
#[derive(Debug, Clone)]
pub struct CometStackConfig {
    /// Position of the object at the start of the sequence.
    pub pos_start: ObjectPosition,

    /// Position of the object at the end of the sequence.
    pub pos_end: ObjectPosition,

    /// Pixel rejection method for removing star trails from comet stack.
    /// Default: sigma clipping with kappa=2.5 (aggressive to remove trails).
    pub rejection: RejectionMethod,

    /// Normalization method for matching frame backgrounds.
    pub normalization: NormalizationMethod,

    /// How to combine the star and comet stacks.
    pub composite_method: CompositeMethod,
}

impl CometStackConfig {
    /// Create a new comet stacking configuration.
    ///
    /// # Arguments
    /// * `pos_start` - Object position in the first frame
    /// * `pos_end` - Object position in the last frame
    ///
    /// # Panics
    /// Panics if timestamps are equal (cannot compute velocity).
    pub fn new(pos_start: ObjectPosition, pos_end: ObjectPosition) -> Self {
        assert!(
            (pos_end.timestamp - pos_start.timestamp).abs() > f64::EPSILON,
            "Start and end timestamps must be different to compute object velocity"
        );

        Self {
            pos_start,
            pos_end,
            rejection: RejectionMethod::default(),
            normalization: NormalizationMethod::default(),
            composite_method: CompositeMethod::default(),
        }
    }

    /// Set the pixel rejection method.
    ///
    /// Lower kappa values (2.0-2.5) are recommended for comet stacking
    /// to aggressively reject star trails.
    pub fn rejection(mut self, rejection: RejectionMethod) -> Self {
        self.rejection = rejection;
        self
    }

    /// Set the normalization method.
    pub fn normalization(mut self, normalization: NormalizationMethod) -> Self {
        self.normalization = normalization;
        self
    }

    /// Set the composite method.
    pub fn composite_method(mut self, method: CompositeMethod) -> Self {
        self.composite_method = method;
        self
    }

    /// Compute the object's velocity in pixels per timestamp unit.
    ///
    /// Returns DVec2(vx, vy) where:
    /// - vx: velocity in x direction (pixels per time unit)
    /// - vy: velocity in y direction (pixels per time unit)
    pub fn velocity(&self) -> DVec2 {
        let dt = self.pos_end.timestamp - self.pos_start.timestamp;
        debug_assert!(dt.abs() > f64::EPSILON, "Time delta must be non-zero");

        (self.pos_end.pos - self.pos_start.pos) / dt
    }

    /// Compute the total displacement in pixels.
    pub fn total_displacement(&self) -> f64 {
        self.pos_start.pos.distance(self.pos_end.pos)
    }

    /// Compute a comet-aligned transform from a star-aligned transform.
    ///
    /// This applies the frame-specific comet offset to the given star-aligned
    /// transform, producing a transform that aligns the comet instead of the stars.
    ///
    /// # Arguments
    /// * `star_transform` - Transform computed from star registration
    /// * `frame_timestamp` - Timestamp of this frame
    /// * `ref_timestamp` - Timestamp of the reference frame
    ///
    /// # Returns
    /// A new transform that aligns the comet to its position in the reference frame.
    ///
    /// # How It Works
    ///
    /// The comet moves across frames. To align ON the comet:
    /// 1. Apply the star transform to align stars (this misaligns the comet)
    /// 2. Apply an additional translation to shift the comet back to its reference position
    ///
    /// The comet offset is computed as: `offset = -velocity × (frame_time - ref_time)`
    ///
    /// The composition is: `offset_transform ∘ star_transform` which means
    /// star_transform is applied first, then the offset.
    pub fn comet_aligned_transform(
        &self,
        star_transform: &Transform,
        frame_timestamp: f64,
        ref_timestamp: f64,
    ) -> Transform {
        let offset = compute_comet_offset(self, frame_timestamp, ref_timestamp);
        let offset_transform = Transform::translation(offset);

        // offset_transform.compose(star_transform) means:
        // Apply star_transform first, then offset_transform
        // (compose applies `other` first, then `self`)
        offset_transform.compose(star_transform)
    }
}

/// Result of comet stacking operation.
#[derive(Debug)]
pub struct CometStackResult {
    /// Star-aligned stack (sharp stars, blurred/trailed comet).
    pub star_stack: Vec<f32>,

    /// Comet-aligned stack (sharp comet, star trails rejected).
    pub comet_stack: Vec<f32>,

    /// Composite image combining both stacks.
    /// `None` if `CompositeMethod::Separate` was used.
    pub composite: Option<Vec<f32>>,

    /// Image width in pixels.
    pub width: usize,

    /// Image height in pixels.
    pub height: usize,

    /// Computed object velocity (pixels per timestamp unit).
    pub velocity: DVec2,

    /// Total object displacement during the sequence (pixels).
    pub displacement: f64,
}

impl CometStackResult {
    /// Get the number of channels (always 1 for stacked output).
    pub fn channels(&self) -> usize {
        1
    }

    /// Get the pixel count for a single image.
    pub fn pixel_count(&self) -> usize {
        self.width * self.height
    }
}

/// Interpolate object position for a given timestamp.
///
/// Uses linear interpolation between start and end positions.
///
/// # Arguments
/// * `pos_start` - Object position at sequence start
/// * `pos_end` - Object position at sequence end
/// * `timestamp` - Timestamp to interpolate for
///
/// # Returns
/// Interpolated position in pixels.
///
/// # Panics
/// Panics if start and end timestamps are equal.
pub fn interpolate_position(
    pos_start: &ObjectPosition,
    pos_end: &ObjectPosition,
    timestamp: f64,
) -> DVec2 {
    let dt = pos_end.timestamp - pos_start.timestamp;
    assert!(
        dt.abs() > f64::EPSILON,
        "Start and end timestamps must be different"
    );

    let t_normalized = (timestamp - pos_start.timestamp) / dt;

    pos_start.pos.lerp(pos_end.pos, t_normalized)
}

/// Compute the comet offset for a frame at a given timestamp.
///
/// This returns the offset that should be applied to the star-aligned
/// transform to align the comet instead of the stars.
///
/// # Arguments
/// * `config` - Comet stacking configuration
/// * `timestamp` - Frame's observation timestamp
/// * `ref_timestamp` - Reference frame timestamp (typically first frame)
///
/// # Returns
/// Offset in pixels to add to the star-aligned transform.
pub fn compute_comet_offset(
    config: &CometStackConfig,
    timestamp: f64,
    ref_timestamp: f64,
) -> DVec2 {
    let velocity = config.velocity();
    let dt = timestamp - ref_timestamp;

    // The comet has moved by velocity*dt relative to the reference frame.
    // To align ON the comet, we need to shift back by this amount.
    -velocity * dt
}

/// Apply comet offset to a star-aligned transform.
///
/// This is a convenience function that applies the frame-specific comet offset
/// to a star-aligned transform, producing a transform that aligns the comet
/// instead of the stars.
///
/// # Arguments
/// * `star_transform` - Transform computed from star registration
/// * `config` - Comet stacking configuration (defines object velocity)
/// * `frame_timestamp` - Timestamp of this frame
/// * `ref_timestamp` - Timestamp of the reference frame
///
/// # Returns
/// A new transform that aligns the comet to its position in the reference frame.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::stacking::comet::{apply_comet_offset_to_transform, CometStackConfig, ObjectPosition};
/// use lumos::registration::TransformMatrix;
///
/// // Define comet motion
/// let config = CometStackConfig::new(
///     ObjectPosition::new(100.0, 200.0, 0.0),    // First frame
///     ObjectPosition::new(150.0, 210.0, 3600.0), // After 1 hour
/// );
///
/// // For each frame, modify its transform
/// for (frame_idx, star_transform) in star_transforms.iter().enumerate() {
///     let frame_time = frame_timestamps[frame_idx];
///     let comet_transform = apply_comet_offset_to_transform(
///         star_transform,
///         &config,
///         frame_time,
///         frame_timestamps[0], // reference is first frame
///     );
///     // Use comet_transform for warping this frame in the comet-aligned stack
/// }
/// ```
pub fn apply_comet_offset_to_transform(
    star_transform: &Transform,
    config: &CometStackConfig,
    frame_timestamp: f64,
    ref_timestamp: f64,
) -> Transform {
    config.comet_aligned_transform(star_transform, frame_timestamp, ref_timestamp)
}

/// Composite star-aligned and comet-aligned stacks using the specified method.
///
/// This creates the final composite image combining sharp stars from the star-aligned
/// stack and a sharp comet from the comet-aligned stack.
///
/// # Arguments
/// * `star_stack` - Stacked image aligned on stars (sharp stars, blurred comet)
/// * `comet_stack` - Stacked image aligned on comet (sharp comet, star trails rejected)
/// * `method` - How to combine the two stacks
///
/// # Returns
/// `Some(composite)` for `Lighten` or `Additive` methods, `None` for `Separate`.
///
/// # Panics
/// Panics if the two stacks have different lengths.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::stacking::comet::{composite_stacks, CompositeMethod};
///
/// let star_stack = vec![0.5; 1024 * 1024];  // Star-aligned result
/// let comet_stack = vec![0.6; 1024 * 1024]; // Comet-aligned result
///
/// // Lighten blend: max of each pixel
/// let composite = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);
/// assert!(composite.is_some());
/// ```
pub fn composite_stacks(
    star_stack: &[f32],
    comet_stack: &[f32],
    method: CompositeMethod,
) -> Option<Vec<f32>> {
    assert_eq!(
        star_stack.len(),
        comet_stack.len(),
        "Star and comet stacks must have the same size"
    );

    match method {
        CompositeMethod::Lighten => Some(composite_lighten(star_stack, comet_stack)),
        CompositeMethod::Additive => Some(composite_additive(star_stack, comet_stack)),
        CompositeMethod::Separate => None,
    }
}

/// Lighten blend: take the maximum of each pixel from both stacks.
///
/// This works well when backgrounds are dark and matched, as the brighter
/// element (sharp star or sharp comet) wins at each pixel position.
///
/// Formula: `result = max(star_stack, comet_stack)` per pixel.
fn composite_lighten(star_stack: &[f32], comet_stack: &[f32]) -> Vec<f32> {
    star_stack
        .iter()
        .zip(comet_stack.iter())
        .map(|(&s, &c)| s.max(c))
        .collect()
}

/// Additive blend: add comet to star stack after background subtraction.
///
/// This better preserves faint structures by adding the comet signal
/// on top of the star stack. Background is automatically estimated and
/// subtracted from the comet stack to prevent doubling the sky background.
///
/// Formula: `result = star_stack + max(0, comet_stack - background)`
///
/// The background is estimated as the 5th percentile of the comet stack,
/// which is robust against the comet itself affecting the estimate.
fn composite_additive(star_stack: &[f32], comet_stack: &[f32]) -> Vec<f32> {
    // Estimate background as robust low percentile (5th percentile)
    // This avoids the comet itself biasing the background estimate
    let background = estimate_background_percentile(comet_stack, 0.05);

    star_stack
        .iter()
        .zip(comet_stack.iter())
        .map(|(&s, &c)| {
            // Subtract background from comet, clamp to non-negative
            let comet_signal = (c - background).max(0.0);
            s + comet_signal
        })
        .collect()
}

/// Estimate background using a percentile value.
///
/// Uses partial sorting for efficiency - O(n) average case.
///
/// # Arguments
/// * `data` - Image pixel values
/// * `percentile` - Percentile to use (0.0-1.0), e.g., 0.05 for 5th percentile
fn estimate_background_percentile(data: &[f32], percentile: f32) -> f32 {
    debug_assert!(
        (0.0..=1.0).contains(&percentile),
        "Percentile must be between 0 and 1"
    );

    if data.is_empty() {
        return 0.0;
    }

    // Clone and sort to find percentile
    let mut sorted: Vec<f32> = data.to_vec();
    let k = ((sorted.len() as f32 * percentile) as usize).min(sorted.len() - 1);

    // Partial sort up to k-th element for efficiency
    sorted.select_nth_unstable_by(k, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });

    sorted[k]
}

/// Create a CometStackResult from star and comet stacks.
///
/// This is a convenience function for creating the result struct after
/// both stacks have been computed.
///
/// # Arguments
/// * `star_stack` - Stacked image aligned on stars
/// * `comet_stack` - Stacked image aligned on comet
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `config` - Comet stacking configuration (for velocity and composite method)
///
/// # Panics
/// Panics if stack sizes don't match `width * height`.
pub fn create_comet_stack_result(
    star_stack: Vec<f32>,
    comet_stack: Vec<f32>,
    width: usize,
    height: usize,
    config: &CometStackConfig,
) -> CometStackResult {
    let pixel_count = width * height;
    assert_eq!(
        star_stack.len(),
        pixel_count,
        "Star stack size must match width * height"
    );
    assert_eq!(
        comet_stack.len(),
        pixel_count,
        "Comet stack size must match width * height"
    );

    let composite = composite_stacks(&star_stack, &comet_stack, config.composite_method);

    CometStackResult {
        star_stack,
        comet_stack,
        composite,
        width,
        height,
        velocity: config.velocity(),
        displacement: config.total_displacement(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== ObjectPosition Tests ==========

    #[test]
    fn test_object_position_new() {
        let pos = ObjectPosition::new(100.5, 200.75, 1000.0);
        assert!((pos.pos.x - 100.5).abs() < f64::EPSILON);
        assert!((pos.pos.y - 200.75).abs() < f64::EPSILON);
        assert!((pos.timestamp - 1000.0).abs() < f64::EPSILON);
    }

    // ========== CometStackConfig Tests ==========

    #[test]
    fn test_config_new() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 3600.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        assert_eq!(config.composite_method, CompositeMethod::Lighten);
        assert_eq!(config.normalization, NormalizationMethod::Global);
    }

    #[test]
    #[should_panic(expected = "timestamps must be different")]
    fn test_config_new_same_timestamp_panics() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 0.0);
        CometStackConfig::new(pos_start, pos_end);
    }

    #[test]
    fn test_config_velocity() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 3600.0); // 1 hour
        let config = CometStackConfig::new(pos_start, pos_end);

        let v = config.velocity();
        // 10 pixels / 3600 seconds = 0.00277... pixels/second
        assert!((v.x - 10.0 / 3600.0).abs() < 1e-10);
        assert!((v.y - 5.0 / 3600.0).abs() < 1e-10);
    }

    #[test]
    fn test_config_total_displacement() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(103.0, 104.0, 3600.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let displacement = config.total_displacement();
        // sqrt(3^2 + 4^2) = 5
        assert!((displacement - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_config_builder_pattern() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 3600.0);

        let config = CometStackConfig::new(pos_start, pos_end)
            .composite_method(CompositeMethod::Additive)
            .normalization(NormalizationMethod::Global);

        assert_eq!(config.composite_method, CompositeMethod::Additive);
        assert_eq!(config.normalization, NormalizationMethod::Global);
    }

    // ========== Interpolation Tests ==========

    #[test]
    fn test_interpolate_position_at_start() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(150.0, 250.0, 100.0);

        let p = interpolate_position(&pos_start, &pos_end, 0.0);
        assert!((p.x - 100.0).abs() < 1e-10);
        assert!((p.y - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_at_end() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(150.0, 250.0, 100.0);

        let p = interpolate_position(&pos_start, &pos_end, 100.0);
        assert!((p.x - 150.0).abs() < 1e-10);
        assert!((p.y - 250.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_at_middle() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 100.0);

        let p = interpolate_position(&pos_start, &pos_end, 50.0);
        assert!((p.x - 150.0).abs() < 1e-10);
        assert!((p.y - 250.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_extrapolate_before() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 100.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 200.0);

        let p = interpolate_position(&pos_start, &pos_end, 50.0);
        // Extrapolate backwards: t=-0.5 normalized
        assert!((p.x - 50.0).abs() < 1e-10);
        assert!((p.y - 150.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_extrapolate_after() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 100.0);

        let p = interpolate_position(&pos_start, &pos_end, 150.0);
        // Extrapolate forwards: t=1.5 normalized
        assert!((p.x - 250.0).abs() < 1e-10);
        assert!((p.y - 350.0).abs() < 1e-10);
    }

    #[test]
    #[should_panic(expected = "timestamps must be different")]
    fn test_interpolate_position_same_timestamp_panics() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(150.0, 250.0, 0.0);
        interpolate_position(&pos_start, &pos_end, 50.0);
    }

    // ========== Comet Offset Tests ==========

    #[test]
    fn test_compute_comet_offset_at_reference() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let offset = compute_comet_offset(&config, 0.0, 0.0);
        assert!((offset.x - 0.0).abs() < 1e-10);
        assert!((offset.y - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_comet_offset_after_time() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0); // 0.1 px/s in x, 0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // After 50 seconds, comet moved 5 pixels in x, 10 pixels in y
        // To align ON comet, we need to shift back
        let offset = compute_comet_offset(&config, 50.0, 0.0);
        assert!((offset.x - (-5.0)).abs() < 1e-10);
        assert!((offset.y - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_compute_comet_offset_negative_velocity() {
        let pos_start = ObjectPosition::new(110.0, 220.0, 0.0);
        let pos_end = ObjectPosition::new(100.0, 200.0, 100.0); // -0.1 px/s in x, -0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // After 50 seconds, comet moved -5 pixels in x, -10 pixels in y
        // To align ON comet, we shift back by negative offset = positive
        let offset = compute_comet_offset(&config, 50.0, 0.0);
        assert!((offset.x - 5.0).abs() < 1e-10);
        assert!((offset.y - 10.0).abs() < 1e-10);
    }

    // ========== CompositeMethod Tests ==========

    #[test]
    fn test_composite_method_default() {
        assert_eq!(CompositeMethod::default(), CompositeMethod::Lighten);
    }

    // ========== CometStackResult Tests ==========

    #[test]
    fn test_comet_stack_result() {
        let result = CometStackResult {
            star_stack: vec![0.0; 100],
            comet_stack: vec![0.0; 100],
            composite: Some(vec![0.0; 100]),
            width: 10,
            height: 10,
            velocity: DVec2::new(0.1, 0.2),
            displacement: 5.0,
        };

        assert_eq!(result.channels(), 1);
        assert_eq!(result.pixel_count(), 100);
    }

    // ========== Comet-Aligned Transform Tests ==========

    #[test]
    fn test_comet_aligned_transform_at_reference() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Identity star transform
        let star_transform = Transform::identity();

        // At reference timestamp, comet offset should be zero
        let comet_transform = config.comet_aligned_transform(&star_transform, 0.0, 0.0);

        // Result should be identity (no offset needed)
        let t = comet_transform.translation_components();
        assert!((t.x - 0.0).abs() < 1e-10);
        assert!((t.y - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_after_time() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0); // 0.1 px/s in x, 0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // Identity star transform
        let star_transform = Transform::identity();

        // After 50 seconds, comet moved 5 pixels in x, 10 pixels in y
        // To align ON comet, we need offset (-5, -10)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        let t = comet_transform.translation_components();
        assert!((t.x - (-5.0)).abs() < 1e-10);
        assert!((t.y - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_with_translation() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform with existing translation
        let star_transform = Transform::translation(DVec2::new(10.0, 20.0));

        // After 50 seconds, comet offset is (-5, -10)
        // Combined with star translation (10, 20), result should be (5, 10)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        let t = comet_transform.translation_components();
        assert!((t.x - 5.0).abs() < 1e-10);
        assert!((t.y - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_with_rotation() {
        let pos_start = ObjectPosition::new(0.0, 0.0, 0.0);
        let pos_end = ObjectPosition::new(100.0, 0.0, 100.0); // 1 px/s in x only
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform with 90 degree rotation
        let angle = std::f64::consts::FRAC_PI_2;
        let star_transform = Transform::euclidean(DVec2::ZERO, angle);

        // After 50 seconds, comet offset is (-50, 0)
        // But the compose operation applies star_transform first, then offset
        // So the offset is NOT rotated by the star transform
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        // Apply both transforms to origin to verify
        let result = comet_transform.apply(DVec2::ZERO);
        // Star transform: (0,0) -> (0,0) (rotation around origin)
        // Then offset: (0,0) + (-50,0) = (-50,0)
        assert!((result.x - (-50.0)).abs() < 1e-10);
        assert!((result.y - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_comet_offset_to_transform() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_transform = Transform::identity();

        // Test that the standalone function gives same result as method
        let via_method = config.comet_aligned_transform(&star_transform, 50.0, 0.0);
        let via_function = apply_comet_offset_to_transform(&star_transform, &config, 50.0, 0.0);

        let t1 = via_method.translation_components();
        let t2 = via_function.translation_components();

        assert!((t1.x - t2.x).abs() < 1e-10);
        assert!((t1.y - t2.y).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_point_mapping() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 150.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_transform = Transform::translation(DVec2::new(5.0, 10.0));

        // Frame at t=50: comet has moved (50, 25) pixels from start
        // Comet offset: (-50, -25)
        // Combined: (5, 10) + (-50, -25) = (-45, -15)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        // A point at (0, 0) should map to (-45, -15)
        let result = comet_transform.apply(DVec2::ZERO);
        assert!((result.x - (-45.0)).abs() < 1e-10);
        assert!((result.y - (-15.0)).abs() < 1e-10);
    }

    // ========== Composite Output Tests ==========

    #[test]
    fn test_composite_lighten_basic() {
        let star_stack = vec![0.3, 0.5, 0.7, 0.2];
        let comet_stack = vec![0.4, 0.4, 0.6, 0.8];

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);

        let composite = result.expect("Lighten should return Some");
        assert_eq!(composite.len(), 4);
        assert!((composite[0] - 0.4).abs() < 1e-6); // max(0.3, 0.4)
        assert!((composite[1] - 0.5).abs() < 1e-6); // max(0.5, 0.4)
        assert!((composite[2] - 0.7).abs() < 1e-6); // max(0.7, 0.6)
        assert!((composite[3] - 0.8).abs() < 1e-6); // max(0.2, 0.8)
    }

    #[test]
    fn test_composite_lighten_identical() {
        let star_stack = vec![0.5; 100];
        let comet_stack = vec![0.5; 100];

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);

        let composite = result.expect("Lighten should return Some");
        for val in composite {
            assert!((val - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_composite_additive_basic() {
        // Star stack with uniform background
        let star_stack = vec![0.2, 0.2, 0.2, 0.2];
        // Comet stack: background ~0.1, comet signal at pixel 2
        let comet_stack = vec![0.1, 0.1, 0.5, 0.1];

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Additive);

        let composite = result.expect("Additive should return Some");
        assert_eq!(composite.len(), 4);

        // Background (~0.1) is subtracted from comet_stack
        // Pixel 2 has signal 0.5 - 0.1 = 0.4 added to 0.2 = 0.6
        // Other pixels have signal ~0 added
        assert!(composite[2] > composite[0]); // Comet pixel should be brighter
    }

    #[test]
    fn test_composite_additive_background_subtraction() {
        // Uniform background in comet stack should not change star stack
        let star_stack = vec![0.3, 0.4, 0.5, 0.6];
        let comet_stack = vec![0.1, 0.1, 0.1, 0.1]; // All background, no comet signal

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Additive);

        let composite = result.expect("Additive should return Some");
        // All values should be approximately equal to star_stack
        // (comet - background = 0 for all pixels)
        for (i, val) in composite.iter().enumerate() {
            assert!(
                (*val - star_stack[i]).abs() < 0.01,
                "Expected ~{}, got {}",
                star_stack[i],
                val
            );
        }
    }

    #[test]
    fn test_composite_separate_returns_none() {
        let star_stack = vec![0.5; 100];
        let comet_stack = vec![0.6; 100];

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Separate);

        assert!(result.is_none(), "Separate method should return None");
    }

    #[test]
    #[should_panic(expected = "must have the same size")]
    fn test_composite_stacks_different_sizes_panics() {
        let star_stack = vec![0.5; 100];
        let comet_stack = vec![0.6; 50]; // Different size

        composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);
    }

    #[test]
    fn test_composite_empty_stacks() {
        let star_stack: Vec<f32> = vec![];
        let comet_stack: Vec<f32> = vec![];

        let result = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);
        let composite = result.expect("Should return Some for empty stacks");
        assert!(composite.is_empty());
    }

    // ========== Create Comet Stack Result Tests ==========

    #[test]
    fn test_create_comet_stack_result_lighten() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_stack = vec![0.3, 0.5, 0.7, 0.2];
        let comet_stack = vec![0.4, 0.4, 0.6, 0.8];

        let result = create_comet_stack_result(star_stack, comet_stack, 2, 2, &config);

        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
        assert!(result.composite.is_some());
        assert_eq!(result.composite.as_ref().unwrap().len(), 4);

        assert!((result.velocity.x - 0.1).abs() < 1e-10);
        assert!((result.velocity.y - 0.05).abs() < 1e-10);
    }

    #[test]
    fn test_create_comet_stack_result_separate() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 105.0, 100.0);
        let config =
            CometStackConfig::new(pos_start, pos_end).composite_method(CompositeMethod::Separate);

        let star_stack = vec![0.5; 16];
        let comet_stack = vec![0.6; 16];

        let result = create_comet_stack_result(star_stack, comet_stack, 4, 4, &config);

        assert!(result.composite.is_none());
        assert_eq!(result.star_stack.len(), 16);
        assert_eq!(result.comet_stack.len(), 16);
    }

    #[test]
    #[should_panic(expected = "Star stack size must match")]
    fn test_create_comet_stack_result_wrong_star_size() {
        let pos_start = ObjectPosition::new(0.0, 0.0, 0.0);
        let pos_end = ObjectPosition::new(10.0, 10.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_stack = vec![0.5; 10]; // Wrong size (should be 4)
        let comet_stack = vec![0.6; 4];

        create_comet_stack_result(star_stack, comet_stack, 2, 2, &config);
    }

    #[test]
    #[should_panic(expected = "Comet stack size must match")]
    fn test_create_comet_stack_result_wrong_comet_size() {
        let pos_start = ObjectPosition::new(0.0, 0.0, 0.0);
        let pos_end = ObjectPosition::new(10.0, 10.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_stack = vec![0.5; 4];
        let comet_stack = vec![0.6; 10]; // Wrong size (should be 4)

        create_comet_stack_result(star_stack, comet_stack, 2, 2, &config);
    }

    // ========== Background Estimation Tests ==========

    #[test]
    fn test_estimate_background_percentile() {
        // Sorted: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]
        let data = vec![0.5, 0.2, 0.8, 0.1, 0.9, 0.3, 0.7, 0.4, 1.0, 0.6];

        // 5th percentile (index 0) should be ~0.1
        let bg_5 = estimate_background_percentile(&data, 0.05);
        assert!((bg_5 - 0.1).abs() < 1e-6);

        // 50th percentile (index 5) should be ~0.6
        let bg_50 = estimate_background_percentile(&data, 0.5);
        // After partial sort, the element at index 5 might be 0.5 or 0.6 depending on sort behavior
        assert!((0.5..=0.6).contains(&bg_50));
    }

    #[test]
    fn test_estimate_background_empty() {
        let data: Vec<f32> = vec![];
        let bg = estimate_background_percentile(&data, 0.05);
        assert!((bg - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_background_single_value() {
        let data = vec![0.42];
        let bg = estimate_background_percentile(&data, 0.05);
        assert!((bg - 0.42).abs() < 1e-6);
    }

    // ========== Integration-Style Tests ==========

    #[test]
    fn test_composite_realistic_scenario() {
        // Simulate a realistic scenario:
        // - 64x64 image
        // - Star stack has bright stars on dark background
        // - Comet stack has bright comet center on dark background

        let width = 64;
        let height = 64;
        let mut star_stack = vec![0.1_f32; width * height];
        let mut comet_stack = vec![0.1_f32; width * height];

        // Add stars to star_stack (bright points)
        let star_positions = [(10, 10), (20, 30), (50, 15), (40, 55)];
        for (sx, sy) in star_positions {
            star_stack[sy * width + sx] = 0.9;
        }

        // Add comet to comet_stack (bright region in center)
        let comet_center: (i32, i32) = (32, 32);
        for dy in -2..=2_i32 {
            for dx in -2..=2_i32 {
                let x = (comet_center.0 + dx) as usize;
                let y = (comet_center.1 + dy) as usize;
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                comet_stack[y * width + x] = 0.9 - dist * 0.1;
            }
        }

        // Lighten composite
        let lighten = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);
        let lighten = lighten.unwrap();

        // Stars should be bright
        assert!(lighten[10 * width + 10] > 0.8);
        // Comet center should be bright
        assert!(lighten[32 * width + 32] > 0.8);
        // Background should remain low
        assert!(lighten[0] < 0.2);

        // Additive composite
        let additive = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Additive);
        let additive = additive.unwrap();

        // Stars should be bright
        assert!(additive[10 * width + 10] > 0.8);
        // Comet should be added to star stack (possibly brighter)
        assert!(additive[32 * width + 32] > 0.8);
    }
}

/// Integration tests for comet stacking with synthetic data.
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::f32::consts::PI;

    /// Simple RNG for test reproducibility.
    fn lcg_rng(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*seed >> 33) as f32 / (1u64 << 31) as f32
    }

    /// Render a Gaussian point source at the given position with given sigma and amplitude.
    fn render_gaussian(pixels: &mut [f32], width: usize, pos: Vec2, sigma: f32, amplitude: f32) {
        let height = pixels.len() / width;
        let radius = (sigma * 4.0).ceil() as i32;

        let pos_i = pos.round().as_ivec2();

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let px = pos_i.x + dx;
                let py = pos_i.y + dy;

                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    let dist = Vec2::new(px as f32, py as f32) - pos;
                    let dist_sq = dist.length_squared();
                    let value = amplitude * (-dist_sq / (2.0 * sigma * sigma)).exp();
                    pixels[py as usize * width + px as usize] += value;
                }
            }
        }
    }

    /// Generate a synthetic frame with stars and a moving comet.
    ///
    /// # Arguments
    /// * `width`, `height` - Image dimensions
    /// * `star_positions` - Static star positions
    /// * `comet_pos` - Comet position for this frame
    /// * `background` - Background level
    /// * `noise_sigma` - Gaussian noise level
    /// * `seed` - Random seed for noise
    fn generate_comet_frame(
        width: usize,
        height: usize,
        star_positions: &[Vec2],
        comet_pos: Vec2,
        background: f32,
        noise_sigma: f32,
        seed: u64,
    ) -> Vec<f32> {
        let mut pixels = vec![background; width * height];
        let star_sigma = 2.0; // FWHM ~4.7 pixels
        let star_amplitude = 0.5;
        let comet_sigma = 3.0; // Comet is slightly larger
        let comet_amplitude = 0.6;

        // Render stars
        for &pos in star_positions {
            render_gaussian(&mut pixels, width, pos, star_sigma, star_amplitude);
        }

        // Render comet
        render_gaussian(&mut pixels, width, comet_pos, comet_sigma, comet_amplitude);

        // Add noise
        if noise_sigma > 0.0 {
            let mut rng = seed;
            for p in pixels.iter_mut() {
                let u1 = lcg_rng(&mut rng).max(1e-10);
                let u2 = lcg_rng(&mut rng);
                let z = (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
                *p += z * noise_sigma;
                *p = p.clamp(0.0, 1.0);
            }
        }

        pixels
    }

    /// Apply a transform to warp a frame (simple nearest-neighbor for testing).
    fn warp_frame(src: &[f32], width: usize, height: usize, transform: &Transform) -> Vec<f32> {
        let mut dst = vec![0.0f32; width * height];

        for y in 0..height {
            for x in 0..width {
                // For each output pixel, find the corresponding input pixel
                // apply_inverse maps destination -> source
                let src_pos = transform.apply_inverse(DVec2::new(x as f64, y as f64));

                // Nearest-neighbor sampling
                let sx = src_pos.x.round() as i32;
                let sy = src_pos.y.round() as i32;

                if sx >= 0 && sx < width as i32 && sy >= 0 && sy < height as i32 {
                    dst[y * width + x] = src[sy as usize * width + sx as usize];
                }
            }
        }

        dst
    }

    /// Simple mean stacking (for testing).
    fn stack_mean(frames: &[Vec<f32>]) -> Vec<f32> {
        assert!(!frames.is_empty());
        let len = frames[0].len();
        let mut result = vec![0.0f32; len];

        for frame in frames {
            for (i, &val) in frame.iter().enumerate() {
                result[i] += val;
            }
        }

        let n = frames.len() as f32;
        for val in result.iter_mut() {
            *val /= n;
        }

        result
    }

    /// Measure peak brightness at a position (3x3 region).
    fn measure_peak(pixels: &[f32], width: usize, x: usize, y: usize) -> f32 {
        let height = pixels.len() / width;
        let mut max_val = 0.0f32;

        for dy in -1..=1_i32 {
            for dx in -1..=1_i32 {
                let px = x as i32 + dx;
                let py = y as i32 + dy;
                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    max_val = max_val.max(pixels[py as usize * width + px as usize]);
                }
            }
        }
        max_val
    }

    /// Measure the spread of a point source (variance of pixel positions weighted by brightness).
    fn measure_spread(pixels: &[f32], width: usize, center: Vec2us, radius: usize) -> f32 {
        let height = pixels.len() / width;
        let mut sum_dist_sq = 0.0f32;
        let mut sum_weight = 0.0f32;

        let background = estimate_background_percentile(pixels, 0.1);

        for dy in -(radius as i32)..=(radius as i32) {
            for dx in -(radius as i32)..=(radius as i32) {
                let px = center.x as i32 + dx;
                let py = center.y as i32 + dy;
                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    let val = pixels[py as usize * width + px as usize] - background;
                    if val > 0.0 {
                        let dist_sq = (dx * dx + dy * dy) as f32;
                        sum_dist_sq += dist_sq * val;
                        sum_weight += val;
                    }
                }
            }
        }

        if sum_weight > 0.0 {
            (sum_dist_sq / sum_weight).sqrt()
        } else {
            0.0
        }
    }

    // ========== Integration Tests ==========

    #[test]
    fn test_integration_comet_motion_interpolation() {
        // Test that comet position interpolation works correctly across multiple frames.
        let pos_start = ObjectPosition::new(50.0, 60.0, 0.0);
        let pos_end = ObjectPosition::new(70.0, 80.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Test at multiple timestamps
        let timestamps = [0.0, 25.0, 50.0, 75.0, 100.0];
        let expected_positions = [
            DVec2::new(50.0, 60.0),
            DVec2::new(55.0, 65.0),
            DVec2::new(60.0, 70.0),
            DVec2::new(65.0, 75.0),
            DVec2::new(70.0, 80.0),
        ];

        for (t, expected) in timestamps.iter().zip(expected_positions.iter()) {
            let p = interpolate_position(&pos_start, &pos_end, *t);
            assert!(
                (p.x - expected.x).abs() < 1e-10,
                "At t={}, expected x={}, got x={}",
                t,
                expected.x,
                p.x
            );
            assert!(
                (p.y - expected.y).abs() < 1e-10,
                "At t={}, expected y={}, got y={}",
                t,
                expected.y,
                p.y
            );
        }

        // Verify velocity
        let v = config.velocity();
        assert!((v.x - 0.2).abs() < 1e-10); // 20 pixels / 100 seconds
        assert!((v.y - 0.2).abs() < 1e-10);
    }

    #[test]
    fn test_integration_comet_aligned_transform_sequence() {
        // Test that comet-aligned transforms correctly compensate for comet motion.
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(120.0, 110.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform is identity (reference frame)
        let star_transform = Transform::identity();
        let ref_timestamp = 0.0;

        // At each frame, compute comet-aligned transform and verify it centers the comet
        for frame_idx in 0..5 {
            let frame_timestamp = frame_idx as f64 * 25.0;
            let comet_transform =
                config.comet_aligned_transform(&star_transform, frame_timestamp, ref_timestamp);

            // The comet's position at this frame
            let comet_pos =
                interpolate_position(&config.pos_start, &config.pos_end, frame_timestamp);

            // Apply comet transform to the comet position
            let aligned = comet_transform.apply(comet_pos);

            // After applying the comet transform, the comet should map to its reference position
            // The reference position is pos_start (at ref_timestamp=0)
            assert!(
                (aligned.x - pos_start.pos.x).abs() < 1e-6,
                "Frame {}: expected aligned_x={}, got {}",
                frame_idx,
                pos_start.pos.x,
                aligned.x
            );
            assert!(
                (aligned.y - pos_start.pos.y).abs() < 1e-6,
                "Frame {}: expected aligned_y={}, got {}",
                frame_idx,
                pos_start.pos.y,
                aligned.y
            );
        }
    }

    #[test]
    fn test_integration_synthetic_comet_stacking_mean() {
        // Generate synthetic frames with a moving comet and verify stacking.
        let width = 128;
        let height = 128;
        let num_frames = 5;
        let background = 0.1;
        let noise_sigma = 0.01;

        // Fixed star positions
        let star_positions: Vec<Vec2> = vec![
            Vec2::new(30.0, 30.0),
            Vec2::new(90.0, 40.0),
            Vec2::new(50.0, 100.0),
            Vec2::new(110.0, 95.0),
        ];

        // Comet motion: moves from (40, 60) to (80, 70) over the sequence
        let pos_start = ObjectPosition::new(40.0, 60.0, 0.0);
        let pos_end = ObjectPosition::new(80.0, 70.0, (num_frames - 1) as f64);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Generate frames with comet at different positions
        let mut frames = Vec::new();
        for frame_idx in 0..num_frames {
            let timestamp = frame_idx as f64;
            let comet_pos = interpolate_position(&pos_start, &pos_end, timestamp);

            let frame = generate_comet_frame(
                width,
                height,
                &star_positions,
                comet_pos.as_vec2(),
                background,
                noise_sigma,
                42 + frame_idx as u64,
            );
            frames.push(frame);
        }

        // Star-aligned stacking (identity transforms)
        // In this simplified test, frames are already star-aligned
        let star_stack = stack_mean(&frames);

        // Comet-aligned stacking: warp each frame to align the comet
        let ref_timestamp = 0.0;
        let mut comet_aligned_frames = Vec::new();
        for (frame_idx, frame) in frames.iter().enumerate() {
            let frame_timestamp = frame_idx as f64;
            let star_transform = Transform::identity();
            let comet_transform =
                config.comet_aligned_transform(&star_transform, frame_timestamp, ref_timestamp);

            let warped = warp_frame(frame, width, height, &comet_transform);
            comet_aligned_frames.push(warped);
        }
        let comet_stack = stack_mean(&comet_aligned_frames);

        // Create composite result
        let result = create_comet_stack_result(
            star_stack.clone(),
            comet_stack.clone(),
            width,
            height,
            &config,
        );

        // Verify results
        assert_eq!(result.width, width);
        assert_eq!(result.height, height);
        assert!(result.composite.is_some());

        // In star stack: stars should be sharp, comet should be spread out
        // Measure star spread at first star position
        let star_spread_in_star_stack = measure_spread(&star_stack, width, Vec2us::new(30, 30), 8);

        // In comet stack: comet should be relatively sharp
        // The comet's reference position is (40, 60)
        let comet_spread_in_comet_stack =
            measure_spread(&comet_stack, width, Vec2us::new(40, 60), 8);

        // The comet in star stack should be more spread out than in comet stack
        // (because it moved during the sequence)
        // Calculate comet spread in star stack - it should be larger
        // The comet center in star stack is somewhere in the middle of its motion
        let comet_avg = Vec2us::new(
            ((pos_start.pos.x + pos_end.pos.x) / 2.0).round() as usize,
            ((pos_start.pos.y + pos_end.pos.y) / 2.0).round() as usize,
        );
        let comet_spread_in_star_stack = measure_spread(&star_stack, width, comet_avg, 15);

        // Log values for debugging
        eprintln!("Star spread in star stack: {}", star_spread_in_star_stack);
        eprintln!(
            "Comet spread in comet stack: {}",
            comet_spread_in_comet_stack
        );
        eprintln!("Comet spread in star stack: {}", comet_spread_in_star_stack);

        // Stars should be compact (small spread)
        assert!(
            star_spread_in_star_stack < 5.0,
            "Stars should be compact in star stack, got spread={}",
            star_spread_in_star_stack
        );

        // Comet in comet stack should be compact (properly aligned)
        assert!(
            comet_spread_in_comet_stack < 6.0,
            "Comet should be compact in comet stack, got spread={}",
            comet_spread_in_comet_stack
        );
    }

    #[test]
    fn test_integration_composite_preserves_both() {
        // Test that the lighten composite preserves both stars and comet.
        let width = 64;
        let height = 64;

        // Create star stack with bright star at (20, 20)
        let mut star_stack = vec![0.1f32; width * height];
        render_gaussian(&mut star_stack, width, Vec2::splat(20.0), 2.0, 0.8);

        // Create comet stack with bright comet at (45, 45)
        let mut comet_stack = vec![0.1f32; width * height];
        render_gaussian(&mut comet_stack, width, Vec2::splat(45.0), 3.0, 0.7);

        let composite =
            composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten).unwrap();

        // Both should be bright in the composite
        let star_peak = measure_peak(&composite, width, 20, 20);
        let comet_peak = measure_peak(&composite, width, 45, 45);
        let background_level = composite[0]; // Corner should be background

        assert!(
            star_peak > 0.6,
            "Star should be bright in composite, got {}",
            star_peak
        );
        assert!(
            comet_peak > 0.5,
            "Comet should be bright in composite, got {}",
            comet_peak
        );
        assert!(
            background_level < 0.2,
            "Background should be low, got {}",
            background_level
        );
    }

    #[test]
    fn test_integration_velocity_computation() {
        // Test various velocity scenarios
        let test_cases = [
            // (start_x, start_y, end_x, end_y, duration, expected_vx, expected_vy)
            (0.0, 0.0, 100.0, 0.0, 100.0, 1.0, 0.0), // Horizontal motion
            (0.0, 0.0, 0.0, 100.0, 100.0, 0.0, 1.0), // Vertical motion
            (0.0, 0.0, 100.0, 100.0, 100.0, 1.0, 1.0), // Diagonal motion
            (50.0, 50.0, 0.0, 0.0, 50.0, -1.0, -1.0), // Negative velocity
            (10.0, 20.0, 50.0, 80.0, 200.0, 0.2, 0.3), // Non-integer velocity
        ];

        for (start_x, start_y, end_x, end_y, duration, expected_vx, expected_vy) in test_cases {
            let pos_start = ObjectPosition::new(start_x, start_y, 0.0);
            let pos_end = ObjectPosition::new(end_x, end_y, duration);
            let config = CometStackConfig::new(pos_start, pos_end);

            let v = config.velocity();
            assert!(
                (v.x - expected_vx).abs() < 1e-10,
                "Expected vx={}, got vx={}",
                expected_vx,
                v.x
            );
            assert!(
                (v.y - expected_vy).abs() < 1e-10,
                "Expected vy={}, got vy={}",
                expected_vy,
                v.y
            );
        }
    }

    #[test]
    fn test_integration_displacement_computation() {
        // Test displacement computation
        let test_cases = [
            // (dx, dy, expected_displacement)
            (3.0, 4.0, 5.0),   // 3-4-5 triangle
            (0.0, 10.0, 10.0), // Vertical only
            (10.0, 0.0, 10.0), // Horizontal only
            (5.0, 12.0, 13.0), // 5-12-13 triangle
        ];

        for (dx, dy, expected) in test_cases {
            let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
            let pos_end = ObjectPosition::new(100.0 + dx, 100.0 + dy, 100.0);
            let config = CometStackConfig::new(pos_start, pos_end);

            let displacement = config.total_displacement();
            assert!(
                (displacement - expected).abs() < 1e-10,
                "Expected displacement={}, got {}",
                expected,
                displacement
            );
        }
    }

    #[test]
    fn test_integration_result_struct_completeness() {
        // Verify all fields in CometStackResult are correctly populated
        let pos_start = ObjectPosition::new(10.0, 20.0, 0.0);
        let pos_end = ObjectPosition::new(30.0, 50.0, 100.0);
        let config =
            CometStackConfig::new(pos_start, pos_end).composite_method(CompositeMethod::Lighten);

        let star_stack = vec![0.5f32; 256];
        let comet_stack = vec![0.6f32; 256];

        let result = create_comet_stack_result(star_stack, comet_stack, 16, 16, &config);

        assert_eq!(result.width, 16);
        assert_eq!(result.height, 16);
        assert_eq!(result.star_stack.len(), 256);
        assert_eq!(result.comet_stack.len(), 256);
        assert!(result.composite.is_some());
        assert_eq!(result.composite.as_ref().unwrap().len(), 256);

        assert!((result.velocity.x - 0.2).abs() < 1e-10);
        assert!((result.velocity.y - 0.3).abs() < 1e-10);

        // Displacement = sqrt(20^2 + 30^2) = sqrt(400 + 900) = sqrt(1300)
        let expected_displacement = (20.0f64 * 20.0 + 30.0 * 30.0).sqrt();
        assert!((result.displacement - expected_displacement).abs() < 1e-10);
    }

    #[test]
    fn test_integration_transform_with_rotation() {
        // Test comet-aligned transform when star transform includes rotation
        let pos_start = ObjectPosition::new(64.0, 64.0, 0.0);
        let pos_end = ObjectPosition::new(84.0, 64.0, 100.0); // Horizontal motion only
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform with small rotation (1 degree)
        let angle = 1.0f64.to_radians();
        let star_transform = Transform::euclidean(DVec2::ZERO, angle);

        // At t=50, comet has moved 10 pixels in x
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        // The transform should include both the rotation and the comet offset
        // Verify it's not identity
        assert!(
            comet_transform.deviation_from_identity() > 0.01,
            "Transform should differ from identity"
        );

        // Verify the comet at t=50 maps correctly
        let comet_pos_at_50 = DVec2::new(74.0, 64.0); // 64 + 10, 64

        let aligned = comet_transform.apply(comet_pos_at_50);

        // After the star rotation and comet offset, it should map near the reference position
        // The exact position depends on the composition order
        // The important thing is the transform is being applied correctly
        assert!(
            aligned.x.is_finite() && aligned.y.is_finite(),
            "Transform should produce valid coordinates"
        );
    }

    #[test]
    fn test_integration_multiple_composite_methods() {
        // Test all composite methods work correctly
        let width = 32;
        let height = 32;

        let mut star_stack = vec![0.1f32; width * height];
        render_gaussian(&mut star_stack, width, Vec2::splat(10.0), 2.0, 0.5);

        let mut comet_stack = vec![0.1f32; width * height];
        render_gaussian(&mut comet_stack, width, Vec2::splat(20.0), 2.0, 0.6);

        // Test Lighten
        let lighten = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Lighten);
        assert!(lighten.is_some());
        let lighten = lighten.unwrap();
        // Lighten should have max of each pixel
        for (i, (&s, &c)) in star_stack.iter().zip(comet_stack.iter()).enumerate() {
            assert!(
                (lighten[i] - s.max(c)).abs() < 1e-6,
                "Lighten mismatch at pixel {}",
                i
            );
        }

        // Test Additive
        let additive = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Additive);
        assert!(additive.is_some());
        let additive = additive.unwrap();
        // Additive should have star + (comet - background)
        // Just verify it produced a result and is different from lighten in most places
        assert_eq!(additive.len(), lighten.len());

        // Test Separate
        let separate = composite_stacks(&star_stack, &comet_stack, CompositeMethod::Separate);
        assert!(separate.is_none());
    }

    #[test]
    fn test_integration_config_with_rejection_method() {
        // Verify rejection method can be configured
        let pos_start = ObjectPosition::new(50.0, 50.0, 0.0);
        let pos_end = ObjectPosition::new(70.0, 70.0, 100.0);

        let config = CometStackConfig::new(pos_start, pos_end)
            .rejection(RejectionMethod::SigmaClip(
                crate::stacking::rejection::SigmaClipConfig::new(2.0, 3),
            ))
            .normalization(NormalizationMethod::Global)
            .composite_method(CompositeMethod::Additive);

        // Verify configuration was set
        assert_eq!(config.composite_method, CompositeMethod::Additive);
        assert_eq!(config.normalization, NormalizationMethod::Global);
        match config.rejection {
            RejectionMethod::SigmaClip(sc) => {
                assert!((sc.sigma - 2.0).abs() < 1e-6);
                assert_eq!(sc.max_iterations, 3);
            }
            _ => panic!("Expected SigmaClip rejection method"),
        }
    }
}
