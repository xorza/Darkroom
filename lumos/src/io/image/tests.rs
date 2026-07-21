use crate::image_ops::rgb::Rgb;
use common::test_utils::test_output_path;
use imaginarium::{Buffer2, ColorFormat, Image, ImageDesc};

#[cfg(feature = "real-data")]
use crate::io::image::cfa::CfaImage;
use crate::io::image::linear::{LinearImage, test_support};
use crate::io::image::*;
use crate::io::raw;
use crate::stacking::frame_store::StackableImage;
#[cfg(feature = "real-data")]
use common::CancelToken;

#[test]
fn loadable_extensions_match_decoder_policies() {
    let expected: Vec<&str> = FITS_EXTENSIONS
        .iter()
        .chain(raw::RAW_EXTENSIONS)
        .chain(STANDARD_IMAGE_EXTENSIONS)
        .copied()
        .collect();

    assert_eq!(PREVIEW_IMAGE_EXTENSIONS, expected);
}

#[test]
fn test_metadata_default() {
    let meta = ImageMetadata::default();
    assert!(meta.object.is_none());
    assert!(meta.header_dimensions.is_empty());
    assert!(meta.camera_white_balance.is_none());
    assert!(meta.provenance.is_none());
}

#[test]
fn test_convert_to_imaginarium_image_grayscale() {
    let astro = LinearImage::from_pixels(
        ImageDimensions::new((3, 2), 1),
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
    let astro = LinearImage::from_pixels(
        ImageDimensions::new((2, 2), 3),
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
    let astro = LinearImage::from_file(path).unwrap();
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
    let image = LinearImage::from_file(path).unwrap();

    assert_eq!(image.width(), 100);
    assert_eq!(image.height(), 100);
    assert_eq!(image.channels(), 1);
    assert!(image.is_grayscale());
    assert_eq!(image.pixel_count(), 10000);
    assert_eq!(image.metadata.bitpix, BitPix::Int32);
    assert_eq!(image.metadata.header_dimensions, vec![100, 100]);

    let pixel = image.get_pixel_gray(5, 20);
    assert_eq!(pixel, 152.0);

    // No BAYERPAT header → cfa_type is None
    assert!(image.metadata.cfa_type.is_none());

    // New metadata fields are None for this simple test file
    assert!(image.metadata.filter.is_none());
    assert!(image.metadata.gain.is_none());
    assert!(image.metadata.ccd_temp.is_none());
    assert!(image.metadata.image_type.is_none());
}

#[test]
fn test_from_image_no_stride_padding() {
    let desc = ImageDesc::new(3, 2, ColorFormat::L_F32);
    let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let linear = test_support::from_image(&image);

    assert_eq!(linear.width(), 3);
    assert_eq!(linear.height(), 2);
    assert_eq!(linear.channels(), 1);
    assert_eq!(linear.channel(0).pixels(), &pixels[..]);
}

#[test]
fn test_mean() {
    let image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0, 2.0, 3.0, 4.0]);
    assert!((image.mean() - 2.5).abs() < f32::EPSILON);
}

#[test]
fn test_save_grayscale_tiff() {
    let image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![0.1, 0.2, 0.3, 0.4]);
    let output_path = test_output_path("astro_save_gray.tiff");

    image.save(&output_path).unwrap();
    assert!(output_path.exists());

    let loaded = LinearImage::from_file(&output_path).unwrap();
    assert_eq!(loaded.width(), 2);
    assert_eq!(loaded.height(), 2);
    assert_eq!(loaded.channels(), 1);
}

#[test]
fn test_save_rgb_tiff() {
    let image = LinearImage::from_pixels(
        ImageDimensions::new((2, 2), 3),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
    );
    let output_path = test_output_path("astro_save_rgb.tiff");

    image.save(&output_path).unwrap();
    assert!(output_path.exists());

    let loaded = LinearImage::from_file(&output_path).unwrap();
    assert_eq!(loaded.width(), 2);
    assert_eq!(loaded.height(), 2);
    assert_eq!(loaded.channels(), 3);
}

