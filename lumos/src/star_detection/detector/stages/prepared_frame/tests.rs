use crate::internals::prelude::*;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::star_detection::detector::stages::prepared_frame::*;

/// `image` reduced to one plane, as the measurement plane is before its sky goes.
fn prepare(image: &LinearImage, pool: &mut DetectionResources) -> Buffer2<f32> {
    let mut plane = pool.acquire_f32();
    combine_channels(image, &mut plane);
    plane
}

/// Build a 16-pixel channel whose median is `center` and MAD is exactly `mad`
/// (8 pixels at `center - mad`, 8 at `center + mad`).
fn channel_with_mad(center: f32, mad: f32) -> Vec<f32> {
    let mut v = vec![center - mad; 8];
    v.extend(vec![center + mad; 8]);
    v
}

#[test]
fn prepare_uniform() {
    let dim = ImageDimensions::new((64, 64), 1);
    let data = vec![0.5f32; 64 * 64];
    let image = LinearImage::from_pixels(dim, data);

    let mut pool = DetectionResources::new(Size2us::new(64, 64));
    let result = prepare(&image, &mut pool);

    assert_eq!(result.width(), 64);
    assert_eq!(result.height(), 64);
    for &v in result.pixels() {
        assert!((v - 0.5).abs() < 1e-6);
    }
}

#[test]
fn prepare_with_star() {
    let width = 64;
    let height = 64;
    let mut data = vec![0.1f32; width * height];
    // Add bright pixel (simulating a star)
    data[32 * width + 32] = 0.9;

    let dim = ImageDimensions::new((width, height), 1);
    let image = LinearImage::from_pixels(dim, data);

    let mut pool = DetectionResources::new(Size2us::new(width, height));
    let result = prepare(&image, &mut pool);

    // Star pixel should be preserved (no CFA, no defects)
    assert!((result[(32, 32)] - 0.9).abs() < 1e-6);
}

/// The measurement plane is never filtered, and the detection plane takes the 3×3 median only
/// when the frame was interpolated. An isolated spike of 7/8 on a sky of 1/8 is what the median
/// erases, since 8 of its 9 neighbours sit at the sky: the sky comes out exactly, the spike's
/// residual is 3/4 on the measurement plane, and on the detection plane 0 after the median, 3/4
/// without it.
#[test]
fn the_median_filter_follows_interpolation() {
    #[derive(Debug)]
    struct Case {
        demosaic: Option<DemosaicProvenance>,
        /// The spike on the detection plane.
        detected: f32,
    }

    let size = Size2us::new(8, 8);
    let mut data = vec![0.125f32; size.pixel_count()];
    data[4 * size.width + 4] = 0.875;

    let cases = [
        // Interpolated: the artifacts the filter exists for are present.
        Case {
            demosaic: Some(DemosaicProvenance::LumosRcd),
            detected: 0.0,
        },
        // libraw's own processing interpolates too.
        Case {
            demosaic: Some(DemosaicProvenance::LibRaw),
            detected: 0.0,
        },
        // A monochrome sensor's plane is measured, not interpolated.
        Case {
            demosaic: Some(DemosaicProvenance::None),
            detected: 0.75,
        },
        // No provenance at all: nothing claims an interpolation, so nothing is suppressed.
        Case {
            demosaic: None,
            detected: 0.75,
        },
    ];

    let config = Config::default();
    for case in cases {
        let mut image = LinearImage::from_pixels(ImageDimensions::new(size, 1), data.clone());
        image.metadata.provenance = case.demosaic.map(|demosaic| ImageProvenance {
            container: SourceContainer::CameraRaw,
            decoder: DecoderProvenance::LibRaw,
            transfer: TransferProvenance::RawNormalized,
            color: ColorProvenance::SensorRgb,
            clipped: true,
            demosaic,
            row_order: RowOrder::TopDown,
        });

        let mut pool = DetectionResources::new(size);
        let frame = PreparedFrame::new(&image, &config, &mut pool);
        assert_eq!(frame.measure[(4, 4)], 0.75, "{case:?}");
        assert_eq!(frame.measure[(0, 0)], 0.0, "{case:?}");
        let plane = frame.detection_plane(None, &config, &mut pool);
        assert_eq!(plane.values[(4, 4)], case.detected, "{case:?}");
        assert_eq!(plane.values[(0, 0)], 0.0, "{case:?}");
    }
}

/// Weights are `1/σ²` normalized: equal σ give a third each; σ 1/64, 1/64 and 1/16 give relative
/// weights 1, 1 and 1/16, so 16/33, 16/33 and 1/33. A tiny σ does not overflow, since only the
/// ratios enter. A channel with no noise takes the whole weight, shared among such channels.
#[test]
fn detection_weights_are_inverse_variances() {
    assert_eq!(inverse_variance_weights([0.25; 3]), [1.0 / 3.0; 3]);
    assert_eq!(
        inverse_variance_weights([1.0 / 64.0, 1.0 / 64.0, 1.0 / 16.0]),
        [16.0 / 33.0, 16.0 / 33.0, 1.0 / 33.0]
    );
    assert_eq!(
        inverse_variance_weights([1e-30, 1e-30, 4e-30]),
        inverse_variance_weights([1.0, 1.0, 4.0])
    );
    assert_eq!(inverse_variance_weights([0.0, 0.25, 0.0]), [0.5, 0.0, 0.5]);
    assert_eq!(inverse_variance_weights([0.0; 3]), [1.0 / 3.0; 3]);
}

