use anyhow::{Context, Result};
use libraw_sys as sys;
use std::fs;
use std::path::Path;
use std::slice;
use std::time::Instant;

use super::demosaic::{BayerImage, CfaPattern, demosaic_bilinear};
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

/// Load raw file using libraw (C library, broader camera support).
/// Returns a demosaiced RGB image using our custom fast demosaic.
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

    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width
        .checked_mul(raw_height)
        .expect("Raw image dimensions overflow");

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    // The slice is valid for the lifetime of the guard (until libraw_close is called).
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize to 0.0-1.0 range using proper black and maximum values
    let bayer_data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

    // Drop guard is no longer needed after we've copied the data
    drop(guard);

    // Demosaic Bayer to RGB (assuming RGGB pattern for libraw)
    let bayer = BayerImage::with_margins(
        &bayer_data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        CfaPattern::Rggb,
    );
    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_bilinear(&bayer);
    let demosaic_elapsed = demosaic_start.elapsed();
    tracing::info!(
        "Demosaicing {}x{} (libraw) took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    let dimensions = ImageDimensions::new(width, height, 3);

    assert!(
        rgb_pixels.len() == dimensions.pixel_count(),
        "Pixel count mismatch: expected {}, got {}",
        dimensions.pixel_count(),
        rgb_pixels.len()
    );

    let metadata = AstroImageMetadata {
        object: None,
        instrument: None,
        telescope: None,
        date_obs: None,
        exposure_time: None,
        bitpix: BitPix::Int16,
        header_dimensions: vec![height, width, 3],
    };

    Ok(AstroImage {
        metadata,
        pixels: rgb_pixels,
        dimensions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_raw_path() -> Option<PathBuf> {
        std::env::var("LUMOS_CALIBRATION_DIR")
            .ok()
            .map(PathBuf::from)
            .and_then(|dir| {
                let lights = dir.join("Lights");
                if lights.exists() {
                    common::file_utils::astro_image_files(&lights)
                        .into_iter()
                        .find(|p| {
                            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                            matches!(
                                ext.to_lowercase().as_str(),
                                "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng"
                            )
                        })
                } else {
                    None
                }
            })
    }

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
        let Some(path) = test_raw_path() else {
            eprintln!("No RAW file found for testing, skipping");
            return;
        };

        crate::test_utils::init_tracing();

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
        let Some(path) = test_raw_path() else {
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
