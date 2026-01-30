//! GPU-accelerated threshold mask creation for star detection.
//!
//! Creates binary masks of pixels above detection threshold using GPU compute.
//! Uses a hybrid approach: GPU for threshold mask creation, CPU for connected
//! component labeling (optimal for sparse astronomical images).

// This module provides an optional GPU-accelerated path for star detection.
// Some items are only used by public API and tests, not internal code.
#![allow(dead_code)]

use bytemuck::{Pod, Zeroable};
use imaginarium::ProcessingContext;
use wgpu::util::DeviceExt;

use super::pipeline::{GpuDilateMaskPipeline, GpuThresholdMaskPipeline};
use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::StarDetectionConfig;
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::deblend::DeblendConfig;
use crate::star_detection::detection::{
    DetectionConfig, LabelMap, StarCandidate, extract_candidates,
};

/// Maximum dilation radius supported by the GPU shader.
pub const MAX_DILATION_RADIUS: u32 = 8;

/// Configuration for GPU threshold detection.
#[derive(Debug, Clone, Copy)]
pub struct GpuThresholdConfig {
    /// Detection threshold in standard deviations above background (default: 4.0).
    pub sigma_threshold: f32,
    /// Dilation radius in pixels to connect fragmented detections (default: 1).
    pub dilation_radius: u32,
}

impl Default for GpuThresholdConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 4.0,
            dilation_radius: 1,
        }
    }
}

impl GpuThresholdConfig {
    /// Create a new config with the given sigma threshold.
    pub fn new(sigma_threshold: f32, dilation_radius: u32) -> Self {
        assert!(sigma_threshold > 0.0, "sigma_threshold must be positive");
        assert!(
            dilation_radius <= MAX_DILATION_RADIUS,
            "dilation_radius must be <= {}, got {}",
            MAX_DILATION_RADIUS,
            dilation_radius
        );
        Self {
            sigma_threshold,
            dilation_radius,
        }
    }
}

/// GPU shader parameters for threshold mask creation.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ThresholdParams {
    width: u32,
    height: u32,
    sigma_threshold: f32,
    _padding: u32,
}

/// GPU shader parameters for mask dilation.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DilateParams {
    width: u32,
    height: u32,
    radius: u32,
    mask_width_u32: u32,
}

/// GPU-accelerated threshold detector.
///
/// Creates binary masks of pixels above the detection threshold.
/// Reuses the GPU context across multiple operations.
#[derive(Debug)]
pub struct GpuThresholdDetector {
    ctx: ProcessingContext,
}

impl GpuThresholdDetector {
    /// Create a new GPU threshold detector.
    pub fn new() -> Self {
        Self {
            ctx: ProcessingContext::new(),
        }
    }

    /// Check if GPU is available.
    pub fn gpu_available(&self) -> bool {
        self.ctx.gpu().is_some()
    }