/// The weights read the white noise, not the spread: a red channel carrying a strong gradient on
/// top of the same noise as green and blue keeps its weight. Noise σ 0.01 in every channel of a
/// 256 × 256 frame; red adds a ramp across the frame, whose MAD alone is 0.25. The MAD would put
/// red's weight near 0; the estimates differ by their own error, a few percent, so each weight is
/// within 0.03 of a third.
#[test]
fn detection_weights_ignore_signal() {
    let size = 256;
    let mut rng = TestRng::new(7);
    let mut channel = |ramp: bool| -> Vec<f32> {
        (0..size * size)
            .map(|index| {
                let gradient = if ramp {
                    (index % size) as f32 / size as f32
                } else {
                    0.0
                };
                0.5 + gradient + 0.01 * rng.next_gaussian_f32()
            })
            .collect()
    };
    let image = LinearImage::from_planar_channels(
        ImageDimensions::new((size, size), 3),
        vec![channel(true), channel(false), channel(false)],
    );
    let weights = inverse_variance_weights(channel_noise(&image));
    for weight in weights {
        assert!((weight - 1.0 / 3.0).abs() < 0.03, "{weights:?}");
    }
}

#[test]
fn prepare_rgb_equal_noise_is_mean() {
    // Distinct per-channel levels; a 4 × 4 frame is too small for the noise estimate, which reads
    // 0 in every channel, so the weights are equal and the detection plane is the plain mean.
    let dims = ImageDimensions::new((4, 4), 3);
    let r = channel_with_mad(0.30, 0.02);
    let g = channel_with_mad(0.50, 0.02);
    let b = channel_with_mad(0.70, 0.02);
    let image = LinearImage::from_planar_channels(dims, vec![r.clone(), g.clone(), b.clone()]);

    let mut pool = DetectionResources::new(Size2us::new(4, 4));
    let out = prepare(&image, &mut pool);

    for (i, &out_v) in out.pixels().iter().enumerate() {
        let expected = (r[i] + g[i] + b[i]) / 3.0;
        assert!(
            (out_v - expected).abs() < 1e-4,
            "pixel {i}: expected mean {expected}, got {out_v}"
        );
    }
}

#[test]
fn prepare_rgb_red_star_survives() {
    // A star bright only in R must remain prominent in the detection plane. With equal weights the
    // star peak lands at 1/3 of its R amplitude — far above Rec.709's 0.21× crush of red.
    let dims = ImageDimensions::new((4, 4), 3);
    let mut r = channel_with_mad(0.10, 0.01);
    r[5] = 0.90; // bright red star, off the symmetric background
    let g = channel_with_mad(0.10, 0.01);
    let b = channel_with_mad(0.10, 0.01);
    let image = LinearImage::from_planar_channels(dims, vec![r, g, b]);

    let mut pool = DetectionResources::new(Size2us::new(4, 4));
    let out = prepare(&image, &mut pool);

    // A 4 × 4 frame is too small for the noise estimate, which reads 0 in every channel, so the
    // weights are equal and the star pixel is the mean (0.90 + 0.09 + 0.09)/3 = 0.36, to the
    // rounding of three products and two sums (4ε).
    assert!(
        (out[5] - 0.36).abs() <= 4.0 * f32::EPSILON * 0.36,
        "red star pixel {}",
        out[5]
    );
}

/// Review item 9.3. Without decoder flags the test runs per channel at 0.95 of the ceiling: a
/// star clipped in green alone, (0.6, 1.0, 0.6), is saturated, though its channels average
/// 0.73; (0.9, 0.9, 0.9) is not. With decoder flags only the flags count, so a calibrated pixel
/// that a flat lifted to 0.99 is not saturated, and a flagged one at 0.5 is.
#[test]
fn saturation_is_marked_per_channel_or_from_the_decoders_flags() {
    use crate::internals::prelude::*;
    use crate::io::image::pixel_flags::PixelFlags;

    let size = Size2us::new(2, 1);
    let image = rgb_image(size, vec![0.6, 0.9], vec![1.0, 0.9], vec![0.6, 0.9]);
    let mut mask = BitBuffer2::new_default(size);
    mark_saturated(&image, &mut mask);
    assert_eq!([mask.get(0), mask.get(1)], [true, false]);

    let mut flagged = rgb_image(size, vec![0.99, 0.5], vec![0.99, 0.5], vec![0.99, 0.5]);
    flagged.metadata.saturation_flagged = true;
    flagged.flags = PixelFlags::from_fn(size, |index| {
        if index == 1 {
            QualityFlags::SATURATED
        } else {
            QualityFlags::default()
        }
    });
    mark_saturated(&flagged, &mut mask);
    assert_eq!([mask.get(0), mask.get(1)], [false, true]);
}

