use crate::internals::prelude::*;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;
use crate::math::noise::difference_noise::DifferenceNoise;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::statistics::MedianMad;

fn white_noise(size: Size2us, sigma: f32, seed: u64) -> Vec<f32> {
    let mut rng = TestRng::new(seed);
    (0..size.pixel_count())
        .map(|_| sigma * rng.next_gaussian_f32())
        .collect()
}

/// White Gaussian noise of σ = 1 reads its σ. Two fresh frames of 10⁶ analysed pixels: each
/// estimate scatters by 7 × 10⁻⁴, so their mean by 5 × 10⁻⁴, and the calibration adds 1.3 × 10⁻⁴;
/// 0.002 is three of the combined standard errors.
#[test]
fn white_noise_reads_its_own_sigma() {
    let size = Size2us::new(1024, 1024);
    let mean = (1..=2u64)
        .map(|seed| {
            f64::from(MrsNoise::estimate(
                &white_noise(size, 1.0, seed * 104_729),
                size,
                |_| false,
            ))
        })
        .sum::<f64>()
        / 2.0;
    assert!((mean - 1.0).abs() < 0.002, "{mean}");
}

/// A linear ramp is structure at every scale, so it leaves the estimate: noise of σ = 0.01 plus a
/// ramp from 0 to 1 across 512 columns reads the noise alone's σ to 0.1% — the tiles at the frame's
/// edge reflect the ramp, which bends it within 30 pixels of the edge — while the MAD
/// of the frame reads the ramp: its deviations are uniform on [0, 0.5] around the median 0.5, so
/// MAD = 0.25 and 1.4826 × 0.25 = 0.37, 37 times the noise.
#[test]
fn a_ramp_leaves_the_noise_estimate() {
    let size = Size2us::new(512, 512);
    let noise = white_noise(size, 0.01, 3);
    let ramped: Vec<f32> = noise
        .iter()
        .enumerate()
        .map(|(index, &value)| value + (index % 512) as f32 / 512.0)
        .collect();
    let plain = MrsNoise::estimate(&noise, size, |_| false);
    let with_ramp = MrsNoise::estimate(&ramped, size, |_| false);
    assert!(
        (with_ramp - plain).abs() < 0.001 * plain,
        "{plain} against {with_ramp}"
    );
    assert!((plain - 0.01).abs() < 0.000_2, "{plain}");
    let mut copy = ramped;
    assert_close!(MedianMad::of_mut(&mut copy).sigma(), 0.371, 0.001);
}

/// Excluded pixels leave the estimate: the right half of a frame holds noise of σ = 0.05 and is
/// excluded, the left half σ = 0.01, so the frame reads the left half's σ.
#[test]
fn excluded_pixels_leave_the_estimate() {
    let size = Size2us::new(512, 512);
    let left = white_noise(size, 0.01, 5);
    let right = white_noise(size, 0.05, 6);
    let frame: Vec<f32> = (0..size.pixel_count())
        .map(|index| {
            if index % 512 < 256 {
                left[index]
            } else {
                right[index]
            }
        })
        .collect();
    let sigma = MrsNoise::estimate(&frame, size, |index| index % 512 >= 256);
    assert!((sigma - 0.01).abs() < 0.000_3, "{sigma}");
    // A constant frame has no noise.
    assert_eq!(
        MrsNoise::estimate(&vec![0.5; size.pixel_count()], size, |_| false),
        0.0
    );
}

/// A frame the support covers whole still reads its noise: 48 × 48 of σ = 0.01 crossed by one
/// bright 32-pixel line, whose coarse layers reach every pixel, leaves no pixel outside the
/// support, and the k-σ estimate of the first layer stands. Each frame reads above 0, and the mean
/// of 16 reads σ within 4%: one frame's estimate scatters by 5.2% (400 frames measured), against
/// 1.7% on the noise alone, as the line's trace in the first layer is clipped more or less with
/// each draw; the mean of 16 scatters by 1.3%, and 4% is three of that.
#[test]
fn a_frame_the_support_covers_whole_reads_its_noise() {
    let size = Size2us::new(48, 48);
    let estimates: Vec<f32> = (0..16)
        .map(|seed| {
            let mut frame = white_noise(size, 0.01, 11 + seed);
            for x in 8..40 {
                frame[24 * 48 + x] += 1.0;
            }
            MrsNoise::estimate(&frame, size, |_| false)
        })
        .collect();
    assert!(estimates.iter().all(|&sigma| sigma > 0.0), "{estimates:?}");
    let mean = estimates.iter().sum::<f32>() / 16.0;
    assert!((mean - 0.01).abs() < 0.000_4, "{mean}");
}

/// Each colour of a Bayer mosaic reads its own noise, through a gradient the differences cancel.
/// σ = 0.01 red, 0.02 green, 0.03 blue on a 512² mosaic: red and blue keep 32 768 pairs each, green
/// 65 536. MAD's standard error is 1.166·σ/√n, so 3 of them is 1.9% of σ for red and blue and 1.4%
/// for green; 2% of σ covers all three.
#[test]
fn each_bayer_colour_reads_its_own_noise() {
    let size = Size2us::new(512, 512);
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let sigmas = [0.01f32, 0.02, 0.03];
    let mut rng = TestRng::new(9);
    let mosaic: Vec<f32> = (0..size.pixel_count())
        .map(|index| {
            let (x, y) = (index % 512, index / 512);
            let colour = cfa_type.color_at(Vec2us::new(x, y));
            0.2 + 0.3 * (x as f32 / 512.0) + sigmas[colour] * rng.next_gaussian_f32()
        })
        .collect();
    let estimates = DifferenceNoise::estimate(&mosaic, size, &cfa_type, |_| false);
    for (colour, (&estimate, &sigma)) in estimates.iter().zip(&sigmas).enumerate() {
        assert!(
            (estimate - sigma).abs() < 0.02 * sigma,
            "colour {colour}: {estimate}"
        );
    }
}