    /// Create threshold mask on GPU.
    ///
    /// Returns a boolean mask where `true` indicates pixels above the threshold.
    ///
    /// # Arguments
    ///
    /// * `pixels` - Image pixel data (width × height f32 values)
    /// * `background` - Background map with per-pixel background and noise
    /// * `config` - Threshold configuration
    ///
    /// # Panics
    ///
    /// Panics if no GPU is available or if dimensions don't match.
    pub fn create_mask(
        &mut self,
        pixels: &[f32],
        background: &BackgroundMap,
        config: &GpuThresholdConfig,
    ) -> Vec<bool> {
        let width = background.width();
        let height = background.height();

        assert_eq!(
            pixels.len(),
            width * height,
            "Pixel data length doesn't match background dimensions"
        );
        assert_eq!(
            background.background.len(),
            width * height,
            "Background data length mismatch"
        );
        assert_eq!(
            background.noise.len(),
            width * height,
            "Noise data length mismatch"
        );
        debug_assert_eq!(background.background.width(), width);
        debug_assert_eq!(background.noise.width(), width);

        // Get GPU context and pipeline together to manage borrow lifetimes
        let gpu_ctx = self.ctx.gpu_context().expect("GPU context not available");

        // Clone the GPU (cheap, uses Arc internally) and get pipelines
        let gpu = gpu_ctx.gpu().clone();
        let threshold_pipeline = gpu_ctx
            .get_or_create(|gpu| Ok(GpuThresholdMaskPipeline::new(gpu)))
            .expect("Failed to create threshold mask pipeline");

        let device = gpu.device();
        let queue = gpu.queue();

        // Calculate packed mask dimensions
        let mask_width_u32 = width.div_ceil(32);
        let mask_size_u32 = mask_width_u32 * height;

        // Create GPU buffers for threshold operation
        let pixels_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("threshold_pixels_buffer"),
            contents: bytemuck::cast_slice(pixels),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let background_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("threshold_background_buffer"),
            contents: bytemuck::cast_slice(background.background.pixels()),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let noise_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("threshold_noise_buffer"),
            contents: bytemuck::cast_slice(&background.noise),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Initialize mask to zero
        let mask_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("threshold_mask_buffer"),
            size: (mask_size_u32 * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let threshold_params = ThresholdParams {
            width: width as u32,
            height: height as u32,
            sigma_threshold: config.sigma_threshold,
            _padding: 0,
        };

        let threshold_params_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("threshold_params_buffer"),
                contents: bytemuck::bytes_of(&threshold_params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // Create threshold bind group
        let threshold_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("threshold_bind_group"),
            layout: &threshold_pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: threshold_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixels_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: background_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: noise_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: mask_buffer.as_entire_binding(),
                },
            ],
        });

        // Encode threshold compute pass
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("threshold_encoder"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("threshold_pass"),
                timestamp_writes: None,
            });

            pass.set_pipeline(&threshold_pipeline.pipeline);
            pass.set_bind_group(0, &threshold_bind_group, &[]);

            let workgroups_x = width.div_ceil(16) as u32;
            let workgroups_y = height.div_ceil(16) as u32;
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Submit threshold pass
        queue.submit(std::iter::once(encoder.finish()));
        gpu.wait();

        // Apply dilation if radius > 0
        let final_mask_buffer = if config.dilation_radius > 0 {
            let dilate_pipeline = gpu_ctx
                .get_or_create(|gpu| Ok(GpuDilateMaskPipeline::new(gpu)))
                .expect("Failed to create dilate mask pipeline");

            // Create output buffer for dilation
            let dilated_mask_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dilate_mask_out_buffer"),
                size: (mask_size_u32 * std::mem::size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });

            let dilate_params = DilateParams {
                width: width as u32,
                height: height as u32,
                radius: config.dilation_radius,
                mask_width_u32: mask_width_u32 as u32,
            };

            let dilate_params_buffer =
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("dilate_params_buffer"),
                    contents: bytemuck::bytes_of(&dilate_params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

            let dilate_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dilate_bind_group"),
                layout: &dilate_pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: dilate_params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: mask_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dilated_mask_buffer.as_entire_binding(),
                    },
                ],
            });

            // Encode dilation compute pass
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dilate_encoder"),
            });

            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("dilate_pass"),
                    timestamp_writes: None,
                });

                pass.set_pipeline(&dilate_pipeline.pipeline);
                pass.set_bind_group(0, &dilate_bind_group, &[]);

                let workgroups_x = width.div_ceil(16) as u32;
                let workgroups_y = height.div_ceil(16) as u32;
                pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
            }

            queue.submit(std::iter::once(encoder.finish()));
            gpu.wait();

            dilated_mask_buffer
        } else {
            mask_buffer
        };

        // Read back results
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask_staging_buffer"),
            size: (mask_size_u32 * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback_encoder"),
        });

        encoder.copy_buffer_to_buffer(
            &final_mask_buffer,
            0,
            &staging_buffer,
            0,
            (mask_size_u32 * std::mem::size_of::<u32>()) as u64,
        );

        queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait();

        let data = buffer_slice.get_mapped_range();
        let packed_mask: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        // Unpack to boolean vector
        unpack_mask(&packed_mask, width, height)
    }
}

impl Default for GpuThresholdDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Unpack a bit-packed mask into a boolean vector.
fn unpack_mask(packed: &[u32], width: usize, height: usize) -> Vec<bool> {
    let mask_width_u32 = width.div_ceil(32);
    let mut result = vec![false; width * height];

    for y in 0..height {
        for x in 0..width {
            let bit_position = x % 32;
            let mask_idx = y * mask_width_u32 + (x / 32);
            result[y * width + x] = (packed[mask_idx] & (1 << bit_position)) != 0;
        }
    }

    result
}

