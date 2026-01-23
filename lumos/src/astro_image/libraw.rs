use anyhow::{Context, Result};
use libraw_sys as sys;
use std::fs;
use std::path::Path;
use std::slice;
use std::time::Instant;

use super::demosaic::{BayerImage, CfaPattern, demosaic_bilinear};
use super::sensor::{SensorType, detect_sensor_type};
use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// RAII guard for libraw_data_t to ensure proper cleanup.
struct LibrawGuard(*mut sys::libraw_data_t);

// SAFETY: LibrawGuard owns the pointer and ensures exclusive access.
// The libraw library itself is thread-safe when using separate instances.
unsafe impl Send for LibrawGuard {}

impl Drop for LibrawGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_init.
            unsafe { sys::libraw_close(self.0) };
        }
    }
}

/// RAII guard for libraw_processed_image_t to ensure proper cleanup.
struct ProcessedImageGuard(*mut sys::libraw_processed_image_t);

impl Drop for ProcessedImageGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_dcraw_make_mem_image.
            unsafe { sys::libraw_dcraw_clear_mem(self.0) };
        }
    }
}

/// Load raw file using libraw (C library, broader camera support).
///
/// Demosaicing strategy:
/// - Monochrome sensors: no demosaic needed, returns grayscale
/// - Known Bayer patterns (RGGB, BGGR, GRBG, GBRG): fast SIMD demosaic
/// - Unknown patterns (X-Trans, etc.): libraw's built-in demosaic (slower but correct)
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let buf =
        fs::read(path).with_context(|| format!("Failed to read raw file: {}", path.display()))?;

    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        anyhow::bail!("libraw: Failed to initialize");
    }

    // Guard ensures cleanup even on early return or panic
    let guard = LibrawGuard(inner);

    // SAFETY: inner is valid (checked above), buf is valid for the duration of this call.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to open buffer, error code: {}", ret);
    }

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to unpack, error code: {}", ret);
    }

    // SAFETY: inner is valid and unpack succeeded, sizes struct is initialized.
    let raw_width = unsafe { (*inner).sizes.raw_width } as usize;
    let raw_height = unsafe { (*inner).sizes.raw_height } as usize;
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    let top_margin = unsafe { (*inner).sizes.top_margin } as usize;
    let left_margin = unsafe { (*inner).sizes.left_margin } as usize;

    // Validate dimensions
    if raw_width == 0 || raw_height == 0 {
        anyhow::bail!(
            "libraw: Invalid raw dimensions: {}x{}",
            raw_width,
            raw_height
        );
    }
    if width == 0 || height == 0 {
        anyhow::bail!("libraw: Invalid output dimensions: {}x{}", width, height);
    }
    if top_margin + height > raw_height || left_margin + width > raw_width {
        anyhow::bail!(
            "libraw: Margins exceed raw dimensions: margins ({}, {}) + size ({}, {}) > raw ({}, {})",
            top_margin,
            left_margin,
            width,
            height,
            raw_width,
            raw_height
        );
    }

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black = unsafe { (*inner).color.black } as f32;
    let maximum = unsafe { (*inner).color.maximum } as f32;
    let range = maximum - black;

    // Validate color range
    if range <= 0.0 {
        anyhow::bail!(
            "libraw: Invalid color range: black={}, maximum={}, range={}",
            black,
            maximum,
            range
        );
    }

    tracing::debug!(
        "libraw: black={}, maximum={}, range={}",
        black,
        maximum,
        range
    );

    // Get sensor info from libraw metadata
    // SAFETY: inner is valid, idata struct is initialized after unpack.
    let filters = unsafe { (*inner).idata.filters };
    let colors = unsafe { (*inner).idata.colors };
    let sensor_type = detect_sensor_type(filters, colors);

    tracing::debug!(
        "libraw: filters=0x{:08x}, colors={}, sensor_type={:?}",
        filters,
        colors,
        sensor_type
    );

    // Process based on sensor type
    // Returns (pixels, width, height, num_channels) - dimensions may differ for libraw fallback
    let (pixels, out_width, out_height, num_channels) = match sensor_type {
        SensorType::Monochrome => {
            tracing::info!(
                "Monochrome sensor detected (filters=0x{:08x}, colors={}), skipping demosaic",
                filters,
                colors
            );
            let (pixels, channels) = process_monochrome(
                inner,
                raw_width,
                raw_height,
                width,
                height,
                top_margin,
                left_margin,
                black,
                range,
            )?;
            (pixels, width, height, channels)
        }
        SensorType::Bayer(cfa_pattern) => {
            tracing::debug!(
                "Detected Bayer CFA pattern: {:?} (filters=0x{:08x})",
                cfa_pattern,
                filters
            );
            let (pixels, channels) = process_bayer_fast(
                inner,
                raw_width,
                raw_height,
                width,
                height,
                top_margin,
                left_margin,
                black,
                range,
                cfa_pattern,
            )?;
            (pixels, width, height, channels)
        }
        SensorType::Unknown => {
            tracing::info!(
                "Unknown CFA pattern (filters=0x{:08x}, e.g. X-Trans), using libraw demosaic fallback",
                filters
            );
            process_unknown_libraw_fallback(inner)?
        }
    };

    // Guard can be dropped now - we've extracted all needed data
    drop(guard);

    let dimensions = ImageDimensions::new(out_width, out_height, num_channels);

    assert!(
        pixels.len() == dimensions.pixel_count(),
        "Pixel count mismatch: expected {}, got {}",
        dimensions.pixel_count(),
        pixels.len()
    );

    let metadata = AstroImageMetadata {
        object: None,
        instrument: None,
        telescope: None,
        date_obs: None,
        exposure_time: None,
        bitpix: BitPix::Int16,
        header_dimensions: vec![out_height, out_width, num_channels],
    };

    Ok(AstroImage {
        metadata,
        pixels,
        dimensions,
    })
}

