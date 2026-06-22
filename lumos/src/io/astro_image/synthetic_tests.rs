//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written and read back through the
//! real `load_fits` path — exercising BitPix selection, the unsigned-via-BZERO convention, the
//! `[0,1]` normalization, and NaN/Inf sanitization. The demosaic path is exercised by building a
//! Bayer mosaic from a known colour and demosaicing it back.

use crate::io::astro_image::fits::load_fits;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::testing::make_cfa;
use crate::{AstroImage, CfaType};
use common::CancelToken;
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
    let px = loaded.channel(0).pixels();
    // Nothing non-finite survives, and each injected site is replaced by exactly 0 …
    assert!(
        px.iter().all(|v| v.is_finite()),
        "load must sanitize NaN/Inf"
    );
    assert_eq!(px[0], 0.0, "NaN → 0");
    assert_eq!(px[5], 0.0, "+Inf → 0");
    assert_eq!(px[10], 0.0, "-Inf → 0");
    // … while a valid neighbour passes through untouched (max valid 0.3 < 2.0 → no rescale).
    assert_eq!(px[1], 0.3, "valid pixel must be preserved");
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
    let image = make_cfa(w, h, mosaic, cfa)
        .demosaic(&CancelToken::never())
        .unwrap();

    // A uniform colour must demosaic back to that colour. RCD is gradient-based, so a perfectly
    // flat field is a degenerate (zero-gradient) input with a few ratio artifacts — but recovery
    // must be *unbiased*: the interior mean of every channel matches the true colour, and the
    // typical pixel is close (median deviation small).
    let channels = [
        image.channel(0).pixels(),
        image.channel(1).pixels(),
        image.channel(2).pixels(),
    ];
    for (ch, &true_c) in channels.iter().zip(&rgb) {
        let mut devs: Vec<f32> = Vec::new();
        let mut sum = 0.0f64;
        for y in 6..h - 6 {
            for x in 6..w - 6 {
                let v = ch[y * w + x];
                sum += v as f64;
                devs.push((v - true_c).abs());
            }
        }
        let mean = (sum / devs.len() as f64) as f32;
        devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_dev = devs[devs.len() / 2];
        assert!(
            (mean - true_c).abs() < 0.01,
            "interior mean {mean} should recover channel colour {true_c}"
        );
        assert!(
            median_dev < 0.01,
            "the typical interior pixel should match {true_c}, median deviation {median_dev}"
        );
    }
}
