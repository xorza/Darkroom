//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written and read back through the
//! real `load_fits` path — exercising BitPix selection, the unsigned-via-BZERO convention, the
//! `[0,1]` normalization, and NaN/Inf sanitization. The demosaic path is exercised by building a
//! Bayer mosaic from a known colour and demosaicing it back.

use crate::astro_image::fits::load_fits;
use crate::raw::demosaic::bayer::CfaPattern;
use crate::testing::make_cfa;
use crate::{AstroImage, CfaType};
use fits_well::{FitsWriter, Image, ImageData, Scaling};

fn identity_scaling() -> Scaling {
    Scaling {
        bscale: 1.0,
        bzero: 0.0,
        blank: None,
    }
}

/// Write `image` to a temp FITS file via `FitsWriter`, then load it through `load_fits`.
fn write_and_load(name: &str, image: &Image) -> AstroImage {
    let path = common::test_utils::test_output_path(&format!("fits_roundtrip/{name}.fits"));
    let mut writer = FitsWriter::new(std::fs::File::create(&path).unwrap());
    writer.write_image(image).unwrap();
    writer.into_inner().sync_all().unwrap();
    load_fits(&path).unwrap()
}

#[test]
fn fits_float32_round_trips_pixels_and_order() {
    let (w, h) = (32usize, 24usize);
    // Horizontal gradient in [0,1] — asymmetric, so a transposed read-back would be caught.
    let pixels: Vec<f32> = (0..h)
        .flat_map(|_| (0..w).map(|x| x as f32 / (w - 1) as f32))
        .collect();
    let image = Image {
        shape: vec![w, h], // fits-well is NAXIS1-first: [width, height]
        samples: ImageData::F32(pixels.clone()),
        scaling: identity_scaling(),
    };

    let loaded = write_and_load("float32", &image);
    assert_eq!(loaded.width(), w);
    assert_eq!(loaded.height(), h);
    assert_eq!(loaded.channels(), 1);
    for (a, b) in loaded.channel(0).pixels().iter().zip(&pixels) {
        assert!((a - b).abs() < 1e-6, "pixel mismatch {a} vs {b}");
    }
}

#[test]
fn fits_unsigned16_round_trips_via_bzero_and_normalizes() {
    let (w, h) = (5usize, 1usize);
    // Stored signed-16 + BZERO=32768 by `from_u16`; loaded back and divided by 65535 → [0,1].
    let raw = [0u16, 16384, 32768, 49152, 65535];
    let image = Image::from_u16(vec![w, h], &raw);

    let loaded = write_and_load("uint16", &image);
    let expected: Vec<f32> = raw.iter().map(|&v| v as f32 / 65535.0).collect();
    for (a, b) in loaded.channel(0).pixels().iter().zip(&expected) {
        assert!((a - b).abs() < 1e-4, "normalized pixel {a} vs {b}");
    }
}

#[test]
fn fits_sanitizes_nan_and_inf() {
    let (w, h) = (4usize, 4usize);
    let mut pixels = vec![0.3f32; w * h];
    pixels[0] = f32::NAN;
    pixels[5] = f32::INFINITY;
    pixels[10] = f32::NEG_INFINITY;
    let image = Image {
        shape: vec![w, h],
        samples: ImageData::F32(pixels),
        scaling: identity_scaling(),
    };

    let loaded = write_and_load("nan_inf", &image);
    // Non-finite values are replaced (with 0); nothing NaN/Inf survives into the pipeline.
    assert!(
        loaded.channel(0).pixels().iter().all(|v| v.is_finite()),
        "load must sanitize NaN/Inf"
    );
    assert_eq!(loaded.channel(0).pixels()[0], 0.0);
}

#[test]
fn demosaic_uniform_bayer_recovers_colour() {
    let (w, h) = (32usize, 32usize);
    let rgb = [0.8f32, 0.5, 0.2]; // R, G, B
    let cfa = CfaType::Bayer(CfaPattern::Rggb);

    // Sample each Bayer site from the (uniform) true colour.
    let mut mosaic = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            mosaic[y * w + x] = rgb[cfa.color_at(x, y) as usize];
        }
    }
    let image = make_cfa(w, h, mosaic, cfa).demosaic();

    // A uniform colour must demosaic back to that colour in the interior (away from the border).
    let idx = 16 * w + 16;
    let (r, g, b) = (
        image.channel(0).pixels()[idx],
        image.channel(1).pixels()[idx],
        image.channel(2).pixels()[idx],
    );
    assert!((r - rgb[0]).abs() < 0.02, "R {r} vs {}", rgb[0]);
    assert!((g - rgb[1]).abs() < 0.02, "G {g} vs {}", rgb[1]);
    assert!((b - rgb[2]).abs() < 0.02, "B {b} vs {}", rgb[2]);
}
