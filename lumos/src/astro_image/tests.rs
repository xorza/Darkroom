use common::test_utils::test_output_path;
use imaginarium::{ColorFormat, Image, ImageDesc};

use crate::testing::{calibration_dir, init_tracing};

use super::*;

#[test]
fn test_metadata_default() {
    let meta = AstroImageMetadata::default();
    assert!(meta.object.is_none());
    assert!(meta.header_dimensions.is_empty());
}

#[test]
fn test_convert_to_imaginarium_image_grayscale() {
    let astro = AstroImage::from_pixels(
        ImageDimensions::new(3, 2, 1),
        vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.5],
    );

    let image: Image = astro.into();
    let desc = image.desc();

    assert_eq!(desc.width, 3);
    assert_eq!(desc.height, 2);
    assert_eq!(desc.color_format, ColorFormat::L_F32);

    let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
    assert_eq!(pixels.len(), 6);
    assert_eq!(pixels[0], 0.0);
    assert_eq!(pixels[1], 0.25);
    assert_eq!(pixels[4], 1.0);
}

#[test]
fn test_convert_to_imaginarium_image_rgb() {
    let astro = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![
            1.0, 0.0, 0.0, // red
            0.0, 1.0, 0.0, // green
            0.0, 0.0, 1.0, // blue
            1.0, 1.0, 1.0, // white
        ],
    );

    let image: Image = astro.into();
    let desc = image.desc();

    assert_eq!(desc.width, 2);
    assert_eq!(desc.height, 2);
    assert_eq!(desc.color_format, ColorFormat::RGB_F32);

    let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
    assert_eq!(pixels.len(), 12);
    assert_eq!(pixels[0], 1.0);
    assert_eq!(pixels[1], 0.0);
    assert_eq!(pixels[2], 0.0);
    assert_eq!(pixels[9], 1.0);
    assert_eq!(pixels[10], 1.0);
    assert_eq!(pixels[11], 1.0);
}

#[test]
fn test_convert_fits_to_imaginarium_image() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_resources/full_example.fits"
    );
    let astro = AstroImage::from_file(path).unwrap();
    let image: Image = astro.into();

    let desc = image.desc();
    assert_eq!(desc.width, 100);
    assert_eq!(desc.height, 100);
    assert_eq!(desc.color_format, ColorFormat::L_F32);
}

#[test]
fn test_load_full_example_fits() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_resources/full_example.fits"
    );
    let image = AstroImage::from_file(path).unwrap();

    assert_eq!(image.width(), 100);
    assert_eq!(image.height(), 100);
    assert_eq!(image.channels(), 1);
    assert!(image.is_grayscale());
    assert_eq!(image.pixel_count(), 10000);
    assert_eq!(image.metadata.bitpix, BitPix::Int32);
    assert_eq!(image.metadata.header_dimensions, vec![100, 100]);

    let pixel = image.get_pixel_gray(5, 20);
    assert_eq!(pixel, 152.0);
}

#[test]
fn test_from_image_no_stride_padding() {
    let desc = ImageDesc::new_with_stride(3, 2, ColorFormat::L_F32);
    let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let astro: AstroImage = image.into();

    assert_eq!(astro.width(), 3);
    assert_eq!(astro.height(), 2);
    assert_eq!(astro.channels(), 1);
    assert_eq!(astro.channel(0).pixels(), &pixels[..]);
}

#[test]
fn test_mean() {
    let image = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);
    assert!((image.mean() - 2.5).abs() < f32::EPSILON);
}

#[test]
fn test_save_grayscale_tiff() {
    let image = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.1, 0.2, 0.3, 0.4]);
    let output_path = test_output_path("astro_save_gray.tiff");

    image.save(&output_path).unwrap();
    assert!(output_path.exists());

    let loaded = AstroImage::from_file(&output_path).unwrap();
    assert_eq!(loaded.width(), 2);
    assert_eq!(loaded.height(), 2);
    assert_eq!(loaded.channels(), 1);
}

#[test]
fn test_save_rgb_tiff() {
    let image = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
    );
    let output_path = test_output_path("astro_save_rgb.tiff");

    image.save(&output_path).unwrap();
    assert!(output_path.exists());

    let loaded = AstroImage::from_file(&output_path).unwrap();
    assert_eq!(loaded.width(), 2);
    assert_eq!(loaded.height(), 2);
    assert_eq!(loaded.channels(), 3);
}

#[test]
fn test_save_invalid_extension() {
    let image = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.1, 0.2, 0.3, 0.4]);
    let output_path = test_output_path("astro_save_invalid.xyz");

    let result = image.save(&output_path);
    assert!(result.is_err());
}

