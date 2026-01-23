use anyhow::{Context, Result};
use std::path::Path;
use std::time::Instant;

use super::demosaic::demosaic_bilinear;
use super::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};

/// Load raw file using rawloader (pure Rust, faster but limited camera support).
/// Returns a demosaiced RGB image.
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let raw_image = rawloader::decode_file(path)
        .with_context(|| format!("rawloader: Failed to decode: {}", path.display()))?;

    let width = raw_image.width;
    let height = raw_image.height;

    // Normalize raw Bayer data to 0.0-1.0
    let black_level = raw_image.blacklevels[0] as f32;
    let white_level = raw_image.whitelevels[0] as f32;
    let range = white_level - black_level;

    let bayer: Vec<f32> = match &raw_image.data {
        rawloader::RawImageData::Integer(data) => data
            .iter()
            .map(|&v| ((v as f32) - black_level) / range)
            .collect(),
        rawloader::RawImageData::Float(data) => {
            data.iter().map(|&v| (v - black_level) / range).collect()
        }
    };

    // Demosaic Bayer to RGB
    let cfa = raw_image.cropped_cfa();
    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_bilinear(&bayer, width, height, &cfa);
    let demosaic_elapsed = demosaic_start.elapsed();
    tracing::info!(
        "Demosaicing {}x{} took {:.2}ms",
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

    let is_float = matches!(&raw_image.data, rawloader::RawImageData::Float(_));
    let metadata = AstroImageMetadata {
        object: None,
        instrument: Some(format!(
            "{} {}",
            raw_image.clean_make, raw_image.clean_model
        )),
        telescope: None,
        date_obs: None,
        exposure_time: None,
        bitpix: if is_float {
            BitPix::Float32
        } else {
            BitPix::Int16
        },
        header_dimensions: vec![height, width, 3],
    };

    Ok(AstroImage {
        metadata,
        pixels: rgb_pixels,
        dimensions,
    })
}
