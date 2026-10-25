#[cfg(feature = "real-data")]
mod real_data;
mod synthetic;

use common::{Size2us, TempDir, fs};
use fits_well::image::SampleType;
use imaginarium::{
    ChannelLayout, ColorFormat, InterleavedDesc, InterleavedImage, PlanarDesc, RasterFormat,
};

use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::{ColorProvenance, ImageProvenance};
use crate::internals::assertions::assert_close;
use crate::io::decoder::Decoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::io::error::{ImageError, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::NullPolicy;
use crate::io::raster;
use common::Vec2us;

#[test]
fn metadata_default() {
    let meta = ImageMetadata::default();
    assert!(meta.header.object.is_none());
    assert!(meta.header.camera_white_balance.is_none());
    assert!(meta.derived.provenance().is_none());
}

#[test]
fn convert_fits_to_imaginarium_image() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/full_example.fits"
    );
    let astro = LoadCtx::default().linear(path).unwrap();
    let image: InterleavedImage = astro.into();

    let desc = image.desc();
    assert_eq!(desc.size().width, 100);
    assert_eq!(desc.size().height, 100);
    assert_eq!(desc.color_format(), ColorFormat::MONO_F32);
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
    assert_eq!(image.channel_count(), 1);
    assert!(image.is_mono());
    assert_eq!(image.pixel_count(), 10000);
    assert_eq!(image.metadata.header.sample_type, Some(SampleType::I32));

    // BITPIX = 32 with BSCALE = 1, so the samples were divided by the declared span 2³² − 1,
    // exact in f64, and the domain carries that span back: the physical ADU value stays
    // recoverable. The sample is that quotient narrowed once.
    let physical_scale = image.metadata.derived.domain().unwrap().scale;
    assert_eq!(physical_scale, 4_294_967_295.0);
    let pixel = image.channel(0)[image.desc().size().idx_of(Vec2us::new(5, 20))];
    assert_eq!(pixel, (152.0 / 4_294_967_295.0_f64) as f32);
    assert_close!(f64::from(pixel) * physical_scale, 152.0, 1e-5, "{pixel}");

    // New metadata fields are None for this simple test file
    assert!(image.metadata.header.filter.is_none());
    assert!(image.metadata.header.gain.is_none());
    assert!(image.metadata.header.ccd_temp.is_none());
    assert!(image.metadata.header.image_type.is_none());
}

/// A saved TIFF reloads with the same dimensions and the same f32 samples, mono and color.
#[test]
fn saved_tiff_round_trips_its_samples() {
    let dir = TempDir::new("lumos-save-tiff");
    for (name, image) in [
        (
            "mono.tiff",
            LinearImage::from_interleaved(
                PlanarDesc::new((2, 2), ChannelLayout::Mono),
                vec![0.1, 0.2, 0.3, 0.4],
            ),
        ),
        (
            "rgb.tiff",
            LinearImage::from_interleaved(
                PlanarDesc::new((2, 2), ChannelLayout::Rgb),
                vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.25],
            ),
        ),
    ] {
        let output_path = dir.join(name);
        image.write_raster(&output_path).unwrap();
        let loaded = LoadCtx::default().linear(&output_path).unwrap();
        assert_eq!(loaded.desc(), image.desc(), "{name}");
        for channel in 0..image.channel_count() {
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
    let image = InterleavedImage::new_with_data(
        InterleavedDesc::new(Size2us::new(2, 2), ColorFormat::MONO_F32),
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
    let float_image = InterleavedImage::new_with_data(
        InterleavedDesc::new(Size2us::new(2, 2), ColorFormat::MONO_F32),
        bytemuck::cast_slice(&float_pixels).to_vec(),
    )
    .unwrap();
    raster::write_image(&float_path, &float_image).unwrap();

    let scientific = LoadCtx::default().linear(&float_path).unwrap();
    assert_eq!(scientific.channel(0).pixels(), float_pixels);
    let preview = LoadCtx::default().preview(&float_path).unwrap();
    assert!(preview.is_mono());
    assert_eq!(preview.channel(0).pixels(), float_pixels);

    let integer_tiff = dir.join("integer.tiff");
    let png = dir.join("display.png");
    let jpeg = dir.join("lossy.jpg");
    let integer_image = InterleavedImage::new_with_data(
        InterleavedDesc::new(Size2us::new(2, 2), ColorFormat::MONO_U8),
        vec![0, 64, 128, 255],
    )
    .unwrap();
    for (path, refusal) in [
        (&integer_tiff, ScientificRefusal::IntegerRaster),
        (
            &png,
            ScientificRefusal::PreviewOnlyRaster {
                format: RasterFormat::Png,
            },
        ),
        (
            &jpeg,
            ScientificRefusal::PreviewOnlyRaster {
                format: RasterFormat::Jpeg,
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
    let alpha_image = InterleavedImage::new_with_data(
        InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGBA_F32),
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
        PlanarDesc::new((2, 2), ChannelLayout::Mono),
        vec![0.1, 0.2, 0.3, 0.4],
    );
    let output_path = dir.join("astro_save_invalid.xyz");

    let result = image.write_raster(&output_path);
    assert!(
        matches!(&result, Err(WriteError::RasterExtension { path }) if *path == output_path),
        "{result:?}"
    );
    assert!(!fs::exists(&output_path));
}

#[test]
fn rgb_image_creation_and_operations() {
    let image = LinearImage::from_interleaved(
        PlanarDesc::new((2, 2), ChannelLayout::Rgb),
        vec![
            10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
        ],
    );

    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 2);
    assert_eq!(image.channel_count(), 3);
    assert!(!image.is_mono());
    assert_eq!(image.pixel_count(), 4);
    assert_eq!(image.sample_count(), 12);
}
