//! Demosaic quality against libraw's own, as a hand-run report on the dataset's RAW files.
//!
//! The reports print per-channel and colour-structure differences after removing each channel's
//! affine difference (libraw white-balances, this pipeline does not), so they read pure demosaic
//! quality. They assert only that the two decodes share a shape: a quality ranking is a judgement
//! to read, not a number to pin.

use std::array;
use std::path::Path;

use imaginarium::Buffer2;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::raw::internals::load_raw_libraw_demosaic;
use crate::io::raw::load_raw;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::testing::init_tracing;
use crate::testing::real_data::raw_frames;
use common::CancelToken;

/// Pixels this far from an edge are left out: both demosaics extrapolate there.
const BORDER: usize = 6;

/// Our Markesteijn against libraw's 1-pass and 3-pass, and the two libraw passes against each
/// other for scale.
#[test]
#[ignore = "a hand-run quality report; run it with --ignored --nocapture"]
fn markesteijn_quality_vs_libraw() {
    init_tracing();

    let path = raw_frames("Lights").swap_remove(0);
    println!("Quality comparison on: {}\n", path.display());

    let ours = load_raw(&path, &CancelToken::never()).unwrap();
    let one_pass = load_raw_libraw_demosaic(&path, 1).unwrap();
    let three_pass = load_raw_libraw_demosaic(&path, 3).unwrap();
    assert_eq!(ours.dimensions(), one_pass.dimensions());
    assert_eq!(ours.dimensions().channels(), 3);
    let size = Size2us::new(ours.width(), ours.height());

    println!("--- Ours vs libraw 1-pass (linear regression normalized) ---");
    let ours_vs_one = compare_images(&ours, &one_pass, size, BORDER);
    for (name, stats) in ["Red", "Green", "Blue"].iter().zip(&ours_vs_one.channels) {
        println!(
            "  {name}: MAE={:.6}, max={:.6}, PSNR={:.1}dB, r={:.6}  (scale={:.4}, offset={:.6})",
            stats.mae, stats.max_abs, stats.psnr, stats.correlation, stats.scale, stats.offset,
        );
    }
    println!(
        "  Avg MAE={:.6}, chroma error: mean={:.6}, max={:.6}",
        ours_vs_one.average_mae, ours_vs_one.color_structure.mean, ours_vs_one.color_structure.max,
    );

    let ours_vs_three = compare_images(&ours, &three_pass, size, BORDER);
    let one_vs_three = compare_images(&one_pass, &three_pass, size, BORDER);
    println!(
        "  Ours vs 3-pass:   avg MAE={:.6}, chroma mean={:.6}, max={:.6}",
        ours_vs_three.average_mae,
        ours_vs_three.color_structure.mean,
        ours_vs_three.color_structure.max,
    );
    println!(
        "  1-pass vs 3-pass: avg MAE={:.6}, chroma mean={:.6}, max={:.6}  (baseline)",
        one_vs_three.average_mae,
        one_vs_three.color_structure.mean,
        one_vs_three.color_structure.max,
    );
}

/// Our RCD against libraw's AHD, PPG and DHT on a Bayer sample from `test_data/raw_samples`.
#[test]
#[ignore = "a hand-run quality report; run it with --ignored --nocapture"]
fn bayer_rcd_quality_vs_libraw() {
    init_tracing();

    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/raw_samples");
    let path = ["sample_canon.cr2", "raw-12bit-GBRG.dng", "sample.dng"]
        .iter()
        .map(|f| base.join(f))
        .find(|p| p.exists())
        .expect("No Bayer test file found in test_data/raw_samples/");
    println!("Bayer quality comparison on: {}\n", path.display());

    let ours = load_raw(&path, &CancelToken::never()).unwrap();
    let size = Size2us::new(ours.width(), ours.height());
    for (qual, label) in [(3, "AHD"), (2, "PPG"), (11, "DHT")] {
        let reference = load_raw_libraw_demosaic(&path, qual).unwrap();
        assert_eq!(ours.dimensions(), reference.dimensions(), "{label}");
        let compared = compare_images(&ours, &reference, size, BORDER);
        println!("  --- Ours vs libraw {label} (linear regression normalized) ---");
        for (name, stats) in ["Red", "Green", "Blue"].iter().zip(&compared.channels) {
            println!(
                "    {name}: MAE={:.6}, PSNR={:.1}dB, r={:.6}  (scale={:.4}, offset={:.6})",
                stats.mae, stats.psnr, stats.correlation, stats.scale, stats.offset,
            );
        }
        println!("    Avg MAE: {:.6}\n", compared.average_mae);
    }
}

#[derive(Debug)]
struct ChannelCompareStats {
    mae: f64,
    max_abs: f64,
    psnr: f64,
    correlation: f64,
    scale: f64,
    offset: f64,
}

#[derive(Debug)]
struct ColorStructureStats {
    mean: f64,
    max: f64,
}

#[derive(Debug)]
struct ImageCompareStats {
    channels: [ChannelCompareStats; 3],
    average_mae: f64,
    color_structure: ColorStructureStats,
}

