#[cfg(feature = "real-data")]
mod real_data;
mod synthetic;

use std::path::Path;

use common::{TempDir, fs};
use fits_well::image::SampleType;
use imaginarium::Buffer2;
use imaginarium::{ColorFormat, FileFormat, Image, ImageDesc};

use crate::image::channel_layout::ChannelLayout;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::{ColorProvenance, ImageProvenance};
use crate::image::linear_image::LinearImage;
use crate::image::preview_image::PreviewPixels;
use crate::image::quality_flags::QualityFlags;
use crate::internals::assertions::assert_close;
use crate::io::decoder::Decoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::io::error::{ImageError, OverBudget, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::NullPolicy;
use crate::io::raster;
use crate::io::writer;
use crate::math::vec2us::Vec2us;

#[test]
fn metadata_default() {
    let meta = ImageMetadata::default();
    assert!(meta.header.object.is_none());
    assert!(meta.header.camera_white_balance.is_none());
    assert!(meta.derived.provenance().is_none());
}

#[test]
fn convert_to_imaginarium_image_grayscale() {
    let astro = LinearImage::from_interleaved(
        ImageDimensions::new((3, 2), ChannelLayout::Mono),
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
fn convert_to_imaginarium_image_rgb() {
    let astro = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Rgb),
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
fn convert_fits_to_imaginarium_image() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/full_example.fits"
    );
    let astro = LoadCtx::default().linear(path).unwrap();
    let image: Image = astro.into();

    let desc = image.desc();
    assert_eq!(desc.width, 100);
    assert_eq!(desc.height, 100);
    assert_eq!(desc.color_format, ColorFormat::L_F32);
}

#[test]
fn load_full_example_fits() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/full_example.fits"
    );
    let image = LoadCtx::default().linear(path).unwrap();

    assert_eq!(image.width(), 100);
    assert_eq!(image.height(), 100);
    assert_eq!(image.channels(), 1);
    assert!(image.is_grayscale());
    assert_eq!(image.pixel_count(), 10000);
    assert_eq!(image.metadata.header.sample_type, Some(SampleType::I32));

    // BITPIX = 32 with BSCALE = 1, so the samples were divided by the declared span 2³² − 1,
    // exact in f64, and the domain carries that span back: the physical ADU value stays
    // recoverable. The sample is that quotient narrowed once.
    let physical_scale = image.metadata.derived.domain().unwrap().scale;
    assert_eq!(physical_scale, 4_294_967_295.0);
    let pixel = image.channel(0)[image.dimensions().size().idx_of(Vec2us::new(5, 20))];
    assert_eq!(pixel, (152.0 / 4_294_967_295.0_f64) as f32);
    assert_close!(f64::from(pixel) * physical_scale, 152.0, 1e-5, "{pixel}");

    // New metadata fields are None for this simple test file
    assert!(image.metadata.header.filter.is_none());
    assert!(image.metadata.header.gain.is_none());
    assert!(image.metadata.header.ccd_temp.is_none());
    assert!(image.metadata.header.image_type.is_none());
}

#[test]
fn from_image_no_stride_padding() {
    let desc = ImageDesc::new(3, 2, ColorFormat::L_F32);
    let pixels: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let linear = LinearImage::from(&image);

    assert_eq!(linear.width(), 3);
    assert_eq!(linear.height(), 2);
    assert_eq!(linear.channels(), 1);
    assert_eq!(linear.channel(0).pixels(), &*pixels);
}

/// A saved TIFF reloads with the same dimensions and the same f32 samples, gray and color.
#[test]
fn saved_tiff_round_trips_its_samples() {
    let dir = TempDir::new("lumos-save-tiff");
    for (name, image) in [
        (
            "gray.tiff",
            LinearImage::from_interleaved(
                ImageDimensions::new((2, 2), ChannelLayout::Mono),
                vec![0.1, 0.2, 0.3, 0.4],
            ),
        ),
        (
            "rgb.tiff",
            LinearImage::from_interleaved(
                ImageDimensions::new((2, 2), ChannelLayout::Rgb),
                vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.25],
            ),
        ),
    ] {
        let output_path = dir.join(name);
        writer::raster(&image, &output_path).unwrap();
        let loaded = LoadCtx::default().linear(&output_path).unwrap();
        assert_eq!(loaded.dimensions(), image.dimensions(), "{name}");
        for channel in 0..image.channels() {
            assert_eq!(
                loaded.channel(channel).pixels(),
                image.channel(channel).pixels(),
                "{name} channel {channel}"
            );
        }
    }
}

