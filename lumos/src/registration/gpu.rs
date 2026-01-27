//! GPU-accelerated warping for image registration.
//!
//! This module provides GPU-accelerated image warping using imaginarium's
//! compute shader infrastructure. It's significantly faster than CPU warping
//! for large images.

use imaginarium::{
    Affine2, ColorFormat, FilterMode, Image, ImageBuffer, ImageDesc, ProcessingContext, Transform,
    Vec2,
};

use super::types::TransformMatrix;

/// Convert a lumos TransformMatrix to an imaginarium Affine2.
///
/// Note: This only works correctly for affine transforms (Translation, Euclidean,
/// Similarity, Affine). Homography transforms have perspective components that
/// cannot be represented by Affine2.
fn transform_to_affine2(transform: &TransformMatrix) -> Affine2 {
    let d = &transform.data;
    // TransformMatrix is row-major:
    // [a, b, tx]    [d[0], d[1], d[2]]
    // [c, d, ty] =  [d[3], d[4], d[5]]
    // [0, 0, 1 ]    [d[6], d[7], d[8]]
    //
    // Affine2 uses column-major Mat2 + translation Vec2
    // Mat2 columns: col0 = (a, c), col1 = (b, d)
    Affine2::from_mat2_translation(
        glam::Mat2::from_cols(
            Vec2::new(d[0] as f32, d[3] as f32),
            Vec2::new(d[1] as f32, d[4] as f32),
        ),
        Vec2::new(d[2] as f32, d[5] as f32),
    )
}

/// GPU warping context that caches the processing context and pipeline.
#[derive(Debug)]
pub struct GpuWarper {
    ctx: ProcessingContext,
}

impl GpuWarper {
    /// Create a new GPU warper.
    ///
    /// This initializes the GPU context and may take some time on first call.
    pub fn new() -> Self {
        Self {
            ctx: ProcessingContext::new(),
        }
    }

    /// Warp a single-channel f32 image using GPU.
    ///
    /// # Arguments
    ///
    /// * `input` - Input image data (row-major, single channel f32)
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `transform` - Transformation from reference to target coordinates
    ///
    /// # Returns
    ///
    /// Warped image data as Vec<f32>
    pub fn warp_channel(
        &mut self,
        input: &[f32],
        width: usize,
        height: usize,
        transform: &TransformMatrix,
    ) -> Vec<f32> {
        // Create input image from f32 data
        let input_bytes: Vec<u8> = bytemuck::cast_slice(input).to_vec();
        let input_desc = ImageDesc {
            width,
            height,
            stride: width * 4, // f32 = 4 bytes
            color_format: ColorFormat::L_F32,
        };
        let input_image =
            Image::new_with_data(input_desc, input_bytes).expect("Failed to create input image");

        // Create output image
        let output_desc = ImageDesc {
            width,
            height,
            stride: width * 4,
            color_format: ColorFormat::L_F32,
        };

        // Wrap in ImageBuffers
        let input_buffer = ImageBuffer::from_cpu(input_image);
        let mut output_buffer = ImageBuffer::new_empty(output_desc);

        // Convert transform - we need the inverse for backward mapping
        // warp_to_reference passes transform that maps ref->target
        // GPU shader needs inverse (target->ref) for sampling
        let inverse = transform.inverse();
        let affine = transform_to_affine2(&inverse);

        // Create transform operation with bilinear filtering
        let op = Transform::new().affine(affine).filter(FilterMode::Bilinear);

        // Execute on GPU
        op.execute(&mut self.ctx, &input_buffer, &mut output_buffer)
            .expect("GPU warp failed");

        // Extract result - download from GPU to CPU
        let result_image = output_buffer
            .to_cpu(&self.ctx)
            .expect("Failed to download result");
        let result_bytes = result_image.bytes();
        bytemuck::cast_slice(result_bytes).to_vec()
    }

    /// Warp an RGB f32 image using GPU.
    ///
    /// # Arguments
    ///
    /// * `input` - Input image data (row-major, RGB interleaved f32)
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `transform` - Transformation from reference to target coordinates
    ///
    /// # Returns
    ///
    /// Warped image data as Vec<f32> (RGB interleaved)
    pub fn warp_rgb(
        &mut self,
        input: &[f32],
        width: usize,
        height: usize,
        transform: &TransformMatrix,
    ) -> Vec<f32> {
        assert_eq!(
            input.len(),
            width * height * 3,
            "Input size mismatch for RGB image"
        );

        // Create input image from f32 RGB data
        let input_bytes: Vec<u8> = bytemuck::cast_slice(input).to_vec();
        let input_desc = ImageDesc {
            width,
            height,
            stride: width * 12, // RGB_F32 = 12 bytes per pixel
            color_format: ColorFormat::RGB_F32,
        };
        let input_image =
            Image::new_with_data(input_desc, input_bytes).expect("Failed to create input image");

        // Create output image
        let output_desc = ImageDesc {
            width,
            height,
            stride: width * 12,
            color_format: ColorFormat::RGB_F32,
        };

        // Wrap in ImageBuffers
        let input_buffer = ImageBuffer::from_cpu(input_image);
        let mut output_buffer = ImageBuffer::new_empty(output_desc);

        // Convert transform
        let inverse = transform.inverse();
        let affine = transform_to_affine2(&inverse);

        // Create transform operation
        let op = Transform::new().affine(affine).filter(FilterMode::Bilinear);

        // Execute on GPU
        op.execute(&mut self.ctx, &input_buffer, &mut output_buffer)
            .expect("GPU warp failed");

        // Extract result - download from GPU to CPU
        let result_image = output_buffer
            .to_cpu(&self.ctx)
            .expect("Failed to download result");
        let result_bytes = result_image.bytes();
        bytemuck::cast_slice(result_bytes).to_vec()
    }
}

