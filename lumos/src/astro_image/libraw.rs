use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::Instant;

use super::demosaic::demosaic_bilinear_libraw;
use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Load raw file using libraw (C library, broader camera support).
/// Returns a demosaiced RGB image using our custom fast demosaic.
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let buf =
        fs::read(path).with_context(|| format!("Failed to read raw file: {}", path.display()))?;

    let processor = libraw::Processor::new();
    let raw_image = processor
        .decode(&buf)
        .with_context(|| format!("libraw: Failed to decode: {}", path.display()))?;

    let sizes = raw_image.sizes();
    let raw_width = sizes.raw_width as usize;
    let raw_height = sizes.raw_height as usize;
    let width = sizes.width as usize;
    let height = sizes.height as usize;
    let top_margin = sizes.top_margin as usize;
    let left_margin = sizes.left_margin as usize;

    // Get raw Bayer data
    let raw_data: &[u16] = &raw_image;

    // Normalize to 0.0-1.0 range
    let max_value = raw_data.iter().max().copied().unwrap_or(65535) as f32;
    let bayer: Vec<f32> = raw_data.iter().map(|&v| (v as f32) / max_value).collect();

    // Demosaic Bayer to RGB
    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_bilinear_libraw(
        &bayer,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
    );
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
