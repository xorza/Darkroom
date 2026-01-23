//! Integration tests for X-Trans sensor support.
//!
//! These tests require the LUMOS_CALIBRATION_DIR environment variable
//! to point to a directory containing X-Trans RAF files in Lights subdirectory.

use crate::AstroImage;
use crate::testing::{calibration_dir, init_tracing};
use std::path::PathBuf;

/// Returns path to the X-Trans test file: Lights/03_DSCF6799.RAF
fn xtrans_test_file() -> Option<PathBuf> {
    let cal_dir = calibration_dir()?;
    let file_path = cal_dir.join("Lights").join("03_DSCF6799.RAF");
    if file_path.exists() {
        Some(file_path)
    } else {
        eprintln!("X-Trans test file not found: {}", file_path.display());
        None
    }
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_load_xtrans_raf_file() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    // Load the X-Trans RAF file
    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    // Validate basic dimensions are non-zero
    assert!(image.dimensions.width > 0, "Image width must be non-zero");
    assert!(image.dimensions.height > 0, "Image height must be non-zero");
    assert!(
        image.dimensions.channels > 0,
        "Image channels must be non-zero"
    );

    // X-Trans demosaic should produce RGB output (3 channels)
    assert_eq!(
        image.dimensions.channels, 3,
        "X-Trans demosaic should produce RGB image with 3 channels"
    );

    // Validate pixel data exists and matches dimensions
    let expected_pixel_count = image.dimensions.pixel_count();
    assert_eq!(
        image.pixels.len(),
        expected_pixel_count,
        "Pixel data length {} doesn't match expected {}",
        image.pixels.len(),
        expected_pixel_count
    );

    // Validate pixels are in normalized range [0.0, 1.0]
    for (i, &pixel) in image.pixels.iter().enumerate() {
        assert!(
            (0.0..=1.0).contains(&pixel),
            "Pixel {} out of range: {} (expected 0.0-1.0)",
            i,
            pixel
        );
        assert!(
            !pixel.is_nan(),
            "Pixel {} is NaN - demosaic produced invalid data",
            i
        );
        assert!(
            pixel.is_finite(),
            "Pixel {} is infinite - demosaic produced invalid data",
            i
        );
    }

    tracing::info!(
        "Successfully loaded X-Trans image: {}x{} ({} channels, {} pixels)",
        image.dimensions.width,
        image.dimensions.height,
        image.dimensions.channels,
        expected_pixel_count
    );
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_image_dimensions_reasonable() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    // Fujifilm X-Trans sensors typically have resolutions around:
    // X-T1/X-T2: 4896x3264 (16MP)
    // X-T3/X-T4: 6240x4160 (26MP)
    // X-H2: 8256x6192 (40MP)
    // Allow reasonable range for various X-Trans cameras
    assert!(
        image.dimensions.width >= 1000,
        "Image width {} seems too small for X-Trans sensor",
        image.dimensions.width
    );
    assert!(
        image.dimensions.width <= 10000,
        "Image width {} seems too large for X-Trans sensor",
        image.dimensions.width
    );
    assert!(
        image.dimensions.height >= 1000,
        "Image height {} seems too small for X-Trans sensor",
        image.dimensions.height
    );
    assert!(
        image.dimensions.height <= 8000,
        "Image height {} seems too large for X-Trans sensor",
        image.dimensions.height
    );

    // Validate aspect ratio is reasonable (typical camera aspect ratios: 3:2, 4:3, 16:9)
    let aspect_ratio = image.dimensions.width as f64 / image.dimensions.height as f64;
    assert!(
        (1.0..=2.0).contains(&aspect_ratio),
        "Aspect ratio {} seems unreasonable (expected 1.0-2.0 for landscape)",
        aspect_ratio
    );

    tracing::info!(
        "X-Trans image dimensions: {}x{}, aspect ratio: {:.2}",
        image.dimensions.width,
        image.dimensions.height,
        aspect_ratio
    );
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_pixel_statistics() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    // Calculate basic statistics per channel
    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    for ch in 0..channels {
        let channel_pixels: Vec<f32> = (0..width * height)
            .map(|i| image.pixels[i * channels + ch])
            .collect();

        let sum: f64 = channel_pixels.iter().map(|&p| p as f64).sum();
        let mean = sum / channel_pixels.len() as f64;

        let min = channel_pixels.iter().copied().fold(f32::INFINITY, f32::min);
        let max = channel_pixels
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);

        // Validate channel statistics
        assert!(
            (0.0..=1.0).contains(&mean),
            "Channel {} mean {} out of range",
            ch,
            mean
        );
        assert!(min >= 0.0, "Channel {} min {} should be >= 0.0", ch, min);
        assert!(max <= 1.0, "Channel {} max {} should be <= 1.0", ch, max);

        // For a real image, we expect some variation (not all zeros or all ones)
        assert!(
            max > min,
            "Channel {} has no variation (min=max={}), likely a processing error",
            ch,
            min
        );

        let channel_name = match ch {
            0 => "Red",
            1 => "Green",
            2 => "Blue",
            _ => "Unknown",
        };

        tracing::info!(
            "{} channel: min={:.4}, max={:.4}, mean={:.4}",
            channel_name,
            min,
            max,
            mean
        );
    }
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_no_checkerboard_artifacts() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    // Check for checkerboard patterns by comparing adjacent pixel differences
    // A bad demosaic might produce alternating patterns
    // Sample a region in the middle of the image (avoid edges)
    let start_x = width / 4;
    let start_y = height / 4;
    let sample_size = 100.min(width / 4).min(height / 4);

    for ch in 0..channels {
        let mut horizontal_diffs: Vec<f32> = Vec::new();
        let mut vertical_diffs: Vec<f32> = Vec::new();

        for y in start_y..(start_y + sample_size) {
            for x in start_x..(start_x + sample_size - 1) {
                let idx1 = (y * width + x) * channels + ch;
                let idx2 = (y * width + x + 1) * channels + ch;
                horizontal_diffs.push((image.pixels[idx1] - image.pixels[idx2]).abs());
            }
        }

        for y in start_y..(start_y + sample_size - 1) {
            for x in start_x..(start_x + sample_size) {
                let idx1 = (y * width + x) * channels + ch;
                let idx2 = ((y + 1) * width + x) * channels + ch;
                vertical_diffs.push((image.pixels[idx1] - image.pixels[idx2]).abs());
            }
        }

        // Calculate variance of differences
        let h_mean: f64 =
            horizontal_diffs.iter().map(|&d| d as f64).sum::<f64>() / horizontal_diffs.len() as f64;
        let v_mean: f64 =
            vertical_diffs.iter().map(|&d| d as f64).sum::<f64>() / vertical_diffs.len() as f64;

        // For a smooth demosaic, mean differences should be relatively small
        // (unless there's actual detail in the image)
        // We mainly check they're not unreasonably large
        assert!(
            h_mean < 0.5,
            "Channel {} horizontal differences mean {:.4} too high - possible artifact",
            ch,
            h_mean
        );
        assert!(
            v_mean < 0.5,
            "Channel {} vertical differences mean {:.4} too high - possible artifact",
            ch,
            v_mean
        );

        let channel_name = match ch {
            0 => "Red",
            1 => "Green",
            2 => "Blue",
            _ => "Unknown",
        };

        tracing::info!(
            "{} channel smoothness: h_diff_mean={:.4}, v_diff_mean={:.4}",
            channel_name,
            h_mean,
            v_mean
        );
    }
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_color_balance() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    assert_eq!(channels, 3, "Expected 3 channels for RGB image");

    // Calculate mean for each channel
    let mut channel_means = [0.0f64; 3];
    let pixel_count = width * height;

    for y in 0..height {
        for x in 0..width {
            let base_idx = (y * width + x) * channels;
            for (ch, mean) in channel_means.iter_mut().enumerate() {
                *mean += image.pixels[base_idx + ch] as f64;
            }
        }
    }

    for mean in &mut channel_means {
        *mean /= pixel_count as f64;
    }

    // Check that channels are reasonably balanced (no extreme color cast)
    // Allow ratio up to 3x between channels (real images can have some color cast)
    let max_mean = channel_means.iter().copied().fold(0.0f64, f64::max);
    let min_mean = channel_means.iter().copied().fold(1.0f64, f64::min);

    // Avoid division by zero
    if min_mean > 0.001 {
        let ratio = max_mean / min_mean;
        assert!(
            ratio < 5.0,
            "Channel imbalance ratio {:.2} is extreme (R={:.4}, G={:.4}, B={:.4})",
            ratio,
            channel_means[0],
            channel_means[1],
            channel_means[2]
        );
    }

    tracing::info!(
        "Channel means: R={:.4}, G={:.4}, B={:.4}",
        channel_means[0],
        channel_means[1],
        channel_means[2]
    );
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_metadata() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    // Metadata from RAF files may be limited (not FITS)
    // Just verify we can access it without panic
    tracing::info!("Metadata: {:?}", image.metadata);

    // BitPix should indicate the source data type
    tracing::info!("BitPix: {:?}", image.metadata.bitpix);
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_xtrans_conversion_to_imaginarium_image() {
    init_tracing();

    let Some(file_path) = xtrans_test_file() else {
        eprintln!("Skipping test: X-Trans test file not available");
        return;
    };

    let astro_image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

    // Convert to imaginarium Image
    let image: imaginarium::Image = astro_image.clone().into();

    // Validate conversion preserved dimensions
    assert_eq!(
        image.desc().width,
        astro_image.dimensions.width,
        "Width mismatch after conversion"
    );
    assert_eq!(
        image.desc().height,
        astro_image.dimensions.height,
        "Height mismatch after conversion"
    );

    // RGB image should have RGB_F32 format
    assert_eq!(
        image.desc().color_format,
        imaginarium::ColorFormat::RGB_F32,
        "Expected RGB_F32 format for converted X-Trans image"
    );

    // Validate data size matches
    let expected_bytes = astro_image.dimensions.width
        * astro_image.dimensions.height
        * 3
        * std::mem::size_of::<f32>();
    assert_eq!(
        image.bytes().len(),
        expected_bytes,
        "Byte count mismatch after conversion"
    );

    tracing::info!(
        "Successfully converted X-Trans image to imaginarium::Image ({}x{} RGB_F32)",
        image.desc().width,
        image.desc().height
    );
}