/// A float TIFF's NaN is a null like a FITS one: the default policy fills it with the median of
/// the finite samples (0.5) and flags it `NO_DATA`, and `Reject` refuses the load.
#[test]
fn a_float_tiffs_nulls_follow_the_load_policy() {
    let dir = TempDir::new("lumos-tiff-nulls");
    let path = dir.join("holes.tiff");
    let samples = vec![0.25f32, f32::NAN, 0.5, 0.75];
    let image = Image::new_with_data(
        ImageDesc::new(2, 2, ColorFormat::L_F32),
        bytemuck::cast_slice(&samples).to_vec(),
    )
    .unwrap();
    raster::write_image(&path, &image).unwrap();

    let masked = LoadCtx::default().linear(&path).unwrap();
    assert_eq!(masked.channel(0).pixels(), &[0.25, 0.5, 0.5, 0.75]);
    let flags = masked.flags().unwrap();
    assert_eq!(flags.at(1), QualityFlags::NO_DATA);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);

    let strict = LoadCtx {
        nulls: NullPolicy::Reject,
        ..LoadCtx::default()
    };
    let refused = strict.linear(&path).unwrap_err();
    assert!(
        matches!(
            refused,
            ImageError::NullSamples {
                count: 1,
                first_idx: 1,
                ..
            }
        ),
        "{refused:?}"
    );
}

