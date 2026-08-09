use crate::background_mesh::tile_stats::*;
use crate::testing::prelude::*;

#[test]
fn sextractor_sky_hand_computed() {
    let stats = |median: f32, mean: f32, sigma: f32| ClippedStats {
        median,
        sigma,
        mean,
    };
    // Mild skew (|mean−median| = 0.2 < 0.3σ): Pearson mode 2.5·100 − 1.5·100.2 = 99.7,
    // pulled below the median toward the histogram peak.
    let sky = sextractor_sky(&stats(100.0, 100.2, 1.0));
    assert!((sky - 99.7).abs() < 1e-4, "mode = 99.7, got {sky}");
    // Strong skew (1.0 ≥ 0.3σ): the mode extrapolation is unreliable → plain median.
    assert_eq!(sextractor_sky(&stats(100.0, 101.0, 1.0)), 100.0);
    // Symmetric histogram: mode = 2.5·m − 1.5·m = m — estimator changes nothing.
    assert_eq!(sextractor_sky(&stats(100.0, 100.0, 1.0)), 100.0);
    // Uniform tile (σ = 0): |0| < 0 is false → median fallback.
    assert_eq!(sextractor_sky(&stats(5.0, 5.0, 0.0)), 5.0);
}

#[test]
fn collect_sampled_pixels_small_tile() {
    let pixels = Buffer2::new_filled(32, 32, 0.5);
    let mut values = Vec::new();
    collect_sampled_pixels(
        &pixels,
        URect::new(Vec2us::ZERO, Vec2us::new(32, 32)),
        &mut values,
    );
    // Small tile should collect all or most pixels
    assert!(values.len() >= 100);
    assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
}

#[test]
fn collect_sampled_pixels_large_tile() {
    let pixels = Buffer2::new_filled(256, 256, 0.5);
    let mut values = Vec::new();
    collect_sampled_pixels(
        &pixels,
        URect::new(Vec2us::ZERO, Vec2us::new(256, 256)),
        &mut values,
    );
    assert!(values.len() <= MAX_TILE_SAMPLES);
    assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
}

#[test]
fn collect_unmasked_pixels_none_masked() {
    let pixels = Buffer2::new_filled(64, 64, 0.5);
    let mask = BitBuffer2::new_filled(Size2us::new(64, 64), false);
    let mut values = Vec::new();
    collect_unmasked_pixels(
        &pixels,
        &mask,
        URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
        &mut values,
    );
    assert_eq!(values.len(), MAX_TILE_SAMPLES);
}

#[test]
fn collect_unmasked_pixels_all_masked() {
    let pixels = Buffer2::new_filled(64, 64, 0.5);
    let mask = BitBuffer2::new_filled(Size2us::new(64, 64), true);
    let mut values = Vec::new();
    collect_unmasked_pixels(
        &pixels,
        &mask,
        URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
        &mut values,
    );
    assert!(values.is_empty());
}

#[test]
fn collect_unmasked_pixels_partial_mask() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5);

    // Mask every other pixel
    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);
    for y in 0..height {
        for x in 0..width {
            if (x + y) % 2 == 0 {
                mask.set_at(Vec2us::new(x, y), true);
            }
        }
    }

    let mut values = Vec::new();
    collect_unmasked_pixels(
        &pixels,
        &mask,
        URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
        &mut values,
    );
    assert_eq!(values.len(), MAX_TILE_SAMPLES);
}

#[test]
fn collect_unmasked_pixels_partial_tile() {
    let pixels = Buffer2::new_filled(100, 100, 0.5);
    let mask = BitBuffer2::new_filled(Size2us::new(100, 100), false);
    let mut values = Vec::new();
    collect_unmasked_pixels(
        &pixels,
        &mask,
        URect::new(Vec2us::new(10, 20), Vec2us::new(70, 80)),
        &mut values,
    );
    assert_eq!(values.len(), MAX_TILE_SAMPLES);
}

#[test]
fn masked_sampling_matches_evenly_spaced_unmasked_ordinals() {
    #[derive(Debug)]
    struct SamplingCase {
        size: Size2us,
        tile: URect,
        mask_modulus: Option<usize>,
    }

    let cases = [
        SamplingCase {
            size: Size2us::new(17, 19),
            tile: URect::new(Vec2us::ZERO, Vec2us::new(17, 19)),
            mask_modulus: None,
        },
        SamplingCase {
            size: Size2us::new(32, 32),
            tile: URect::new(Vec2us::ZERO, Vec2us::new(32, 32)),
            mask_modulus: None,
        },
        SamplingCase {
            size: Size2us::new(40, 40),
            tile: URect::new(Vec2us::ZERO, Vec2us::new(40, 40)),
            mask_modulus: Some(7),
        },
        SamplingCase {
            size: Size2us::new(130, 75),
            tile: URect::new(Vec2us::new(3, 2), Vec2us::new(129, 74)),
            mask_modulus: Some(5),
        },
        SamplingCase {
            size: Size2us::new(256, 256),
            tile: URect::new(Vec2us::ZERO, Vec2us::new(256, 256)),
            mask_modulus: None,
        },
    ];

    for case in cases {
        let size = case.size;
        let pixels = Buffer2::new(
            size.width,
            size.height,
            (0..size.pixel_count()).map(|i| i as f32).collect(),
        );
        let mut mask = BitBuffer2::new_filled(size, false);
        if let Some(modulus) = case.mask_modulus {
            for y in 0..size.height {
                for x in 0..size.width {
                    if (x + 3 * y) % modulus == 0 {
                        mask.set_at(Vec2us::new(x, y), true);
                    }
                }
            }
        }

        let mut expected: Vec<f32> = (case.tile.min.y..case.tile.max.y)
            .flat_map(|y| {
                let mask = &mask;
                let pixels = &pixels;
                (case.tile.min.x..case.tile.max.x)
                    .filter(move |&x| !mask.get_at(Vec2us::new(x, y)))
                    .map(move |x| pixels[size.index_of(Vec2us::new(x, y))])
            })
            .collect();
        reference_subsample(&mut expected, MAX_TILE_SAMPLES);

        let mut actual = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, case.tile, &mut actual);

        assert_eq!(actual, expected, "case: {case:?}");
        assert!(
            actual.capacity() <= MAX_TILE_SAMPLES,
            "case retained {} samples: {case:?}",
            actual.capacity()
        );
    }
}