#[test]
fn product_constructors_separate_linear_science_from_preview_rasters() {
    let float_path = test_output_path("product_constructors/linear_float.tiff");
    let float_pixels = vec![-0.25f32, 0.5, 1.25, 3.0];
    let float_image = Image::new_with_data(
        ImageDesc::new(2, 2, ColorFormat::L_F32),
        bytemuck::cast_slice(&float_pixels).to_vec(),
    )
    .unwrap();
    float_image.save_file(&float_path).unwrap();

    let scientific = LinearImage::from_file(&float_path).unwrap();
    assert_eq!(scientific.channel(0).pixels(), float_pixels);
    let preview: Image = PreviewImage::from_file(&float_path).unwrap().into();
    assert_eq!(preview.desc().color_format, ColorFormat::L_F32);
    assert_eq!(
        bytemuck::cast_slice::<u8, f32>(preview.bytes()),
        float_pixels
    );

    let integer_tiff = test_output_path("product_constructors/integer.tiff");
    let png = test_output_path("product_constructors/display.png");
    let jpeg = test_output_path("product_constructors/lossy.jpg");
    let integer_image = Image::new_with_data(
        ImageDesc::new(2, 2, ColorFormat::L_U8),
        vec![0, 64, 128, 255],
    )
    .unwrap();
    for path in [&integer_tiff, &png, &jpeg] {
        integer_image.save_file(path).unwrap();
        assert!(matches!(
            LinearImage::from_file(path),
            Err(ImageError::ScientificInputRejected { .. })
        ));
        assert!(matches!(
            <LinearImage as StackableImage>::load(path),
            Err(ImageError::ScientificInputRejected { .. })
        ));
        PreviewImage::from_file(path).unwrap();
    }

    let alpha_path = test_output_path("product_constructors/alpha.tiff");
    let alpha_pixels = vec![1.0f32, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 0.0];
    Image::new_with_data(
        ImageDesc::new(2, 1, ColorFormat::RGBA_F32),
        bytemuck::cast_slice(&alpha_pixels).to_vec(),
    )
    .unwrap()
    .save_file(&alpha_path)
    .unwrap();
    assert!(matches!(
        LinearImage::from_file(&alpha_path),
        Err(ImageError::ScientificInputRejected { .. })
    ));
    let alpha_preview = PreviewImage::from_file(&alpha_path).unwrap();
    assert!(matches!(
        alpha_preview.metadata.provenance,
        Some(ImageProvenance {
            color: ColorProvenance::UnmanagedRaster {
                alpha_dropped: true
            },
            ..
        })
    ));

    let nonexistent_raw = test_output_path("product_constructors/nonexistent.dng");
    assert!(matches!(
        LinearImage::from_file(nonexistent_raw),
        Err(ImageError::ScientificInputRejected { .. })
    ));
}

#[test]
fn test_save_invalid_extension() {
    let image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![0.1, 0.2, 0.3, 0.4]);
    let output_path = test_output_path("astro_save_invalid.xyz");

    let result = image.save(&output_path);
    assert!(result.is_err());
}

