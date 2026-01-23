use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Load raw file using libraw (C library, broader camera support).
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let buf =
        fs::read(path).with_context(|| format!("Failed to read raw file: {}", path.display()))?;

    let processor = libraw::Processor::new();
    let raw_image = processor
        .decode(&buf)
        .with_context(|| format!("libraw: Failed to decode: {}", path.display()))?;

    let sizes = raw_image.sizes();
    let width = sizes.raw_width as usize;
    let height = sizes.raw_height as usize;

    let dimensions = ImageDimensions::new(width, height, 1);

    // Raw data is u16, normalize to 0.0-1.0 range
    let raw_data: &[u16] = &raw_image;
    let max_value = 65535.0_f32;
    let pixels: Vec<f32> = raw_data.iter().map(|&v| (v as f32) / max_value).collect();

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
        header_dimensions: vec![height, width],
    };

    Ok(AstroImage {
        metadata,
        pixels,
        dimensions,
    })
}