/// Process monochrome sensor data (no demosaicing needed).
#[allow(clippy::too_many_arguments)]
fn process_monochrome(
    inner: *mut sys::libraw_data_t,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
) -> Result<(Vec<f32>, usize)> {
    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width * raw_height;
    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Extract the active area and normalize
    let mut mono_pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let src_y = top_margin + y;
        for x in 0..width {
            let src_x = left_margin + x;
            let idx = src_y * raw_width + src_x;
            let normalized = ((raw_data[idx] as f32) - black).max(0.0) / range;
            mono_pixels.push(normalized);
        }
    }

    Ok((mono_pixels, 1))
}

/// Process Bayer sensor data using our fast SIMD demosaic.
#[allow(clippy::too_many_arguments)]
fn process_bayer_fast(
    inner: *mut sys::libraw_data_t,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
    cfa_pattern: CfaPattern,
) -> Result<(Vec<f32>, usize)> {
    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width * raw_height;
    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize to 0.0-1.0 range
    let normalized_data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

    let bayer = BayerImage::with_margins(
        &normalized_data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        cfa_pattern,
    );

    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_bilinear(&bayer);
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "Fast SIMD demosaicing {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok((rgb_pixels, 3))
}

/// Process unknown CFA pattern (X-Trans, etc.) using libraw's built-in demosaic.
/// This is slower but handles exotic sensor patterns correctly.
/// Returns (pixels, width, height, num_channels).
fn process_unknown_libraw_fallback(
    inner: *mut sys::libraw_data_t,
) -> Result<(Vec<f32>, usize, usize, usize)> {
    let demosaic_start = Instant::now();

    // Configure libraw for linear output (no gamma, no color conversion)
    // SAFETY: inner is valid
    unsafe {
        // Output in linear color space (no gamma curve)
        (*inner).params.gamm[0] = 1.0;
        (*inner).params.gamm[1] = 1.0;
        // No brightness adjustment
        (*inner).params.bright = 1.0;
        // Use camera white balance
        (*inner).params.use_camera_wb = 1;
        // Output 16-bit
        (*inner).params.output_bps = 16;
        // Linear color space (raw)
        (*inner).params.output_color = 0;
        // No auto-brightness
        (*inner).params.no_auto_bright = 1;
    }

    // Run libraw's demosaic
    // SAFETY: inner is valid and configured
    let ret = unsafe { sys::libraw_dcraw_process(inner) };
    if ret != 0 {
        anyhow::bail!("libraw: dcraw_process failed, error code: {}", ret);
    }

    // Get the processed image
    // SAFETY: inner is valid and dcraw_process succeeded
    let mut errc: i32 = 0;
    let processed_ptr = unsafe { sys::libraw_dcraw_make_mem_image(inner, &mut errc) };
    if processed_ptr.is_null() || errc != 0 {
        anyhow::bail!("libraw: dcraw_make_mem_image failed, error code: {}", errc);
    }

    // Guard ensures cleanup even on early return or panic
    let _processed_guard = ProcessedImageGuard(processed_ptr);

    // Extract image data
    // SAFETY: processed_ptr is valid (checked above)
    let img_width = unsafe { (*processed_ptr).width } as usize;
    let img_height = unsafe { (*processed_ptr).height } as usize;
    let img_colors = unsafe { (*processed_ptr).colors } as usize;
    let img_bits = unsafe { (*processed_ptr).bits } as usize;
    let data_size = unsafe { (*processed_ptr).data_size } as usize;

    tracing::debug!(
        "libraw fallback: {}x{}x{}, {} bits, {} bytes",
        img_width,
        img_height,
        img_colors,
        img_bits,
        data_size
    );

    // The data array is at the end of the struct (flexible array member)
    // SAFETY: processed_ptr is valid, data_size tells us the valid range
    let data_ptr = unsafe { (*processed_ptr).data.as_ptr() };

    // Use checked arithmetic to prevent overflow on extremely large images
    let pixel_count = img_width
        .checked_mul(img_height)
        .and_then(|v| v.checked_mul(img_colors))
        .expect("libraw: image dimensions overflow");

    let pixels = if img_bits == 16 {
        // 16-bit data
        let expected_size = pixel_count
            .checked_mul(2)
            .expect("libraw: expected_size overflow");
        assert!(
            data_size >= expected_size,
            "libraw: data_size {} < expected {}",
            data_size,
            expected_size
        );

        // SAFETY: data_ptr points to valid u16 data of the calculated size
        let data_u16 = unsafe { slice::from_raw_parts(data_ptr as *const u16, pixel_count) };

        // Normalize to 0.0-1.0
        data_u16
            .iter()
            .map(|&v| (v as f32) / 65535.0)
            .collect::<Vec<f32>>()
    } else {
        // 8-bit data
        assert!(
            data_size >= pixel_count,
            "libraw: data_size {} < expected {}",
            data_size,
            pixel_count
        );

        // SAFETY: data_ptr points to valid u8 data of the calculated size
        let data_u8 = unsafe { slice::from_raw_parts(data_ptr, pixel_count) };

        // Normalize to 0.0-1.0
        data_u8
            .iter()
            .map(|&v| (v as f32) / 255.0)
            .collect::<Vec<f32>>()
    };

    // Memory is freed automatically by _processed_guard when it goes out of scope

    let demosaic_elapsed = demosaic_start.elapsed();
    tracing::info!(
        "Libraw fallback demosaicing {}x{} took {:.2}ms",
        img_width,
        img_height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok((pixels, img_width, img_height, img_colors))
}

#[cfg(test)]
mod tests {
    use crate::testing::{first_raw_file, init_tracing};

    use super::*;

    #[test]
    fn test_load_raw_invalid_path() {
        let result = load_raw(Path::new("/nonexistent/path/to/file.raf"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read raw file"));
    }

    #[test]
    fn test_load_raw_invalid_data() {
        // Create a temp file with invalid data
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("invalid_raw_test.raf");
        fs::write(&temp_file, b"not a valid raw file").unwrap();

        let result = load_raw(&temp_file);
        assert!(result.is_err());

        // Cleanup
        let _ = fs::remove_file(&temp_file);
    }

    #[test]
    fn test_load_raw_empty_file() {
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("empty_raw_test.raf");
        fs::write(&temp_file, b"").unwrap();

        let result = load_raw(&temp_file);
        assert!(result.is_err());

        // Cleanup
        let _ = fs::remove_file(&temp_file);
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_load_raw_valid_file() {
        let Some(path) = first_raw_file() else {
            eprintln!("No RAW file found for testing, skipping");
            return;
        };

        init_tracing();

        let result = load_raw(&path);
        assert!(result.is_ok(), "Failed to load {:?}: {:?}", path, result);

        let image = result.unwrap();

        // Validate dimensions
        assert!(image.dimensions.width > 0);
        assert!(image.dimensions.height > 0);
        assert_eq!(image.dimensions.channels, 3); // RGB output

        // Validate pixel count
        assert_eq!(image.pixels.len(), image.dimensions.pixel_count());

        // Validate pixel values are normalized
        for &pixel in &image.pixels {
            assert!(pixel >= 0.0, "Pixel value {} is negative", pixel);
            // Values can exceed 1.0 slightly due to demosaic interpolation
            assert!(pixel <= 2.0, "Pixel value {} is too large", pixel);
        }

        // Check mean is reasonable (not all zeros or all ones)
        let mean = image.mean();
        assert!(mean > 0.0, "Mean is zero, image may be all black");
        assert!(mean < 1.0, "Mean is >= 1.0, image may be overexposed");
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_load_raw_dimensions_match() {
        let Some(path) = first_raw_file() else {
            eprintln!("No RAW file found for testing, skipping");
            return;
        };

        let image = load_raw(&path).unwrap();

        // Header dimensions should match actual dimensions
        assert_eq!(image.metadata.header_dimensions.len(), 3);
        assert_eq!(image.metadata.header_dimensions[0], image.dimensions.height);
        assert_eq!(image.metadata.header_dimensions[1], image.dimensions.width);
        assert_eq!(
            image.metadata.header_dimensions[2],
            image.dimensions.channels
        );
    }

    #[test]
    fn test_libraw_guard_cleanup() {
        // Test that LibrawGuard properly cleans up
        {
            let inner = unsafe { sys::libraw_init(0) };
            assert!(!inner.is_null());
            let _guard = LibrawGuard(inner);
            // Guard will be dropped here and call libraw_close
        }
        // If we got here without crashing, cleanup worked
    }

    #[test]
    fn test_libraw_guard_null_safe() {
        // Test that LibrawGuard handles null pointer safely
        let _guard = LibrawGuard(std::ptr::null_mut());
        // Should not crash on drop
    }
}
