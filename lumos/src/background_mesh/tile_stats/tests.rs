#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::background_mesh::tile_stats::*;
use crate::internals::prelude::*;

#[test]
fn sextractor_sky_hand_computed() {
    let stats = |median: f32, mean: f32, sigma: f32| ClippedStats {
        median,
        sigma,
        mean,
    };
    // Mild skew (|mean−median| = 0.2 < 0.3σ): Pearson mode 2.5·100 − 1.5·100.2 = 99.7,
    // pulled below the median toward the histogram peak. 2.5·100 is exact; 1.5·100.2 and the
    // difference round once each, by half an ulp of 150 and of 99.7: 1.2e-5.
    let sky = sextractor_sky(&stats(100.0, 100.2, 1.0));
    let mode = 250.0 - 1.5 * f64::from(100.2f32);
    assert!(
        (f64::from(sky) - mode).abs() <= 1.2e-5,
        "mode = {mode}, got {sky}"
    );
    // Strong skew (1.0 ≥ 0.3σ): the mode extrapolation is unreliable → plain median.
    assert_eq!(sextractor_sky(&stats(100.0, 101.0, 1.0)), 100.0);
    // Symmetric histogram: mode = 2.5·m − 1.5·m = m — estimator changes nothing.
    assert_eq!(sextractor_sky(&stats(100.0, 100.0, 1.0)), 100.0);
    // Uniform tile (σ = 0): |0| < 0 is false → median fallback.
    assert_eq!(sextractor_sky(&stats(5.0, 5.0, 0.0)), 5.0);
}

/// Hand rows, then the invariants over a sweep: strictly increasing, gaps within one of the even
/// spacing `n/m` (the two halves meet one wider, where the mirrored floors round apart), and
/// `o_k + o_(m−1−k) = n − 1`.
#[test]
fn sample_ordinals_are_even_and_point_symmetric() {
    let ordinals = |count: usize, candidates: usize| -> Vec<usize> {
        (0..count)
            .map(|k| sample_ordinal(k, count, candidates))
            .collect()
    };
    // ⌊((2k+1)·10 − 4)/8⌋ = 0, 3, then the mirror 9 − 3, 9 − 0.
    assert_eq!(ordinals(4, 10), [0, 3, 6, 9]);
    // An odd count of an odd n keeps the middle ordinal (9 − 1)/2 = 4.
    assert_eq!(ordinals(3, 9), [1, 4, 7]);
    assert_eq!(ordinals(10, 10), (0..10).collect::<Vec<_>>());

    for candidates in (2..3000).step_by(7).chain([4096, 65536]) {
        for count in [2, 4, 64, MAX_TILE_SAMPLES] {
            if count > candidates {
                continue;
            }
            let picked = ordinals(count, candidates);
            let (low, high) = (candidates / count, candidates.div_ceil(count) + 1);
            for pair in picked.windows(2) {
                let gap = pair[1] - pair[0];
                assert!(
                    (low..=high).contains(&gap),
                    "{count} of {candidates}: gap {gap} outside {low}..={high}"
                );
            }
            for (k, &ordinal) in picked.iter().enumerate() {
                assert_eq!(
                    ordinal + picked[count - 1 - k],
                    candidates - 1,
                    "{count} of {candidates}: k = {k}"
                );
            }
        }
    }
}

/// A 64×64 tile at (10, 20) in a frame of `x + 1000·y`: 1024 of its 4096 pixels, centred on the
/// tile centre (41.5, 51.5) — their mean is the plane there, exactly, as every value and the sum
/// are exact in f64. Each offset is the doubled step from that centre to the pixel its value
/// names: `2·v = 2·41.5 + o.x + 1000·(2·51.5 + o.y)`.
#[test]
fn tile_samples_centre_on_the_tile() {
    let size = Size2us::new(100, 100);
    let pixels = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| (i % size.width + 1000 * (i / size.width)) as f32)
            .collect(),
    );
    let tile = URect::new(Vec2us::new(10, 20), Vec2us::new(74, 84));
    let (mut values, mut offsets) = (Vec::new(), Vec::new());
    collect_tile_pixels(&pixels, tile, &mut values, &mut offsets);
    assert_eq!(values.len(), MAX_TILE_SAMPLES);
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / values.len() as f64;
    assert_eq!(mean, 41.5 + 1000.0 * 51.5);
    for (&v, &[ox, oy]) in values.iter().zip(&offsets) {
        assert_eq!(2 * v as i32, 83 + ox + 1000 * (103 + oy), "{v}");
    }

    let small = URect::new(Vec2us::new(3, 4), Vec2us::new(13, 9));
    values.clear();
    offsets.clear();
    collect_tile_pixels(&pixels, small, &mut values, &mut offsets);
    let every: Vec<f32> = (4..9)
        .flat_map(|y| (3..13).map(move |x| (x + 1000 * y) as f32))
        .collect();
    assert_eq!(values, every, "a tile within the budget reads every pixel");
}