#[test]
fn test_roundtrip_astro_to_image_to_astro() {
    let gray = AstroImage::from_pixels(
        ImageDimensions::new(3, 2, 1),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
    );

    let image: Image = gray.clone().into();
    let restored: AstroImage = image.into();

    assert_eq!(restored.dimensions(), gray.dimensions());
    for (a, b) in gray.channel(0).iter().zip(restored.channel(0).iter()) {
        assert!((a - b).abs() < 1e-6);
    }

    let rgb = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5],
    );

    let image: Image = rgb.clone().into();
    let restored: AstroImage = image.into();

    assert_eq!(restored.dimensions(), rgb.dimensions());
    for c in 0..rgb.channels() {
        for (a, b) in rgb.channel(c).iter().zip(restored.channel(c).iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}

#[test]
fn test_image_rgba_to_astro_drops_alpha() {
    let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::RGBA_F32);
    let pixels: Vec<f32> = vec![1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let astro: AstroImage = image.into();

    assert_eq!(astro.channels(), 3);
    assert!((astro.channel(0)[0] - 1.0).abs() < 1e-6);
    assert!((astro.channel(1)[0] - 0.0).abs() < 1e-6);
    assert!((astro.channel(2)[0] - 0.0).abs() < 1e-6);
    assert!((astro.channel(0)[1] - 0.0).abs() < 1e-6);
    assert!((astro.channel(1)[1] - 1.0).abs() < 1e-6);
    assert!((astro.channel(2)[1] - 0.0).abs() < 1e-6);
}

#[test]
fn test_image_gray_alpha_to_astro_drops_alpha() {
    let desc = ImageDesc::new_with_stride(2, 1, ColorFormat::LA_F32);
    let pixels: Vec<f32> = vec![0.5, 0.8, 0.9, 1.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let astro: AstroImage = image.into();

    assert_eq!(astro.channels(), 1);
    assert!((astro.channel(0)[0] - 0.5).abs() < 1e-6);
    assert!((astro.channel(0)[1] - 0.9).abs() < 1e-6);
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_load_single_raw_from_env() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::astro_image_files(&lights_dir);
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading file: {:?}", first_file);

    let image = AstroImage::from_file(first_file).expect("Failed to load image");

    println!(
        "Loaded image: {}x{}x{}",
        image.width(),
        image.height(),
        image.channels()
    );
    println!("Mean: {}", image.mean());

    assert!(image.width() > 0);
    assert!(image.height() > 0);
    assert_eq!(image.channels(), 3);

    let image: imaginarium::Image = image.into();
    image
        .save_file(test_output_path("light_from_raw.tiff"))
        .unwrap();
}

#[test]
fn test_rgb_image_creation_and_operations() {
    let image = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![
            10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
        ],
    );

    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 2);
    assert_eq!(image.channels(), 3);
    assert!(!image.is_grayscale());
    assert_eq!(image.pixel_count(), 12);

    let expected_mean: f32 =
        (10.0 + 20.0 + 30.0 + 40.0 + 50.0 + 60.0 + 70.0 + 80.0 + 90.0 + 100.0 + 110.0 + 120.0)
            / 12.0;
    assert!((image.mean() - expected_mean).abs() < f32::EPSILON);
}

#[test]
fn test_get_pixel_gray() {
    let image = AstroImage::from_pixels(
        ImageDimensions::new(3, 2, 1),
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    );

    assert_eq!(image.get_pixel_gray(0, 0), 1.0);
    assert_eq!(image.get_pixel_gray(2, 0), 3.0);
    assert_eq!(image.get_pixel_gray(0, 1), 4.0);
    assert_eq!(image.get_pixel_gray(2, 1), 6.0);
    assert_eq!(image.get_pixel_channel(1, 0, 0), 2.0);
    assert_eq!(image.get_pixel_channel(1, 1, 0), 5.0);
}

#[test]
fn test_get_pixel_channel_rgb() {
    let image = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
    );

    assert_eq!(image.get_pixel_channel(0, 0, 0), 1.0);
    assert_eq!(image.get_pixel_channel(0, 0, 1), 2.0);
    assert_eq!(image.get_pixel_channel(0, 0, 2), 3.0);
    assert_eq!(image.get_pixel_channel(1, 0, 0), 4.0);
    assert_eq!(image.get_pixel_channel(1, 0, 1), 5.0);
    assert_eq!(image.get_pixel_channel(1, 0, 2), 6.0);
    assert_eq!(image.get_pixel_channel(0, 1, 0), 7.0);
    assert_eq!(image.get_pixel_channel(0, 1, 1), 8.0);
    assert_eq!(image.get_pixel_channel(0, 1, 2), 9.0);
    assert_eq!(image.get_pixel_channel(1, 1, 0), 10.0);
    assert_eq!(image.get_pixel_channel(1, 1, 1), 11.0);
    assert_eq!(image.get_pixel_channel(1, 1, 2), 12.0);
}

#[test]
fn test_to_grayscale_rgb() {
    let rgb = AstroImage::from_pixels(
        ImageDimensions::new(2, 1, 3),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    );

    let gray = rgb.into_grayscale();

    assert!(gray.is_grayscale());
    assert_eq!(gray.channels(), 1);
    assert!((gray.channel(0)[0] - 0.2126).abs() < 1e-4);
    assert!((gray.channel(0)[1] - 0.7152).abs() < 1e-4);
}

