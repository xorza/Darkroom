use common::Buffer2;

use crate::internals::assertions::assert_close;
use crate::internals::test_rng::TestRng;
use crate::math::statistics::median_mad::MedianMad;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;
use crate::noise::mrs_noise::{self, NoisePlane};
use common::USize2;

fn white_noise(size: USize2, sigma: f32, seed: u64) -> Buffer2<f32> {
    let mut rng = TestRng::new(seed);
    Buffer2::from_fn(size, |_, _| sigma * rng.next_gaussian_f32())
}

/// White Gaussian noise of σ = 1 reads its σ. Two fresh frames of 10⁶ analysed pixels: each
/// estimate scatters by 7 × 10⁻⁴, so their mean by 5 × 10⁻⁴, and the calibration adds 1.3 × 10⁻⁴;
/// 0.002 is three of the combined standard errors.
#[test]
fn white_noise_reads_its_own_sigma() {
    let size = USize2::new(1024, 1024);
    let mean = (1..=2u64)
        .map(|seed| {
            f64::from(
                NoisePlane {
                    samples: &white_noise(size, 1.0, seed * 104_729),
                    excluded: |_| false,
                }
                .sigma(),
            )
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
    let size = USize2::new(512, 512);
    let noise = white_noise(size, 0.01, 3);
    let ramped = Buffer2::new(
        (512, 512),
        noise
            .iter()
            .enumerate()
            .map(|(idx, &value)| value + (idx % 512) as f32 / 512.0)
            .collect(),
    );
    let plain = NoisePlane {
        samples: &noise,
        excluded: |_| false,
    }
    .sigma();
    let with_ramp = NoisePlane {
        samples: &ramped,
        excluded: |_| false,
    }
    .sigma();
    assert!(
        (with_ramp - plain).abs() < 0.001 * plain,
        "{plain} against {with_ramp}"
    );
    assert!((plain - 0.01).abs() < 0.000_2, "{plain}");
    let mut copy = ramped.into_vec();
    assert_close!(MedianMad::of_mut(&mut copy).sigma(), 0.371, 0.001);
}

/// Excluded pixels leave the estimate: the right half of a frame holds noise of σ = 0.05 and is
/// excluded, the left half σ = 0.01, so the frame reads the left half's σ.
#[test]
fn excluded_pixels_leave_the_estimate() {
    let size = USize2::new(512, 512);
    let left = white_noise(size, 0.01, 5);
    let right = white_noise(size, 0.05, 6);
    let frame = Buffer2::from_fn((512, 512), |x, y| {
        if x < 256 { left[(x, y)] } else { right[(x, y)] }
    });
    let sigma = NoisePlane {
        samples: &frame,
        excluded: |idx| idx % 512 >= 256,
    }
    .sigma();
    assert!((sigma - 0.01).abs() < 0.000_3, "{sigma}");
    // A constant frame has no noise.
    assert_eq!(
        NoisePlane {
            samples: &Buffer2::new_filled((512, 512), 0.5),
            excluded: |_| false
        }
        .sigma(),
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
    let size = USize2::new(48, 48);
    let estimates: Vec<f32> = (0..16)
        .map(|seed| {
            let mut frame = white_noise(size, 0.01, 11 + seed);
            for x in 8..40 {
                frame[24 * 48 + x] += 1.0;
            }
            NoisePlane {
                samples: &frame,
                excluded: |_| false,
            }
            .sigma()
        })
        .collect();
    assert!(estimates.iter().all(|&sigma| sigma > 0.0), "{estimates:?}");
    let mean = estimates.iter().sum::<f32>() / 16.0;
    assert!((mean - 0.01).abs() < 0.000_4, "{mean}");
}

/// What an estimate holds at its peak, counted by hand.
///
/// MRS over 1024²: 4 × 4 tiles of 256², so 1 048 576 coefficients of 32 bytes and a bin byte,
/// 34 603 008. The tiles start at 0, 256, 512 and 768 on each axis and reach 32 past their core,
/// clipped to the frame: regions 288, 320, 320 and 288 wide. A worker's tile scratch has room for
/// the largest region, 320² pixels at 28 bytes, 2 867 200: 16 × 2 867 200 = 45 875 200 on 16
/// threads, 4 × 2 867 200 = 11 468 800 on 4. Each scratch holds a row of the widest region,
/// 320 × 4 bytes: 20 480 on 16, 5120 on 4. Each tile's place in the list, its share of the four
/// columns and its count are 64 + 64 + 8 bytes, 2176 for sixteen. A 192² frame is one tile, its
/// region the frame, which one worker of the 16 takes: 36 864 × 33 + 36 864 × 28 + 192 × 4 + 136.
#[test]
fn working_sets_are_what_the_buffers_hold() {
    let wide = USize2::new(1024, 1024);
    assert_eq!(
        mrs_noise::heap_bytes(wide, 16),
        34_603_008 + 45_875_200 + 20_480 + 2176
    );
    assert_eq!(
        mrs_noise::heap_bytes(wide, 4),
        34_603_008 + 11_468_800 + 5120 + 2176
    );
    assert_eq!(
        mrs_noise::heap_bytes(USize2::new(192, 192), 16),
        36_864 * 33 + 36_864 * 28 + 192 * 4 + 136
    );
}

/// The step's σ is `1/√12` to the nearest f32: the f64 value, 0.28867513459…, rounds to it.
#[test]
fn the_quantization_sigma_is_one_over_root_twelve() {
    assert_eq!(QUANTIZATION_SIGMA_PER_STEP, (1.0f64 / 12.0).sqrt() as f32);
}
