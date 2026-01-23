use anyhow::{Context, Result};
use libraw_sys as sys;
use std::fs;
use std::path::Path;
use std::slice;
use std::time::Instant;

use super::demosaic::{BayerImage, CfaPattern, demosaic_bilinear};
use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Extract CFA pattern from libraw filters field.
///
/// The filters field encodes the color at each position in a repeating pattern.
/// For Bayer sensors, this is a 2x2 pattern. The formula to extract color index is:
/// `color_index = (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3`
///
/// Color indices: 0=Red, 1=Green, 2=Blue, 3=Green2
///
/// Returns None if the pattern doesn't match a known Bayer CFA pattern
/// (e.g., for X-Trans sensors or monochrome cameras).
fn cfa_pattern_from_filters(filters: u32) -> Option<CfaPattern> {
    // Extract color index for each position in the 2x2 pattern
    let color_at =
        |row: u32, col: u32| -> u32 { (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3 };

    let c00 = color_at(0, 0);
    let c01 = color_at(0, 1);
    let c10 = color_at(1, 0);
    let c11 = color_at(1, 1);

    // Color indices: 0=R, 1=G, 2=B, 3=G2 (second green)
    // We treat both green indices (1 and 3) as green
    let is_red = |c: u32| c == 0;
    let is_green = |c: u32| c == 1 || c == 3;
    let is_blue = |c: u32| c == 2;

    // Match against known Bayer patterns
    // RGGB: R at (0,0), G at (0,1) and (1,0), B at (1,1)
    if is_red(c00) && is_green(c01) && is_green(c10) && is_blue(c11) {
        return Some(CfaPattern::Rggb);
    }
    // BGGR: B at (0,0), G at (0,1) and (1,0), R at (1,1)
    if is_blue(c00) && is_green(c01) && is_green(c10) && is_red(c11) {
        return Some(CfaPattern::Bggr);
    }
    // GRBG: G at (0,0), R at (0,1), B at (1,0), G at (1,1)
    if is_green(c00) && is_red(c01) && is_blue(c10) && is_green(c11) {
        return Some(CfaPattern::Grbg);
    }
    // GBRG: G at (0,0), B at (0,1), R at (1,0), G at (1,1)
    if is_green(c00) && is_blue(c01) && is_red(c10) && is_green(c11) {
        return Some(CfaPattern::Gbrg);
    }

    // Unknown pattern (e.g., X-Trans, monochrome, or other exotic sensors)
    None
}

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

    // Get sensor info from libraw metadata
    // SAFETY: inner is valid, idata struct is initialized after unpack.
    let filters = unsafe { (*inner).idata.filters };
    let colors = unsafe { (*inner).idata.colors };

    tracing::debug!("libraw: filters=0x{:08x}, colors={}", filters, colors);

    // Normalize to 0.0-1.0 range using proper black and maximum values
    let normalized_data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

    // Drop guard is no longer needed after we've copied the data
    drop(guard);

    // Check if this is a monochrome sensor (no Bayer filter)
    // filters == 0 means no CFA pattern (monochrome or already full-color)
    // colors == 1 confirms it's a true monochrome sensor
    let is_monochrome = filters == 0 || colors == 1;

    let (pixels, num_channels) = if is_monochrome {
        tracing::info!(
            "Monochrome sensor detected (filters=0x{:08x}, colors={}), skipping demosaic",
            filters,
            colors
        );

        // Extract the active area from the raw data (apply margins)
        let mut mono_pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            let src_y = top_margin + y;
            for x in 0..width {
                let src_x = left_margin + x;
                let idx = src_y * raw_width + src_x;
                mono_pixels.push(normalized_data[idx]);
            }
        }

        (mono_pixels, 1)
    } else {
        // Bayer sensor - need to demosaic
        let cfa_pattern = cfa_pattern_from_filters(filters).unwrap_or_else(|| {
            tracing::warn!(
                "Unknown CFA pattern (filters=0x{:08x}), defaulting to RGGB",
                filters
            );
            CfaPattern::Rggb
        });
        tracing::debug!(
            "Detected CFA pattern: {:?} (filters=0x{:08x})",
            cfa_pattern,
            filters
        );

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
            "Demosaicing {}x{} (libraw) took {:.2}ms",
            width,
            height,
            demosaic_elapsed.as_secs_f64() * 1000.0
        );

        (rgb_pixels, 3)
    };

    let dimensions = ImageDimensions::new(width, height, num_channels);

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
        header_dimensions: vec![height, width, num_channels],
    };

    Ok(AstroImage {
        metadata,
        pixels,
        dimensions,
    })
}

#[cfg(test)]
mod tests {
    use crate::testing::{first_raw_file, init_tracing};

    use super::*;

    #[test]
    fn test_cfa_pattern_from_filters_rggb() {
        // RGGB pattern: R=0, G=1, G=3, B=2
        // Position (0,0)=R=0, (0,1)=G=1, (1,0)=G=3, (1,1)=B=2
        // Encoded in filters with 2 bits per position
        // Common RGGB filters value from Canon cameras
        let filters = 0x94949494u32; // Standard RGGB encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Rggb));
    }

    #[test]
    fn test_cfa_pattern_from_filters_bggr() {
        // BGGR pattern: B at (0,0), G at (0,1), G at (1,0), R at (1,1)
        let filters = 0x16161616u32; // Standard BGGR encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Bggr));
    }

    #[test]
    fn test_cfa_pattern_from_filters_grbg() {
        // GRBG pattern: G at (0,0), R at (0,1), B at (1,0), G at (1,1)
        let filters = 0x61616161u32; // Standard GRBG encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Grbg));
    }

    #[test]
    fn test_cfa_pattern_from_filters_gbrg() {
        // GBRG pattern: G at (0,0), B at (0,1), R at (1,0), G at (1,1)
        let filters = 0x49494949u32; // Standard GBRG encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Gbrg));
    }

    #[test]
    fn test_cfa_pattern_from_filters_unknown() {
        // filters=0 typically indicates monochrome or no CFA
        assert_eq!(cfa_pattern_from_filters(0), None);
        // X-Trans and other exotic patterns should return None
        assert_eq!(cfa_pattern_from_filters(0x12345678), None);
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
