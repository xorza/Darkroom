use crate::internals::prelude::*;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::star_detection::detector::stages::prepare::*;

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

#[test]
fn the_median_filter_follows_interpolation() {
    #[derive(Debug)]
    struct Case {
        demosaic: Option<DemosaicProvenance>,
        /// The spike after `prepare`: background once the median has erased it, else untouched.
        peak: f32,
    }

    // An isolated spike is exactly what a 3×3 median erases — 8 of its 9 neighbours sit at
    // background, so the median is background — which makes it a clean probe for whether the
    // filter ran at all.
    let size = Size2us::new(8, 8);
    let mut data = vec![0.1f32; size.pixel_count()];
    data[4 * size.width + 4] = 0.9;

    let cases = [
        // Interpolated: the artifacts the filter exists for are present.
        Case {
            demosaic: Some(DemosaicProvenance::LumosRcd),
            peak: 0.1,
        },
        // libraw's own processing interpolates too.
        Case {
            demosaic: Some(DemosaicProvenance::LibRaw),
            peak: 0.1,
        },
        // A monochrome sensor's plane is measured, not interpolated — nothing may smooth the PSF
        // that FWHM and flux are read off.
        Case {
            demosaic: Some(DemosaicProvenance::None),
            peak: 0.9,
        },
        // No provenance at all: nothing claims an interpolation, so nothing is suppressed.
        Case {
            demosaic: None,
            peak: 0.9,
        },
    ];

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
        let out = prepare(&image, &mut pool);

        assert_eq!(out[(4, 4)], case.peak, "{case:?}");
        // Only the spike is in play; the flat background is a fixed point of both paths.
        assert_eq!(out[(0, 0)], 0.1, "{case:?}");
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