#[test]
fn test_roundtrip_linear_to_image_to_linear() {
    let gray = LinearImage::from_pixels(
        ImageDimensions::new((3, 2), 1),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
    );

    let image: Image = gray.clone().into();
    let restored = test_support::from_image(&image);

    assert_eq!(restored.dimensions(), gray.dimensions());
    for (a, b) in gray.channel(0).iter().zip(restored.channel(0).iter()) {
        assert!((a - b).abs() < 1e-6);
    }

    let rgb = LinearImage::from_pixels(
        ImageDimensions::new((2, 2), 3),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5],
    );

    let image: Image = rgb.clone().into();
    let restored = test_support::from_image(&image);

    assert_eq!(restored.dimensions(), rgb.dimensions());
    for c in 0..rgb.channels() {
        for (a, b) in rgb.channel(c).iter().zip(restored.channel(c).iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}

#[test]
fn test_image_rgba_to_linear_drops_alpha() {
    let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_F32);
    let pixels: Vec<f32> = vec![1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let linear = test_support::from_image(&image);

    assert_eq!(linear.channels(), 3);
    assert!((linear.channel(0)[0] - 1.0).abs() < 1e-6);
    assert!((linear.channel(1)[0] - 0.0).abs() < 1e-6);
    assert!((linear.channel(2)[0] - 0.0).abs() < 1e-6);
    assert!((linear.channel(0)[1] - 0.0).abs() < 1e-6);
    assert!((linear.channel(1)[1] - 1.0).abs() < 1e-6);
    assert!((linear.channel(2)[1] - 0.0).abs() < 1e-6);
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn test_load_single_raw_from_env() {
    use crate::testing::{calibration_dir, init_tracing};

    init_tracing();

    let cal_dir = calibration_dir();

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::files_with_extensions(&lights_dir, raw::RAW_EXTENSIONS)
        .expect("scan RAW lights directory");
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading file: {:?}", first_file);

    let image = CfaImage::from_file(first_file)
        .expect("Failed to load CFA image")
        .demosaic(&CancelToken::never())
        .expect("Failed to demosaic CFA image");

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
    let image = LinearImage::from_pixels(
        ImageDimensions::new((2, 2), 3),
        vec![
            10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
        ],
    );

    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 2);
    assert_eq!(image.channels(), 3);
    assert!(!image.is_grayscale());
    assert_eq!(image.pixel_count(), 4);
    assert_eq!(image.sample_count(), 12);

    let expected_mean: f32 =
        (10.0 + 20.0 + 30.0 + 40.0 + 50.0 + 60.0 + 70.0 + 80.0 + 90.0 + 100.0 + 110.0 + 120.0)
            / 12.0;
    assert!((image.mean() - expected_mean).abs() < f32::EPSILON);
}

#[test]
fn test_get_pixel_gray() {
    let image = LinearImage::from_pixels(
        ImageDimensions::new((3, 2), 1),
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
    let image = LinearImage::from_pixels(
        ImageDimensions::new((2, 2), 3),
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
fn test_from_planar_channels_grayscale() {
    let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
    let image = LinearImage::from_planar_channels(ImageDimensions::new((2, 2), 1), channels);

    assert!(image.is_grayscale());
    assert_eq!(image.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);

    let plane = Buffer2::new(2, 2, vec![5.0, 6.0, 7.0, 8.0]);
    let pixels = plane.pixels().as_ptr();
    let image = LinearImage::from(plane);
    assert_eq!(image.dimensions(), ImageDimensions::new((2, 2), 1));
    assert_eq!(image.channel(0).pixels(), &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(image.channel(0).pixels().as_ptr(), pixels);
}

#[test]
fn test_from_planar_channels_rgb() {
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 1), 3),
        vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.0]],
    );

    assert!(image.is_rgb());
    assert_eq!(image.channel(0).pixels(), &[1.0, 2.0]);
    assert_eq!(image.channel(1).pixels(), &[3.0, 4.0]);
    assert_eq!(image.channel(2).pixels(), &[5.0, 6.0]);

    let planes = [
        Buffer2::new(2, 1, vec![7.0, 8.0]),
        Buffer2::new(2, 1, vec![9.0, 10.0]),
        Buffer2::new(2, 1, vec![11.0, 12.0]),
    ];
    let green_pixels = planes[1].pixels().as_ptr();
    let image = LinearImage::from(planes);
    assert_eq!(image.dimensions(), ImageDimensions::new((2, 1), 3));
    assert_eq!(image.channel(0).pixels(), &[7.0, 8.0]);
    assert_eq!(image.channel(1).pixels(), &[9.0, 10.0]);
    assert_eq!(image.channel(2).pixels(), &[11.0, 12.0]);
    assert_eq!(image.channel(1).pixels().as_ptr(), green_pixels);
}

#[test]
#[should_panic(expected = "all RGB planes must share width")]
fn rgb_planes_reject_mismatched_dimensions() {
    let _ = LinearImage::from([
        Buffer2::new(2, 1, vec![1.0, 2.0]),
        Buffer2::new(1, 1, vec![3.0]),
        Buffer2::new(2, 1, vec![4.0, 5.0]),
    ]);
}

#[test]
fn test_channel_mut() {
    let mut image =
        LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0, 2.0, 3.0, 4.0]);

    image.channel_mut(0)[0] = 10.0;
    image.channel_mut(0)[3] = 40.0;

    assert_eq!(image.channel(0).pixels(), &[10.0, 2.0, 3.0, 40.0]);
}