#[test]
fn test_to_grayscale_already_gray() {
    let gray = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);
    let result = gray.into_grayscale();

    assert!(result.is_grayscale());
    assert_eq!(result.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn test_from_planar_channels_grayscale() {
    let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
    let image = AstroImage::from_planar_channels(ImageDimensions::new(2, 2, 1), channels);

    assert!(image.is_grayscale());
    assert_eq!(image.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn test_from_planar_channels_rgb() {
    let image = AstroImage::from_planar_channels(
        ImageDimensions::new(2, 1, 3),
        vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.0]],
    );

    assert!(image.is_rgb());
    assert_eq!(image.channel(0).pixels(), &[1.0, 2.0]);
    assert_eq!(image.channel(1).pixels(), &[3.0, 4.0]);
    assert_eq!(image.channel(2).pixels(), &[5.0, 6.0]);
}

#[test]
fn test_channel_mut() {
    let mut image =
        AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

    image.channel_mut(0)[0] = 10.0;
    image.channel_mut(0)[3] = 40.0;

    assert_eq!(image.channel(0).pixels(), &[10.0, 2.0, 3.0, 40.0]);
}

#[test]
fn test_get_pixel_rgb() {
    let image = AstroImage::from_pixels(
        ImageDimensions::new(2, 1, 3),
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    );

    assert_eq!(image.get_pixel_rgb(0, 0), [1.0, 2.0, 3.0]);
    assert_eq!(image.get_pixel_rgb(1, 0), [4.0, 5.0, 6.0]);
}

#[test]
fn test_set_pixel_rgb() {
    let mut image = AstroImage::from_pixels(
        ImageDimensions::new(2, 1, 3),
        vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    );

    image.set_pixel_rgb(0, 0, [1.0, 2.0, 3.0]);
    image.set_pixel_rgb(1, 0, [4.0, 5.0, 6.0]);

    assert_eq!(image.get_pixel_rgb(0, 0), [1.0, 2.0, 3.0]);
    assert_eq!(image.get_pixel_rgb(1, 0), [4.0, 5.0, 6.0]);
}

#[test]
fn test_get_pixel_gray_mut() {
    let mut image =
        AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

    *image.get_pixel_gray_mut(0, 0) = 10.0;
    *image.get_pixel_gray_mut(1, 1) = 40.0;

    assert_eq!(image.get_pixel_gray(0, 0), 10.0);
    assert_eq!(image.get_pixel_gray(1, 1), 40.0);
}

#[test]
fn test_into_interleaved_pixels_grayscale() {
    let image = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![1.0, 2.0, 3.0, 4.0]);

    let interleaved = image.into_interleaved_pixels();
    assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn test_into_interleaved_pixels_rgb() {
    let image = AstroImage::from_planar_channels(
        ImageDimensions::new(2, 1, 3),
        vec![vec![1.0, 4.0], vec![2.0, 5.0], vec![3.0, 6.0]],
    );

    let interleaved = image.into_interleaved_pixels();
    assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn test_sub_assign() {
    let mut image = AstroImage::from_pixels(
        ImageDimensions::new(2, 1, 3),
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    );
    let source = AstroImage::from_pixels(
        ImageDimensions::new(2, 1, 3),
        vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
    );

    image -= &source;

    assert_eq!(image.channel(0).pixels(), &[-9.0, -36.0]);
    assert_eq!(image.channel(1).pixels(), &[-18.0, -45.0]);
    assert_eq!(image.channel(2).pixels(), &[-27.0, -54.0]);
}

#[test]
fn test_image_dimensions_validation() {
    let dims = ImageDimensions::new(100, 200, 3);
    assert_eq!(dims.width, 100);
    assert_eq!(dims.height, 200);
    assert_eq!(dims.channels, 3);
    assert_eq!(dims.pixel_count(), 60000);
    assert!(!dims.is_grayscale());
    assert!(dims.is_rgb());
}

#[test]
#[should_panic(expected = "Width must be positive")]
fn test_image_dimensions_zero_width() {
    ImageDimensions::new(0, 100, 1);
}

#[test]
#[should_panic(expected = "Height must be positive")]
fn test_image_dimensions_zero_height() {
    ImageDimensions::new(100, 0, 1);
}

#[test]
#[should_panic(expected = "Only 1 (grayscale) or 3 (RGB) channels supported")]
fn test_image_dimensions_invalid_channels() {
    ImageDimensions::new(100, 100, 2);
}

#[test]
fn test_bitpix_roundtrip() {
    let values = [
        (8, BitPix::UInt8),
        (16, BitPix::Int16),
        (32, BitPix::Int32),
        (64, BitPix::Int64),
        (-32, BitPix::Float32),
        (-64, BitPix::Float64),
    ];

    for (fits_val, expected) in values {
        let bitpix = BitPix::from_fits_value(fits_val);
        assert_eq!(bitpix, expected);
        assert_eq!(bitpix.to_fits_value(), fits_val);
    }
}