/// A demosaiced frame of pure noise, as the demosaic correlates it: white noise of σ 0.01 on a sky
/// of 0.1 laid out as an RGGB mosaic and interpolated by RCD.
fn demosaiced_noise(size: Size2us, seed: u64) -> LinearImage {
    use crate::internals::cfa::make_cfa;
    use crate::io::image::cfa::CfaType;
    use crate::io::raw::demosaic::bayer::CfaPattern;

    let mut rng = TestRng::new(seed);
    let pixels: Vec<f32> = (0..size.pixel_count())
        .map(|_| 0.1 + 0.01 * rng.next_gaussian_f32())
        .collect();
    let mut image = make_cfa(size, pixels, CfaType::Bayer(CfaPattern::Rggb))
        .demosaic(MarkesteijnPasses::One, &CancelToken::never())
        .unwrap();
    image.metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::CameraRaw,
        decoder: DecoderProvenance::LibRaw,
        transfer: TransferProvenance::RawNormalized,
        color: ColorProvenance::SensorRgb,
        clipped: true,
        demosaic: DemosaicProvenance::LumosRcd,
        row_order: RowOrder::TopDown,
    });
    image
}

/// Review item 9.2. On a demosaiced frame of pure noise, the detection plane at FWHM 4 passes about
/// the fraction of sky its threshold states, because its noise is measured on it: above 2σ the
/// Gaussian tail is 0.02275. Filtered pixels are correlated over some 36 px at FWHM 4, so a 64-px
/// tile holds about 110 independent samples, and its σ carries an 11% relative error. A threshold
/// against a σ that errs both ways passes more than it states, by ½·k³·φ(k)·var = 11% at k = 2;
/// the 768 × 768 frame's 16 000 independent samples add a standard deviation of 5%. The bound is
/// 25%, the bias and 3 of those. Against the measurement plane's σ, which the white-noise
/// normalization of the filter would carry over, the plane reads 1.6 times quieter than it is, and
/// 2σ passes 4 times the stated sky (0.089).
#[test]
fn the_detection_threshold_passes_the_sky_it_states() {
    let size = Size2us::new(768, 768);
    let image = demosaiced_noise(size, 11);
    let config = Config::default();
    let mut pool = DetectionResources::new(size);
    let frame = PreparedFrame::new(&image, &config, &mut pool);
    let plane = frame.detection_plane(Some(4.0), &config, &mut pool);
    let above = |k: f32, noise: &Buffer2<f32>| {
        let count = plane
            .values
            .pixels()
            .iter()
            .zip(noise.pixels())
            .filter(|&(&value, &sigma)| value > k * sigma)
            .count();
        count as f64 / size.pixel_count() as f64
    };
    let two = above(2.0, &plane.noise.noise);
    assert!((two / 0.02275 - 1.0).abs() <= 0.25, "{two}");
    let white = above(2.0, &frame.sky.noise);
    assert!(white > 3.0 * 0.02275, "{white}");
}

/// Review item 8.2. A 64 × 64 corner with no data, filled with the sky's 0.1, on white noise of σ
/// 0.01: its tile is masked out of the mesh and filled from its neighbours, so the sky's σ there is
/// the frame's, within 10%, five times what a 4096-sample MAD errs by, and not the fill's 0, which
/// would sink the threshold onto the fill. No pixel of the corner joins a candidate: of the 12 288
/// others a 4σ threshold passes about 0.4, and the bound of 10 lies far below the thousands a
/// collapsed threshold passes.
#[test]
fn pixels_with_no_data_stay_out_of_the_sky_and_the_threshold() {
    use crate::io::image::pixel_flags::PixelFlags;
    use crate::star_detection::detector::stages::detect::DetectResult;

    let size = Size2us::new(128, 128);
    let missing = |index: usize| index % size.width < 64 && index / size.width < 64;
    let mut rng = TestRng::new(5);
    let pixels: Vec<f32> = (0..size.pixel_count())
        .map(|index| {
            let noise = 0.01 * rng.next_gaussian_f32();
            if missing(index) { 0.1 } else { 0.1 + noise }
        })
        .collect();
    let mut image = gray_image(size, pixels);
    image.flags = PixelFlags::from_fn(size, |index| {
        if missing(index) {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    });
    let config = Config::default();
    let mut pool = DetectionResources::new(size);
    let frame = PreparedFrame::new(&image, &config, &mut pool);
    let sigma = frame.sky.noise[(32, 32)];
    assert!((sigma / 0.01 - 1.0).abs() <= 0.1, "{sigma}");
    let plane = frame.detection_plane(Some(4.0), &config, &mut pool);
    let detected =
        DetectResult::from_plane(&plane, frame.no_data.as_ref(), &config.detection, &mut pool);
    assert!(detected.pixels_above_threshold <= 10, "{detected:?}");
}