fn compare_images(
    a: &LinearImage,
    b: &LinearImage,
    size: Size2us,
    border: usize,
) -> ImageCompareStats {
    let channels = array::from_fn(|channel| {
        compare_channels(a.channel(channel), b.channel(channel), size, border)
    });
    let average_mae = channels.iter().map(|stats| stats.mae).sum::<f64>() / 3.0;
    let color_structure = compare_color_structure(a, b, &channels, size, border);

    ImageCompareStats {
        channels,
        average_mae,
        color_structure,
    }
}

fn compare_color_structure(
    a: &LinearImage,
    b: &LinearImage,
    transforms: &[ChannelCompareStats; 3],
    size: Size2us,
    border: usize,
) -> ColorStructureStats {
    let mut sum = 0.0;
    let mut max = 0.0_f64;
    let mut count = 0usize;

    for y in border..(size.height - border) {
        for x in border..(size.width - border) {
            let index = size.index_of(Vec2us::new(x, y));
            let mut predicted = [0.0; 3];
            let mut actual = [0.0; 3];
            for channel in 0..3 {
                let transform = &transforms[channel];
                predicted[channel] =
                    f64::from(a.channel(channel)[index]) * transform.scale + transform.offset;
                actual[channel] = f64::from(b.channel(channel)[index]);
            }
            let red_green = (predicted[0] - predicted[1]) - (actual[0] - actual[1]);
            let blue_green = (predicted[2] - predicted[1]) - (actual[2] - actual[1]);
            let error = red_green.hypot(blue_green);
            sum += error;
            max = max.max(error);
            count += 1;
        }
    }

    ColorStructureStats {
        mean: sum / count as f64,
        max,
    }
}

/// Compare two channels using linear regression to remove scale/offset differences.
fn compare_channels(
    a: &Buffer2<f32>,
    b: &Buffer2<f32>,
    size: Size2us,
    border: usize,
) -> ChannelCompareStats {
    // Linear regression: b ~ scale * a + offset
    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut sum_a2 = 0.0f64;
    let mut sum_ab = 0.0f64;
    let mut sum_b2 = 0.0f64;
    let mut n = 0u64;

    for y in border..(size.height - border) {
        for x in border..(size.width - border) {
            let idx = size.index_of(Vec2us::new(x, y));
            let av = f64::from(a[idx]);
            let bv = f64::from(b[idx]);
            sum_a += av;
            sum_b += bv;
            sum_a2 += av * av;
            sum_ab += av * bv;
            sum_b2 += bv * bv;
            n += 1;
        }
    }

    let nf = n as f64;
    let denom = nf * sum_a2 - sum_a * sum_a;
    let (scale, offset) = if denom.abs() > 1e-30 {
        let s = (nf * sum_ab - sum_a * sum_b) / denom;
        let o = (sum_b - s * sum_a) / nf;
        (s, o)
    } else {
        (1.0, 0.0)
    };

    // Compute residuals after regression
    let mut sum_abs_err = 0.0f64;
    let mut sum_sq_err = 0.0f64;
    let mut max_abs = 0.0_f64;
    let mean_b = sum_b / nf;

    for y in border..(size.height - border) {
        for x in border..(size.width - border) {
            let idx = size.index_of(Vec2us::new(x, y));
            let predicted = f64::from(a[idx]) * scale + offset;
            let actual = f64::from(b[idx]);
            let diff = predicted - actual;
            let abs = diff.abs();
            sum_abs_err += abs;
            sum_sq_err += diff * diff;
            max_abs = max_abs.max(abs);
        }
    }

    let mae = sum_abs_err / nf;
    let mse = sum_sq_err / nf;
    let psnr = if mse > 0.0 {
        10.0 * (mean_b * mean_b / mse).log10()
    } else {
        f64::INFINITY
    };

    // Pearson correlation
    let correlation = (nf * sum_ab - sum_a * sum_b)
        / ((nf * sum_a2 - sum_a * sum_a).sqrt() * (nf * sum_b2 - sum_b * sum_b).sqrt());

    ChannelCompareStats {
        mae,
        max_abs,
        psnr,
        correlation,
        scale,
        offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_comparison_removes_affine_color_and_measures_chroma_residuals() {
        let source = Buffer2::new(3, 1, vec![0.0, 1.0, 2.0]);
        let reference = Buffer2::new(3, 1, vec![1.0, 3.0, 5.0]);
        let channel = compare_channels(&source, &reference, Size2us::new(3, 1), 0);
        assert!((channel.scale - 2.0).abs() < 1e-12);
        assert!((channel.offset - 1.0).abs() < 1e-12);
        assert_eq!(channel.mae, 0.0);
        assert_eq!(channel.max_abs, 0.0);
        assert!((channel.correlation - 1.0).abs() < 1e-12);

        let dimensions = ImageDimensions::new((1, 1), 3);
        let black = LinearImage::from_pixels(dimensions, vec![0.0; 3]);
        let colored = LinearImage::from_pixels(dimensions, vec![3.0, 1.0, 5.0]);
        let transforms = array::from_fn(|_| ChannelCompareStats {
            mae: 0.0,
            max_abs: 0.0,
            psnr: f64::INFINITY,
            correlation: 1.0,
            scale: 1.0,
            offset: 0.0,
        });
        let chroma = compare_color_structure(&black, &colored, &transforms, Size2us::new(1, 1), 0);
        let expected = 20.0_f64.sqrt();
        assert!((chroma.mean - expected).abs() < 1e-12);
        assert!((chroma.max - expected).abs() < 1e-12);
    }
}