/// Detect star candidates using GPU-accelerated threshold detection.
///
/// Uses a hybrid approach:
/// 1. GPU: Threshold mask creation and dilation
/// 2. CPU: Connected component labeling (optimal for sparse astronomical images)
///
/// This function returns the same results as `detect_stars()` but uses the GPU
/// for the threshold mask creation phase, which is the main per-pixel operation.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Background map from `estimate_background`
/// * `config` - Star detection configuration
///
/// # Returns
/// Vector of star candidates for further processing (centroiding, etc.)
///
/// # Panics
/// Panics if no GPU is available.
pub fn detect_stars_gpu(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let mut detector = GpuThresholdDetector::new();
    detect_stars_gpu_with_detector(&mut detector, pixels, width, height, background, config)
}

/// Detect star candidates using GPU with a reusable detector.
///
/// Same as `detect_stars_gpu` but allows reusing the GPU context across
/// multiple detections for better performance.
///
/// # Arguments
/// * `detector` - Reusable GPU threshold detector
/// * `pixels` - Image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Background map from `estimate_background`
/// * `config` - Star detection configuration
///
/// # Returns
/// Vector of star candidates for further processing.
pub fn detect_stars_gpu_with_detector(
    detector: &mut GpuThresholdDetector,
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    assert!(
        detector.gpu_available(),
        "GPU not available for star detection"
    );

    let detection_config = DetectionConfig::from(config);

    // Create GPU threshold config from star detection config
    let gpu_config = GpuThresholdConfig::new(
        detection_config.sigma_threshold,
        1, // Dilation radius 1 (3x3) - same as CPU version
    );

    // Create threshold mask on GPU (already includes dilation)
    let mask_vec = detector.create_mask(pixels, background, &gpu_config);

    // Convert Vec<bool> to BitBuffer2 for connected components
    let mut mask = BitBuffer2::new_filled(width, height, false);
    for (i, &v) in mask_vec.iter().enumerate() {
        if v {
            mask.set(i, true);
        }
    }

    // Connected component labeling on CPU (optimal for sparse images)
    let label_map = LabelMap::from_mask(&mask);

    // Extract candidate properties with deblending
    let deblend_config = DeblendConfig::from(config);
    let pixels_buf = Buffer2::new(width, height, pixels.to_vec());
    let mut candidates = extract_candidates(
        &pixels_buf,
        &label_map,
        &deblend_config,
        detection_config.max_area,
    );

    // Apply size and edge filters
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            // Edge filter
            && c.bbox.x_min >= detection_config.edge_margin
            && c.bbox.y_min >= detection_config.edge_margin
            && c.bbox.x_max < width - detection_config.edge_margin
            && c.bbox.y_max < height - detection_config.edge_margin
    });

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star_detection::BackgroundConfig;
    use crate::testing::synthetic::background_map;

    fn test_gpu_available() -> bool {
        let detector = GpuThresholdDetector::new();
        detector.gpu_available()
    }

    #[test]
    fn test_config_default() {
        let config = GpuThresholdConfig::default();
        assert!((config.sigma_threshold - 4.0).abs() < f32::EPSILON);
        assert_eq!(config.dilation_radius, 1);
    }

    #[test]
    fn test_config_new() {
        let config = GpuThresholdConfig::new(3.0, 2);
        assert!((config.sigma_threshold - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.dilation_radius, 2);
    }

    #[test]
    #[should_panic(expected = "sigma_threshold must be positive")]
    fn test_config_zero_sigma_panics() {
        GpuThresholdConfig::new(0.0, 1);
    }

    #[test]
    #[should_panic(expected = "dilation_radius must be")]
    fn test_config_large_radius_panics() {
        GpuThresholdConfig::new(4.0, 100);
    }

    #[test]
    fn test_unpack_mask_simple() {
        // 4x2 image packed into 2 u32s (one per row)
        let packed = vec![
            0b1010u32, // row 0: pixels 0,2 are false; 1,3 are true -> bits 1,3 set
            0b0101u32, // row 1: pixels 0,2 are true; 1,3 are false -> bits 0,2 set
        ];

        let mask = unpack_mask(&packed, 4, 2);
        assert_eq!(mask.len(), 8);

        // Row 0: bits 0,2 are 0; bits 1,3 are 1
        assert!(!mask[0]); // x=0
        assert!(mask[1]); // x=1
        assert!(!mask[2]); // x=2
        assert!(mask[3]); // x=3

        // Row 1: bits 0,2 are 1; bits 1,3 are 0
        assert!(mask[4]); // x=0
        assert!(!mask[5]); // x=1
        assert!(mask[6]); // x=2
        assert!(!mask[7]); // x=3
    }

    #[test]
    fn test_create_mask_uniform_background() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut detector = GpuThresholdDetector::new();
        let config = GpuThresholdConfig::new(3.0, 0); // No dilation

        let width = 8;
        let height = 8;

        // Create test data
        // Background = 0.1, noise = 0.01
        // Threshold = 0.1 + 3.0 * 0.01 = 0.13
        // Pixel at (3,3) = 0.2 should be above threshold
        let mut pixels = vec![0.1f32; width * height];
        pixels[3 * width + 3] = 0.2; // Bright pixel

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let mask = detector.create_mask(&pixels, &background, &config);

        assert_eq!(mask.len(), width * height);

        // Only pixel (3,3) should be above threshold
        for y in 0..height {
            for x in 0..width {
                let expected = x == 3 && y == 3;
                assert_eq!(
                    mask[y * width + x],
                    expected,
                    "Mismatch at ({}, {}): expected {}, got {}",
                    x,
                    y,
                    expected,
                    mask[y * width + x]
                );
            }
        }
    }

    #[test]
    fn test_create_mask_with_dilation() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut detector = GpuThresholdDetector::new();
        let config = GpuThresholdConfig::new(3.0, 1); // Radius 1 dilation

        let width = 8;
        let height = 8;

        // Create test data with single bright pixel at (4,4)
        let mut pixels = vec![0.1f32; width * height];
        pixels[4 * width + 4] = 0.2;

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let mask = detector.create_mask(&pixels, &background, &config);

        // With dilation radius 1, the 3x3 area around (4,4) should be true
        for y in 0..height {
            for x in 0..width {
                let expected = (3..=5).contains(&x) && (3..=5).contains(&y);
                assert_eq!(
                    mask[y * width + x],
                    expected,
                    "Mismatch at ({}, {}): expected {}, got {}",
                    x,
                    y,
                    expected,
                    mask[y * width + x]
                );
            }
        }
    }

    #[test]
    fn test_create_mask_all_below_threshold() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut detector = GpuThresholdDetector::new();
        let config = GpuThresholdConfig::new(5.0, 0);

        let width = 16;
        let height = 16;

        // All pixels at background level
        let pixels = vec![0.1f32; width * height];
        let background = background_map::uniform(width, height, 0.1, 0.01);

        let mask = detector.create_mask(&pixels, &background, &config);

        // No pixels should be above threshold
        assert!(mask.iter().all(|&v| !v), "Expected all pixels to be false");
    }

    #[test]
    fn test_create_mask_large_image() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut detector = GpuThresholdDetector::new();
        let config = GpuThresholdConfig::new(4.0, 1);

        let width = 512;
        let height = 512;

        // Create sparse bright pixels
        let mut pixels = vec![0.1f32; width * height];
        pixels[100 * width + 100] = 0.5;
        pixels[200 * width + 200] = 0.5;
        pixels[300 * width + 300] = 0.5;

        let background = background_map::uniform(width, height, 0.1, 0.02);

        let mask = detector.create_mask(&pixels, &background, &config);

        assert_eq!(mask.len(), width * height);

        // Check that bright pixels and their dilation neighborhoods are set
        // (100,100) should have a 3x3 dilated region
        assert!(mask[100 * width + 100], "Center pixel should be true");
        assert!(mask[99 * width + 100], "Dilated pixel should be true");
        assert!(mask[101 * width + 100], "Dilated pixel should be true");
    }

    #[test]
    fn test_detect_stars_gpu_single_star() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let width = 64;
        let height = 64;

        // Create a single bright star at (32, 32)
        let mut pixels = vec![0.1f32; width * height];
        // Create a small star-like profile (3x3 bright region)
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = (32 + dx) as usize;
                let y = (32 + dy) as usize;
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                let value = 0.5 - 0.1 * dist; // Brighter center
                pixels[y * width + x] = value;
            }
        }

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let config = StarDetectionConfig {
            min_area: 3,
            max_area: 50,
            edge_margin: 5,
            background_config: BackgroundConfig {
                detection_sigma: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let candidates = detect_stars_gpu(&pixels, width, height, &background, &config);

        // Should detect exactly one candidate
        assert_eq!(
            candidates.len(),
            1,
            "Expected 1 candidate, got {}",
            candidates.len()
        );

        let star = &candidates[0];
        // Peak should be near (32, 32)
        assert!(
            (star.peak_x as i32 - 32).abs() <= 1,
            "Peak X ({}) not near 32",
            star.peak_x
        );
        assert!(
            (star.peak_y as i32 - 32).abs() <= 1,
            "Peak Y ({}) not near 32",
            star.peak_y
        );
    }

    #[test]
    fn test_detect_stars_gpu_multiple_stars() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let width = 128;
        let height = 128;

        // Create multiple stars at different positions
        let mut pixels = vec![0.1f32; width * height];

        let star_positions: [(usize, usize); 3] = [(30, 30), (80, 40), (60, 90)];

        for &(cx, cy) in &star_positions {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let x = (cx as i32 + dx) as usize;
                    let y = (cy as i32 + dy) as usize;
                    pixels[y * width + x] = 0.4;
                }
            }
        }

        let background = background_map::uniform(width, height, 0.1, 0.02);

        let config = StarDetectionConfig {
            min_area: 3,
            max_area: 50,
            edge_margin: 5,
            background_config: BackgroundConfig {
                detection_sigma: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let candidates = detect_stars_gpu(&pixels, width, height, &background, &config);

        // Should detect three separate candidates
        assert_eq!(
            candidates.len(),
            3,
            "Expected 3 candidates, got {}",
            candidates.len()
        );
    }

    #[test]
    fn test_detect_stars_gpu_no_stars() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let width = 64;
        let height = 64;

        // All pixels at background level
        let pixels = vec![0.1f32; width * height];

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let config = StarDetectionConfig {
            min_area: 3,
            max_area: 50,
            edge_margin: 5,
            background_config: BackgroundConfig {
                detection_sigma: 4.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let candidates = detect_stars_gpu(&pixels, width, height, &background, &config);

        // Should detect no candidates
        assert!(
            candidates.is_empty(),
            "Expected 0 candidates, got {}",
            candidates.len()
        );
    }

    #[test]
    fn test_detect_stars_gpu_reusable_detector() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut detector = GpuThresholdDetector::new();

        let width = 32;
        let height = 32;

        // Create a single bright pixel
        let mut pixels = vec![0.1f32; width * height];
        pixels[16 * width + 16] = 0.5;

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let config = StarDetectionConfig {
            min_area: 1,
            max_area: 50,
            edge_margin: 5,
            background_config: BackgroundConfig {
                detection_sigma: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        // Run detection multiple times with the same detector
        for _ in 0..3 {
            let candidates = detect_stars_gpu_with_detector(
                &mut detector,
                &pixels,
                width,
                height,
                &background,
                &config,
            );

            assert_eq!(
                candidates.len(),
                1,
                "Expected 1 candidate, got {}",
                candidates.len()
            );
        }
    }

    #[test]
    fn test_detect_stars_gpu_edge_rejection() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let width = 64;
        let height = 64;

        // Create stars near edges and in center
        let mut pixels = vec![0.1f32; width * height];

        // Star too close to edge (should be rejected)
        pixels[5 * width + 5] = 0.5;
        // Star in valid region (should be kept)
        pixels[32 * width + 32] = 0.5;
        // Another star too close to edge (should be rejected)
        pixels[58 * width + 58] = 0.5;

        let background = background_map::uniform(width, height, 0.1, 0.01);

        let config = StarDetectionConfig {
            min_area: 1,
            max_area: 50,
            edge_margin: 10, // Large edge margin to reject edge stars
            background_config: BackgroundConfig {
                detection_sigma: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let candidates = detect_stars_gpu(&pixels, width, height, &background, &config);

        // Only the center star should be detected
        assert_eq!(
            candidates.len(),
            1,
            "Expected 1 candidate (edge rejection), got {}",
            candidates.len()
        );
    }
}