#[test]
fn product_constructors_separate_linear_science_from_preview_rasters() {
    let dir = TempDir::new("lumos-io-image");
    let float_path = dir.join("linear_float.tiff");
    let float_pixels = vec![-0.25f32, 0.5, 1.25, 3.0];
    let float_image = Image::new_with_data(
        ImageDesc::new(2, 2, ColorFormat::L_F32),
        bytemuck::cast_slice(&float_pixels).to_vec(),
    )
    .unwrap();
    raster::write_image(&float_path, &float_image).unwrap();

    let scientific = LoadCtx::default().linear(&float_path).unwrap();
    assert_eq!(scientific.channel(0).pixels(), float_pixels);
    let preview = LoadCtx::default().preview(&float_path).unwrap();
    assert!(
        matches!(preview.into_pixels(), PreviewPixels::Interleaved(_)),
        "a raster decode stays interleaved"
    );
    let preview: Image = LoadCtx::default().preview(&float_path).unwrap().into();
    assert_eq!(preview.desc().color_format, ColorFormat::L_F32);
    assert_eq!(
        bytemuck::cast_slice::<u8, f32>(preview.bytes()),
        float_pixels
    );

    let integer_tiff = dir.join("integer.tiff");
    let png = dir.join("display.png");
    let jpeg = dir.join("lossy.jpg");
    let integer_image = Image::new_with_data(
        ImageDesc::new(2, 2, ColorFormat::L_U8),
        vec![0, 64, 128, 255],
    )
    .unwrap();
    for (path, refusal) in [
        (&integer_tiff, ScientificRefusal::IntegerRaster),
        (
            &png,
            ScientificRefusal::PreviewOnlyRaster {
                format: FileFormat::Png,
            },
        ),
        (
            &jpeg,
            ScientificRefusal::PreviewOnlyRaster {
                format: FileFormat::Jpeg,
            },
        ),
    ] {
        raster::write_image(path, &integer_image).unwrap();
        for error in [
            LoadCtx::default().linear(path).unwrap_err(),
            LinearDecoder::load(path, &LoadCtx::default(), &SharedGains::default()).unwrap_err(),
        ] {
            assert!(
                matches!(&error, ImageError::ScientificInputRejected { reason, .. } if *reason == refusal),
                "{path:?}: {error:?}"
            );
        }
        LoadCtx::default().preview(path).unwrap();
    }

    let alpha_path = dir.join("alpha.tiff");
    let alpha_pixels = vec![1.0f32, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 0.0];
    let alpha_image = Image::new_with_data(
        ImageDesc::new(2, 1, ColorFormat::RGBA_F32),
        bytemuck::cast_slice(&alpha_pixels).to_vec(),
    )
    .unwrap();
    raster::write_image(&alpha_path, &alpha_image).unwrap();
    assert!(matches!(
        LoadCtx::default().linear(&alpha_path),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::AlphaChannel,
            ..
        })
    ));
    let alpha_preview = LoadCtx::default().preview(&alpha_path).unwrap();
    assert!(matches!(
        alpha_preview.metadata.derived.provenance(),
        Some(ImageProvenance {
            color: ColorProvenance::UnmanagedRaster {
                alpha_dropped: true
            },
            ..
        })
    ));

    // Each raster decode is admitted from its header on its whole peak. The 2 × 2 grey f32 TIFF
    // moves into its plane, then holds its flag plane beside it: 16 + 4 = 20 B. A 2 × 1 RGB f32
    // TIFF holds its 24 B beside the three planes it splits into: 48 B, past its 24 + 2 B after.
    // The 2 × 2 grey u8 PNG previews beside its f32 copy: 4 + 16 = 20 B. A limit one byte short
    // refuses each with that peak, and the peak itself admits it. The 2 × 2 grey u8 JPEG decoder
    // reads its whole file first, refused past the limit before it is read, then holds it beside
    // the 4 B image, a file longer than the 16 B copy.
    let rgb_path = dir.join("rgb_float.tiff");
    let rgb_pixels = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6];
    raster::write_image(
        &rgb_path,
        &Image::new_with_data(
            ImageDesc::new(2, 1, ColorFormat::RGB_F32),
            bytemuck::cast_slice(&rgb_pixels).to_vec(),
        )
        .unwrap(),
    )
    .unwrap();
    let within = |memory_limit_bytes| LoadCtx {
        memory_limit_bytes,
        ..LoadCtx::default()
    };
    let assert_refused = |path: &Path, peak: u64, refused: ImageError| {
        assert!(
            matches!(
                refused,
                ImageError::ImageOverBudget {
                    budget: OverBudget { required, limit },
                    ..
                } if required == peak && limit == peak - 1
            ),
            "{path:?}: {refused:?}"
        );
    };
    for (path, peak) in [(&float_path, 20), (&rgb_path, 48)] {
        assert_refused(path, peak, within(peak - 1).linear(path).unwrap_err());
        within(peak).linear(path).unwrap();
    }
    assert_refused(&png, 20, within(19).preview(&png).unwrap_err());
    within(20).preview(&png).unwrap();
    let held = fs::metadata(&jpeg).unwrap().len();
    assert!(held > 16, "premise: the file is longer than the copy");
    assert_refused(&jpeg, held, within(held - 1).preview(&jpeg).unwrap_err());
    assert_refused(
        &jpeg,
        held + 4,
        within(held + 3).preview(&jpeg).unwrap_err(),
    );
    within(held + 4).preview(&jpeg).unwrap();

    let nonexistent_raw = dir.join("nonexistent.dng");
    assert!(matches!(
        LoadCtx::default().linear(&nonexistent_raw),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::CameraRaw,
            ..
        })
    ));
}

#[test]
fn save_invalid_extension() {
    let dir = TempDir::new("lumos-io-image");
    let image = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Mono),
        vec![0.1, 0.2, 0.3, 0.4],
    );
    let output_path = dir.join("astro_save_invalid.xyz");

    let result = writer::raster(&image, &output_path);
    assert!(matches!(result, Err(WriteError::Raster(_))), "{result:?}");
    assert!(!fs::exists(&output_path));
}

#[test]
fn roundtrip_linear_to_image_to_linear() {
    let gray = LinearImage::from_interleaved(
        ImageDimensions::new((3, 2), ChannelLayout::Mono),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
    );

    let image: Image = gray.clone().into();
    let restored = LinearImage::from(&image);

    assert_eq!(restored.dimensions(), gray.dimensions());
    for (a, b) in gray.channel(0).iter().zip(restored.channel(0).iter()) {
        assert_close!(*a, *b, 1e-6);
    }

    let rgb = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Rgb),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5],
    );

    let image: Image = rgb.clone().into();
    let restored = LinearImage::from(&image);

    assert_eq!(restored.dimensions(), rgb.dimensions());
    for c in 0..rgb.channels() {
        for (a, b) in rgb.channel(c).iter().zip(restored.channel(c).iter()) {
            assert_close!(*a, *b, 1e-6);
        }
    }
}