impl Default for GpuWarper {
    fn default() -> Self {
        Self::new()
    }
}

/// Warp a multi-channel image with parallel channel processing (CPU).
///
/// This function processes each channel in parallel using rayon, which is
/// faster than sequential processing for multi-channel images.
///
/// # Arguments
///
/// * `input` - Input image data (row-major, channels interleaved)
/// * `width` - Image width
/// * `height` - Image height
/// * `channels` - Number of channels
/// * `transform` - Transformation from reference to target coordinates
/// * `method` - Interpolation method
///
/// # Returns
///
/// Warped image data with channels interleaved.
pub fn warp_multichannel_parallel(
    input: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    transform: &TransformMatrix,
    method: super::interpolation::InterpolationMethod,
) -> Vec<f32> {
    use super::pipeline::warp_to_reference;
    use rayon::prelude::*;

    assert_eq!(
        input.len(),
        width * height * channels,
        "Input size mismatch"
    );

    if channels == 1 {
        // Single channel - just use regular warp
        return warp_to_reference(input, width, height, transform, method);
    }

    // Extract channels
    let channel_data: Vec<Vec<f32>> = (0..channels)
        .map(|c| input.iter().skip(c).step_by(channels).copied().collect())
        .collect();

    // Warp channels in parallel
    let warped_channels: Vec<Vec<f32>> = channel_data
        .par_iter()
        .map(|channel| warp_to_reference(channel, width, height, transform, method))
        .collect();

    // Interleave channels back
    let mut result = vec![0.0f32; input.len()];
    for c in 0..channels {
        for (i, &val) in warped_channels[c].iter().enumerate() {
            result[i * channels + c] = val;
        }
    }

    result
}

/// Warp target image to align with reference using GPU acceleration.
///
/// This is a convenience function that creates a temporary GPU context.
/// For multiple warps, use `GpuWarper` directly to reuse the context.
///
/// # Arguments
///
/// * `target_image` - Target image pixel data (single channel f32)
/// * `width` - Image width
/// * `height` - Image height
/// * `transform` - Transformation from reference to target coordinates
///
/// # Returns
///
/// Warped image aligned to reference frame.
pub fn warp_to_reference_gpu(
    target_image: &[f32],
    width: usize,
    height: usize,
    transform: &TransformMatrix,
) -> Vec<f32> {
    let mut warper = GpuWarper::new();
    warper.warp_channel(target_image, width, height, transform)
}

/// Warp RGB target image to align with reference using GPU acceleration.
///
/// # Arguments
///
/// * `target_image` - Target image pixel data (RGB interleaved f32)
/// * `width` - Image width
/// * `height` - Image height
/// * `transform` - Transformation from reference to target coordinates
///
/// # Returns
///
/// Warped RGB image aligned to reference frame.
pub fn warp_rgb_to_reference_gpu(
    target_image: &[f32],
    width: usize,
    height: usize,
    transform: &TransformMatrix,
) -> Vec<f32> {
    let mut warper = GpuWarper::new();
    warper.warp_rgb(target_image, width, height, transform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_to_affine2_identity() {
        let identity = TransformMatrix::identity();
        let affine = transform_to_affine2(&identity);

        // Should be identity transform
        let (x, y) = (100.0f32, 200.0f32);
        let result = affine.transform_point2(Vec2::new(x, y));
        assert!((result.x - x).abs() < 1e-5);
        assert!((result.y - y).abs() < 1e-5);
    }

    #[test]
    fn test_transform_to_affine2_translation() {
        let transform = TransformMatrix::translation(10.0, 20.0);
        let affine = transform_to_affine2(&transform);

        let (x, y) = (100.0f32, 200.0f32);
        let result = affine.transform_point2(Vec2::new(x, y));
        assert!((result.x - 110.0).abs() < 1e-5);
        assert!((result.y - 220.0).abs() < 1e-5);
    }

    #[test]
    fn test_transform_to_affine2_similarity() {
        // Rotation of 45 degrees with scale 2
        let angle = std::f64::consts::FRAC_PI_4;
        let transform = TransformMatrix::similarity(0.0, 0.0, angle, 2.0);
        let affine = transform_to_affine2(&transform);

        // Point (1, 0) should map to (sqrt(2), sqrt(2)) after 45deg rotation and 2x scale
        let result = affine.transform_point2(Vec2::new(1.0, 0.0));
        let expected = 2.0_f32 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((result.x - expected).abs() < 1e-5);
        assert!((result.y - expected).abs() < 1e-5);
    }

    #[test]
    fn test_gpu_warp_identity() {
        // Create a simple test image
        let width = 64;
        let height = 64;
        let mut input = vec![0.0f32; width * height];

        // Draw a pattern
        for y in 0..height {
            for x in 0..width {
                input[y * width + x] = ((x + y) % 2) as f32;
            }
        }

        let mut warper = GpuWarper::new();
        let identity = TransformMatrix::identity();
        let output = warper.warp_channel(&input, width, height, &identity);

        assert_eq!(output.len(), input.len());

        // With identity transform, output should match input (within interpolation tolerance)
        let mut max_diff = 0.0f32;
        for (&inp, &out) in input.iter().zip(output.iter()) {
            let diff = (inp - out).abs();
            if diff > max_diff {
                max_diff = diff;
            }
        }
        // Allow some tolerance due to bilinear interpolation at pixel centers
        assert!(
            max_diff < 0.1,
            "Max difference {} too large for identity transform",
            max_diff
        );
    }
}