/// The unmasked pixels of a tile, read in raster order and sampled by [`sample_ordinal`] past the
/// budget — on a frame of `i`, where every value names its pixel. The mask is every pixel with
/// `(x + 3y) mod m = 0`; `m = 1` masks the whole tile, which reads nothing.
#[test]
fn masked_sampling_reads_the_unmasked_ordinals() {
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
        SamplingCase {
            size: Size2us::new(64, 64),
            tile: URect::new(Vec2us::ZERO, Vec2us::new(64, 64)),
            mask_modulus: Some(1),
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

        let expected: Vec<f32> = (case.tile.min.y..case.tile.max.y)
            .flat_map(|y| {
                let mask = &mask;
                let pixels = &pixels;
                (case.tile.min.x..case.tile.max.x)
                    .filter(move |&x| !mask.get_at(Vec2us::new(x, y)))
                    .map(move |x| pixels[size.index_of(Vec2us::new(x, y))])
            })
            .collect();
        let count = expected.len().min(MAX_TILE_SAMPLES);
        let expected: Vec<f32> = (0..count)
            .map(|k| expected[sample_ordinal(k, count, expected.len())])
            .collect();

        let (mut actual, mut offsets) = (Vec::new(), Vec::new());
        collect_unmasked_pixels(&pixels, &mask, case.tile, &mut actual, &mut offsets);

        assert_eq!(actual, expected, "case: {case:?}");
        let named: Vec<[i32; 2]> = actual
            .iter()
            .map(|&v| {
                let i = v as usize;
                centred(case.tile, i % size.width, i / size.width)
            })
            .collect();
        assert_eq!(offsets, named, "case: {case:?}");
        assert!(
            actual.capacity() <= MAX_TILE_SAMPLES,
            "case retained {} samples: {case:?}",
            actual.capacity()
        );
    }
}

/// A tile's sky reads its samples less the tile's own plane, so on a planar sky it equals the sky
/// of the same tile levelled to the plane's value at its centre (19.5, 21.5):
/// `0.25·19.5 + 2·21.5 = 47.875`. Exactly: every value, the fitted slope and every difference are
/// exact. A ±0.5 checkerboard gives both tiles the same noise, and is uncorrelated with the
/// position over the tile, so the fit leaves it alone. A masked 4×4 block removes as many of each
/// sign from every row and column it covers, which keeps that true. σ reads the raw samples, so
/// the slope's spread counts in it.
#[test]
fn a_tile_reads_its_sky_about_its_own_plane() {
    use crate::background_mesh::workspace::TileScratch;

    let size = Size2us::new(40, 40);
    // 32×32 = 1024 pixels: the whole tile is read, so the samples are exactly the pixels.
    let tile = URect::new(Vec2us::new(4, 6), Vec2us::new(36, 38));
    let frame = |level: &dyn Fn(usize, usize) -> f32, checker: f32| {
        let pixels = (0..size.pixel_count())
            .map(|i| {
                let (x, y) = (i % size.width, i / size.width);
                let sign = if (x + y).is_multiple_of(2) { 1.0 } else { -1.0 };
                level(x, y) + sign * checker
            })
            .collect();
        Buffer2::new(size.width, size.height, pixels)
    };
    let plane = |x: usize, y: usize| 0.25 * x as f32 + 2.0 * y as f32;
    let flat = |_: usize, _: usize| 47.875;
    let mut star = BitBuffer2::new_filled(size, false);
    for y in 12..16 {
        for x in 20..24 {
            star.set_at(Vec2us::new(x, y), true);
        }
    }

    let mut scratch = TileScratch::default();
    for mask in [None, Some(&star)] {
        let mut stats = |pixels: &Buffer2<f32>| {
            TileStats::compute(pixels, mask, None, tile, 3, &mut scratch).unwrap()
        };
        let masked = mask.is_some();
        assert_eq!(
            stats(&frame(&plane, 0.0)).sky,
            47.875,
            "a bare plane, mask {masked}"
        );
        let levelled = stats(&frame(&flat, 0.5));
        let sloped = stats(&frame(&plane, 0.5));
        assert_eq!(sloped.sky, levelled.sky, "mask {masked}");
        assert!(
            sloped.sigma > 10.0 * levelled.sigma,
            "the raw σ counts the slope: {} against {}, mask {masked}",
            sloped.sigma,
            levelled.sigma
        );
    }
}
