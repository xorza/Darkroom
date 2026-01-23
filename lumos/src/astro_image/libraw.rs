use anyhow::{Context, Result};
use libraw_sys as sys;
use std::fs;
use std::path::Path;
use std::slice;
use std::time::Instant;

use super::demosaic::{BayerImage, CfaPattern, demosaic_bilinear};
use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Load raw file using libraw (C library, broader camera support).
/// Returns a demosaiced RGB image using our custom fast demosaic.
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let buf =
        fs::read(path).with_context(|| format!("Failed to read raw file: {}", path.display()))?;

    // Use libraw-rs-sys directly to access color.maximum and color.black
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        anyhow::bail!("libraw: Failed to initialize");
    }

    // Ensure cleanup on any error
    struct LibrawGuard(*mut sys::libraw_data_t);
    impl Drop for LibrawGuard {
        fn drop(&mut self) {
            unsafe { sys::libraw_close(self.0) };
        }
    }
    let _guard = LibrawGuard(inner);

    // Open buffer
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to open buffer, error code: {}", ret);
    }

    // Unpack raw data
    let ret = unsafe { sys::libraw_unpack(inner) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to unpack, error code: {}", ret);
    }

    // Get sizes
    let raw_width = unsafe { (*inner).sizes.raw_width } as usize;
    let raw_height = unsafe { (*inner).sizes.raw_height } as usize;
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    let top_margin = unsafe { (*inner).sizes.top_margin } as usize;
    let left_margin = unsafe { (*inner).sizes.left_margin } as usize;

    // Get color data for normalization
    let black = unsafe { (*inner).color.black } as f32;
    let maximum = unsafe { (*inner).color.maximum } as f32;
    let range = maximum - black;

    tracing::debug!(
        "libraw: black={}, maximum={}, range={}",
        black,
        maximum,
        range
    );

    // Get raw Bayer data
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, raw_width * raw_height) };

    // Normalize to 0.0-1.0 range using proper black and maximum values
    let bayer_data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

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