#[test]
fn test_get_pixel_rgb() {
    let image = LinearImage::from_pixels(
        ImageDimensions::new((2, 1), 3),
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    );

    assert_eq!(
        image.get_pixel_rgb(0, 0),
        Rgb {
            r: 1.0,
            g: 2.0,
            b: 3.0
        }
    );
    assert_eq!(
        image.get_pixel_rgb(1, 0),
        Rgb {
            r: 4.0,
            g: 5.0,
            b: 6.0
        }
    );
}

#[test]
fn test_set_pixel_rgb() {
    let mut image = LinearImage::from_pixels(
        ImageDimensions::new((2, 1), 3),
        vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    );

    image.set_pixel_rgb(
        0,
        0,
        Rgb {
            r: 1.0,
            g: 2.0,
            b: 3.0,
        },
    );
    image.set_pixel_rgb(
        1,
        0,
        Rgb {
            r: 4.0,
            g: 5.0,
            b: 6.0,
        },
    );

    assert_eq!(
        image.get_pixel_rgb(0, 0),
        Rgb {
            r: 1.0,
            g: 2.0,
            b: 3.0
        }
    );
    assert_eq!(
        image.get_pixel_rgb(1, 0),
        Rgb {
            r: 4.0,
            g: 5.0,
            b: 6.0
        }
    );
}

#[test]
fn test_get_pixel_gray_mut() {
    let mut image =
        LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0, 2.0, 3.0, 4.0]);

    *image.get_pixel_gray_mut(0, 0) = 10.0;
    *image.get_pixel_gray_mut(1, 1) = 40.0;

    assert_eq!(image.get_pixel_gray(0, 0), 10.0);
    assert_eq!(image.get_pixel_gray(1, 1), 40.0);
}

#[test]
fn test_into_interleaved_pixels_grayscale() {
    let image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0, 2.0, 3.0, 4.0]);

    let interleaved = image.into_interleaved_pixels();
    assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn test_into_interleaved_pixels_rgb() {
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 1), 3),
        vec![vec![1.0, 4.0], vec![2.0, 5.0], vec![3.0, 6.0]],
    );

    let interleaved = image.into_interleaved_pixels();
    assert_eq!(interleaved, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn test_sub_assign() {
    let mut image = LinearImage::from_pixels(
        ImageDimensions::new((2, 1), 3),
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    );
    let source = LinearImage::from_pixels(
        ImageDimensions::new((2, 1), 3),
        vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
    );

    image -= &source;

    assert_eq!(image.channel(0).pixels(), &[-9.0, -36.0]);
    assert_eq!(image.channel(1).pixels(), &[-18.0, -45.0]);
    assert_eq!(image.channel(2).pixels(), &[-27.0, -54.0]);
}

#[test]
fn test_image_dimensions_validation() {
    let dims = ImageDimensions::new((100, 200), 3);
    assert_eq!(dims.size(), (100, 200).into());
    assert_eq!(dims.width(), 100);
    assert_eq!(dims.height(), 200);
    assert_eq!(dims.channels(), 3);
    assert_eq!(dims.pixel_count(), 20000);
    assert_eq!(dims.sample_count(), 60000);
    assert!(!dims.is_grayscale());
    assert!(dims.is_rgb());
}

#[test]
#[should_panic(expected = "Width must be positive")]
fn test_image_dimensions_zero_width() {
    ImageDimensions::new((0, 100), 1);
}

#[test]
#[should_panic(expected = "Height must be positive")]
fn test_image_dimensions_zero_height() {
    ImageDimensions::new((100, 0), 1);
}

#[test]
#[should_panic(expected = "Only 1 (grayscale) or 3 (RGB) channels supported")]
fn test_image_dimensions_invalid_channels() {
    ImageDimensions::new((100, 100), 2);
}

#[test]
#[should_panic(expected = "Image pixel count must fit in usize")]
fn test_image_dimensions_reject_pixel_count_overflow() {
    ImageDimensions::new((usize::MAX, 2), 1);
}

#[test]
#[should_panic(expected = "Image sample count must fit in usize")]
fn test_image_dimensions_reject_sample_count_overflow() {
    ImageDimensions::new((usize::MAX, 1), 3);
}