#[test]
fn image_rgba_to_linear_drops_alpha() {
    let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_F32);
    let pixels: Vec<f32> = vec![1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0];
    let bytes: Vec<u8> = bytemuck::cast_slice(&pixels).to_vec();
    let image = Image::new_with_data(desc, bytes).unwrap();

    let linear = LinearImage::from(&image);

    assert_eq!(linear.channels(), 3);
    assert_close!(linear.channel(0)[0], 1.0, 1e-6);
    assert_close!(linear.channel(1)[0], 0.0, 1e-6);
    assert_close!(linear.channel(2)[0], 0.0, 1e-6);
    assert_close!(linear.channel(0)[1], 0.0, 1e-6);
    assert_close!(linear.channel(1)[1], 1.0, 1e-6);
    assert_close!(linear.channel(2)[1], 0.0, 1e-6);
}

#[test]
fn rgb_image_creation_and_operations() {
    let image = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Rgb),
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
}

/// `from_interleaved` takes interleaved samples and splits them into row-major planes.
#[test]
fn from_interleaved_splits_samples_into_planes() {
    let image = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Rgb),
        vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
    );
    assert_eq!(image.channel(0).pixels(), &[1.0, 4.0, 7.0, 10.0]);
    assert_eq!(image.channel(1).pixels(), &[2.0, 5.0, 8.0, 11.0]);
    assert_eq!(image.channel(2).pixels(), &[3.0, 6.0, 9.0, 12.0]);
}

#[test]
fn from_planar_channels_grayscale() {
    let channels = vec![vec![1.0, 2.0, 3.0, 4.0]];
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 2), ChannelLayout::Mono),
        channels,
    );

    assert!(image.is_grayscale());
    assert_eq!(image.channel(0).pixels(), &[1.0, 2.0, 3.0, 4.0]);

    let plane = Buffer2::new(2, 2, vec![5.0, 6.0, 7.0, 8.0]);
    let pixels = plane.pixels().as_ptr();
    let image = LinearImage::from(plane);
    assert_eq!(
        image.dimensions(),
        ImageDimensions::new((2, 2), ChannelLayout::Mono)
    );
    assert_eq!(image.channel(0).pixels(), &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(image.channel(0).pixels().as_ptr(), pixels);
}

#[test]
fn from_planar_channels_rgb() {
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 1), ChannelLayout::Rgb),
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
    assert_eq!(
        image.dimensions(),
        ImageDimensions::new((2, 1), ChannelLayout::Rgb)
    );
    assert_eq!(image.channel(0).pixels(), &[7.0, 8.0]);
    assert_eq!(image.channel(1).pixels(), &[9.0, 10.0]);
    assert_eq!(image.channel(2).pixels(), &[11.0, 12.0]);
    assert_eq!(image.channel(1).pixels().as_ptr(), green_pixels);
}

#[test]
#[should_panic(expected = "all RGB planes must share width")]
fn rgb_planes_reject_mismatched_dimensions() {
    let _image = LinearImage::from([
        Buffer2::new(2, 1, vec![1.0, 2.0]),
        Buffer2::new(1, 1, vec![3.0]),
        Buffer2::new(2, 1, vec![4.0, 5.0]),
    ]);
}

/// Both `From` impls run `ImageDimensions::validate` for its panics alone — an empty plane cannot
/// become a `LinearPixels`. Pinned because the check has no return value, so nothing else would
/// notice it being dropped as dead code.
#[test]
#[should_panic(expected = "width must be positive")]
fn single_plane_rejects_an_empty_image() {
    let _image = LinearImage::from(Buffer2::<f32>::new(0, 4, vec![]));
}

#[test]
#[should_panic(expected = "height must be positive")]
fn rgb_planes_reject_an_empty_image() {
    let _image = LinearImage::from([
        Buffer2::<f32>::new(4, 0, vec![]),
        Buffer2::<f32>::new(4, 0, vec![]),
        Buffer2::<f32>::new(4, 0, vec![]),
    ]);
}

#[test]
fn channel_mut_writes_through_to_the_plane() {
    let mut image = LinearImage::from_interleaved(
        ImageDimensions::new((2, 2), ChannelLayout::Mono),
        vec![1.0, 2.0, 3.0, 4.0],
    );

    image.channel_mut(0)[0] = 10.0;
    image.channel_mut(0)[3] = 40.0;

    assert_eq!(image.channel(0).pixels(), &[10.0, 2.0, 3.0, 40.0]);
}
