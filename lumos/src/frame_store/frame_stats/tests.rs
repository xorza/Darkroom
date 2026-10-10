use std::sync::Arc;

use imaginarium::Buffer2;

use crate::frame_store::frame_stats::FrameStats;
use crate::internals::cfa::make_cfa;
use crate::internals::test_rng::TestRng;
use crate::io::image::cfa::CfaType;
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

const SEED: u64 = 7;

/// An RGGB mosaic of constant colours, red 1/8, green 1/4, blue 3/8: each colour's sky and median
/// is its own level, where the whole-mosaic median, 1/4, would put red's 1/8 below its sky and
/// blue's 3/8 above it, and a constant colour has MAD 0. The noise model takes the quantization σ
/// 1/16 where the measured noise is 0, and the electrons per unit from 2 e⁻/ADU over a declared
/// 1000 ADU: 2000.
#[test]
fn a_mosaic_has_a_sky_per_colour() {
    let size = Size2us::new(8, 8);
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    let pixels = (0..size.pixel_count())
        .map(
            |index| match cfa.color_at(Vec2us::new(index % 8, index / 8)) {
                0 => 0.125,
                1 => 0.25,
                _ => 0.375,
            },
        )
        .collect();
    let mut image = make_cfa(size, pixels, cfa);
    image.metadata.quantization_sigma = Some(1.0 / 16.0);
    image.metadata.egain = Some(2.0);
    image.metadata.domain = Some(SampleDomain {
        scale: 1000.0,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Removed,
        unit: None,
    });
    let stats = FrameStats::measure(&image);
    assert_eq!(stats.sky.as_slice(), [0.125, 0.25, 0.375]);
    for (slot, level) in [0.125, 0.25, 0.375].into_iter().enumerate() {
        assert_eq!(stats.medians[slot], level, "slot {slot}");
        assert_eq!(stats.slot_noise(slot), 1.0 / 16.0, "slot {slot}");
    }
    assert_eq!(
        stats.ccd_noise(2),
        CcdNoise {
            background: BackgroundSplit::unflattened(0.0),
            quantization_variance: 1.0 / 256.0,
            sky: 0.375,
            electrons_per_unit: Some(2000.0),
        }
    );
    assert_eq!(stats.ccd_noise(2).background_at(1.0), 1.0 / 256.0);
}

/// A frame whose noise follows `A·g² + S·g` across a flat whose gain rises from 1 to 3 along
/// its 512 columns measures the two terms back: `σ²` = A + S and `ρ` = A/(A + S). A mono frame
/// 256 rows tall is read by the multiresolution support over two 256² tiles, 16 k coefficients
/// a bin, whose variances carry a standard error of √(2/16k) = 1.1%; the fit extrapolates to
/// gain 1 from the lowest bin near 1.1, so 6% and 0.06 hold `σ²` and `ρ` at about five standard
/// errors. A 512² mosaic's colours are read from pairs of neighbours, 8 k a bin for red and
/// blue, whose MAD-based variance carries about twice that error, so 10% and 0.2. All read
/// noise and all sky are told apart: `ρ` at 1 and at 0.
#[test]
fn a_flat_divided_frame_splits_its_noise_by_the_flat_gain() {
    let gain = |x: usize| 1.0 + 2.0 * x as f64 / 511.0;
    let frame = |size: Size2us, read: f64, sky: f64| {
        let mut rng = TestRng::new(SEED);
        let pixels: Vec<f32> = (0..size.pixel_count())
            .map(|index| {
                let g = gain(index % size.width);
                let sigma = (read * g * g + sky * g).sqrt();
                (0.5 + sigma * f64::from(rng.next_gaussian_f32())) as f32
            })
            .collect();
        let divisor = Buffer2::new(
            size.width,
            size.height,
            (0..size.pixel_count())
                .map(|index| (1.0 / gain(index % size.width)) as f32)
                .collect(),
        );
        (pixels, divisor)
    };
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    for (read, sky) in [(1e-4f64, 1e-4f64), (2e-4, 0.0), (0.0, 2e-4)] {
        let expected_share = (read / (read + sky)) as f32;
        let check = |stats: FrameStats, variance_tolerance: f32, share_tolerance: f32| {
            for (slot, (&sigma, &share)) in stats.noise.iter().zip(&stats.read_share).enumerate() {
                let variance = sigma * sigma;
                assert!(
                    (variance / 2e-4 - 1.0).abs() <= variance_tolerance,
                    "A {read} S {sky} slot {slot}: σ² {variance}"
                );
                assert!(
                    (share - expected_share).abs() <= share_tolerance,
                    "A {read} S {sky} slot {slot}: ρ {share}"
                );
            }
        };
        let size = Size2us::new(512, 256);
        let (pixels, divisor) = frame(size, read, sky);
        let mut mono = LinearImage::from_pixels(ImageDimensions::new(size, 1), pixels);
        mono.metadata.flat_gain = Some(Arc::new(FlatGain::of_divisor(
            &divisor,
            &CfaType::Mono,
            |_| false,
        )));
        check(FrameStats::measure(&mono), 0.06, 0.06);
        let size = Size2us::new(512, 512);
        let (pixels, divisor) = frame(size, read, sky);
        let mut mosaic = make_cfa(size, pixels, cfa);
        mosaic.metadata.flat_gain = Some(Arc::new(FlatGain::of_divisor(&divisor, &cfa, |_| false)));
        check(FrameStats::measure(&mosaic), 0.1, 0.2);
    }
}
