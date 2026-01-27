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

use crate::registration::TransformMatrix;
use crate::stacking::local_normalization::NormalizationMethod;
use crate::stacking::weighted::RejectionMethod;

/// Position of a comet or asteroid at a specific timestamp.
///
/// The position is in pixel coordinates relative to the reference frame.
/// Timestamps can be in any consistent unit (seconds, MJD, etc.) as long
/// as all positions in a sequence use the same unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectPosition {
    /// X coordinate in pixels (sub-pixel precision).
    pub x: f64,
    /// Y coordinate in pixels (sub-pixel precision).
    pub y: f64,
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
        Self { x, y, timestamp }
    }

    /// Create position from integer coordinates.
    pub fn from_coords(x: u32, y: u32, timestamp: f64) -> Self {
        Self::new(f64::from(x), f64::from(y), timestamp)
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
    /// Returns (vx, vy) where:
    /// - vx: velocity in x direction (pixels per time unit)
    /// - vy: velocity in y direction (pixels per time unit)
    pub fn velocity(&self) -> (f64, f64) {
        let dt = self.pos_end.timestamp - self.pos_start.timestamp;
        debug_assert!(dt.abs() > f64::EPSILON, "Time delta must be non-zero");

        let vx = (self.pos_end.x - self.pos_start.x) / dt;
        let vy = (self.pos_end.y - self.pos_start.y) / dt;
        (vx, vy)
    }

    /// Compute the total displacement in pixels.
    pub fn total_displacement(&self) -> f64 {
        let dx = self.pos_end.x - self.pos_start.x;
        let dy = self.pos_end.y - self.pos_start.y;
        (dx * dx + dy * dy).sqrt()
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
        star_transform: &TransformMatrix,
        frame_timestamp: f64,
        ref_timestamp: f64,
    ) -> TransformMatrix {
        let (dx, dy) = compute_comet_offset(self, frame_timestamp, ref_timestamp);
        let offset_transform = TransformMatrix::translation(dx, dy);

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
    pub velocity: (f64, f64),

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
/// Interpolated (x, y) position in pixels.
///
/// # Panics
/// Panics if start and end timestamps are equal.
pub fn interpolate_position(
    pos_start: &ObjectPosition,
    pos_end: &ObjectPosition,
    timestamp: f64,
) -> (f64, f64) {
    let dt = pos_end.timestamp - pos_start.timestamp;
    assert!(
        dt.abs() > f64::EPSILON,
        "Start and end timestamps must be different"
    );

    let t_normalized = (timestamp - pos_start.timestamp) / dt;

    let x = pos_start.x + t_normalized * (pos_end.x - pos_start.x);
    let y = pos_start.y + t_normalized * (pos_end.y - pos_start.y);

    (x, y)
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
/// (dx, dy) offset in pixels to add to the star-aligned transform.
pub fn compute_comet_offset(
    config: &CometStackConfig,
    timestamp: f64,
    ref_timestamp: f64,
) -> (f64, f64) {
    let (vx, vy) = config.velocity();
    let dt = timestamp - ref_timestamp;

    // The comet has moved by (vx*dt, vy*dt) relative to the reference frame.
    // To align ON the comet, we need to shift back by this amount.
    (-vx * dt, -vy * dt)
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
    star_transform: &TransformMatrix,
    config: &CometStackConfig,
    frame_timestamp: f64,
    ref_timestamp: f64,
) -> TransformMatrix {
    config.comet_aligned_transform(star_transform, frame_timestamp, ref_timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== ObjectPosition Tests ==========

    #[test]
    fn test_object_position_new() {
        let pos = ObjectPosition::new(100.5, 200.75, 1000.0);
        assert!((pos.x - 100.5).abs() < f64::EPSILON);
        assert!((pos.y - 200.75).abs() < f64::EPSILON);
        assert!((pos.timestamp - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_object_position_from_coords() {
        let pos = ObjectPosition::from_coords(100, 200, 1000.0);
        assert!((pos.x - 100.0).abs() < f64::EPSILON);
        assert!((pos.y - 200.0).abs() < f64::EPSILON);
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

        let (vx, vy) = config.velocity();
        // 10 pixels / 3600 seconds = 0.00277... pixels/second
        assert!((vx - 10.0 / 3600.0).abs() < 1e-10);
        assert!((vy - 5.0 / 3600.0).abs() < 1e-10);
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

        let (x, y) = interpolate_position(&pos_start, &pos_end, 0.0);
        assert!((x - 100.0).abs() < 1e-10);
        assert!((y - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_at_end() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(150.0, 250.0, 100.0);

        let (x, y) = interpolate_position(&pos_start, &pos_end, 100.0);
        assert!((x - 150.0).abs() < 1e-10);
        assert!((y - 250.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_at_middle() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 100.0);

        let (x, y) = interpolate_position(&pos_start, &pos_end, 50.0);
        assert!((x - 150.0).abs() < 1e-10);
        assert!((y - 250.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_extrapolate_before() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 100.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 200.0);

        let (x, y) = interpolate_position(&pos_start, &pos_end, 50.0);
        // Extrapolate backwards: t=-0.5 normalized
        assert!((x - 50.0).abs() < 1e-10);
        assert!((y - 150.0).abs() < 1e-10);
    }

    #[test]
    fn test_interpolate_position_extrapolate_after() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 300.0, 100.0);

        let (x, y) = interpolate_position(&pos_start, &pos_end, 150.0);
        // Extrapolate forwards: t=1.5 normalized
        assert!((x - 250.0).abs() < 1e-10);
        assert!((y - 350.0).abs() < 1e-10);
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

        let (dx, dy) = compute_comet_offset(&config, 0.0, 0.0);
        assert!((dx - 0.0).abs() < 1e-10);
        assert!((dy - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_comet_offset_after_time() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0); // 0.1 px/s in x, 0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // After 50 seconds, comet moved 5 pixels in x, 10 pixels in y
        // To align ON comet, we need to shift back
        let (dx, dy) = compute_comet_offset(&config, 50.0, 0.0);
        assert!((dx - (-5.0)).abs() < 1e-10);
        assert!((dy - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_compute_comet_offset_negative_velocity() {
        let pos_start = ObjectPosition::new(110.0, 220.0, 0.0);
        let pos_end = ObjectPosition::new(100.0, 200.0, 100.0); // -0.1 px/s in x, -0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // After 50 seconds, comet moved -5 pixels in x, -10 pixels in y
        // To align ON comet, we shift back by negative offset = positive
        let (dx, dy) = compute_comet_offset(&config, 50.0, 0.0);
        assert!((dx - 5.0).abs() < 1e-10);
        assert!((dy - 10.0).abs() < 1e-10);
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
            velocity: (0.1, 0.2),
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
        let star_transform = TransformMatrix::identity();

        // At reference timestamp, comet offset should be zero
        let comet_transform = config.comet_aligned_transform(&star_transform, 0.0, 0.0);

        // Result should be identity (no offset needed)
        let (tx, ty) = comet_transform.translation_components();
        assert!((tx - 0.0).abs() < 1e-10);
        assert!((ty - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_after_time() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0); // 0.1 px/s in x, 0.2 px/s in y
        let config = CometStackConfig::new(pos_start, pos_end);

        // Identity star transform
        let star_transform = TransformMatrix::identity();

        // After 50 seconds, comet moved 5 pixels in x, 10 pixels in y
        // To align ON comet, we need offset (-5, -10)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        let (tx, ty) = comet_transform.translation_components();
        assert!((tx - (-5.0)).abs() < 1e-10);
        assert!((ty - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_with_translation() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform with existing translation
        let star_transform = TransformMatrix::translation(10.0, 20.0);

        // After 50 seconds, comet offset is (-5, -10)
        // Combined with star translation (10, 20), result should be (5, 10)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        let (tx, ty) = comet_transform.translation_components();
        assert!((tx - 5.0).abs() < 1e-10);
        assert!((ty - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_with_rotation() {
        let pos_start = ObjectPosition::new(0.0, 0.0, 0.0);
        let pos_end = ObjectPosition::new(100.0, 0.0, 100.0); // 1 px/s in x only
        let config = CometStackConfig::new(pos_start, pos_end);

        // Star transform with 90 degree rotation
        let angle = std::f64::consts::FRAC_PI_2;
        let star_transform = TransformMatrix::euclidean(0.0, 0.0, angle);

        // After 50 seconds, comet offset is (-50, 0)
        // But the compose operation applies star_transform first, then offset
        // So the offset is NOT rotated by the star transform
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        // Apply both transforms to origin to verify
        let (x, y) = comet_transform.apply(0.0, 0.0);
        // Star transform: (0,0) -> (0,0) (rotation around origin)
        // Then offset: (0,0) + (-50,0) = (-50,0)
        assert!((x - (-50.0)).abs() < 1e-10);
        assert!((y - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_comet_offset_to_transform() {
        let pos_start = ObjectPosition::new(100.0, 200.0, 0.0);
        let pos_end = ObjectPosition::new(110.0, 220.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_transform = TransformMatrix::identity();

        // Test that the standalone function gives same result as method
        let via_method = config.comet_aligned_transform(&star_transform, 50.0, 0.0);
        let via_function = apply_comet_offset_to_transform(&star_transform, &config, 50.0, 0.0);

        let (tx1, ty1) = via_method.translation_components();
        let (tx2, ty2) = via_function.translation_components();

        assert!((tx1 - tx2).abs() < 1e-10);
        assert!((ty1 - ty2).abs() < 1e-10);
    }

    #[test]
    fn test_comet_aligned_transform_point_mapping() {
        let pos_start = ObjectPosition::new(100.0, 100.0, 0.0);
        let pos_end = ObjectPosition::new(200.0, 150.0, 100.0);
        let config = CometStackConfig::new(pos_start, pos_end);

        let star_transform = TransformMatrix::translation(5.0, 10.0);

        // Frame at t=50: comet has moved (50, 25) pixels from start
        // Comet offset: (-50, -25)
        // Combined: (5, 10) + (-50, -25) = (-45, -15)
        let comet_transform = config.comet_aligned_transform(&star_transform, 50.0, 0.0);

        // A point at (0, 0) should map to (-45, -15)
        let (x, y) = comet_transform.apply(0.0, 0.0);
        assert!((x - (-45.0)).abs() < 1e-10);
        assert!((y - (-15.0)).abs() < 1e-10);
    }
}
