use super::*;

/// A value in [0, 1) for each index, spread with no structure a kernel could resonate with.
fn hash_unit(index: usize) -> f32 {
    (index.wrapping_mul(2_654_435_761) >> 8 & 0xffff) as f32 / 65536.0
}

/// A frame of `size` whose pixels lie in [1, 2), different for each `seed`.
fn textured_image(size: Size2us, seed: usize) -> LinearImage {
    let pixels = (0..size.pixel_count())
        .map(|i| 1.0 + hash_unit(i + seed * size.pixel_count()))
        .collect();
    gray_image(size, pixels)
}

/// With no rejection and equal frame weights, the combine of the drizzled frames is the
/// single-pass drizzle: `Σ_f W_f·(S_f / W_f) / Σ_f W_f = Σ S / Σ W`.
///
/// Only the rounding differs. The single pass sums `N = F·MAX_DEPOSITS` deposits per cell in f32,
/// each product and addition rounding once against a running total of at most `Σ|w|·x_max`, and
/// the quotient once: `(2N + 2)·ε·x_max·L` with `L = Σ|w| / Σw`. Each drizzled frame rounds the same
/// way over its own `MAX_DEPOSITS`, its quotient once more, and the combine's f64 mean once into
/// f32: `(2·MAX_DEPOSITS + 3)·ε·x_max·L`. Interior cells keep every frame's whole kernel, so `L` is
/// 1, or at most `LOBE_EXCESS` for Lanczos. The margin is the largest reach, Gaussian's 3 cells
/// plus a shift of 2 at scale 2, and one more.
#[test]
fn the_unrejected_combine_is_the_single_pass_drizzle() {
    const FRAMES: usize = 4;
    const MARGIN: usize = 6;

    let size = Size2us::new(16, 16);
    let shifts = [
        DVec2::new(0.3, -0.2),
        DVec2::new(-0.7, 0.45),
        DVec2::new(0.15, 0.8),
        DVec2::new(-0.4, -0.6),
    ];
    let transforms = shifts.map(Transform::translation);
    let images: Vec<LinearImage> = (0..FRAMES).map(|f| textured_image(size, f)).collect();
    let x_max = 2.0;
    let bound = (2.0 * (FRAMES as f32 + 1.0) * MAX_DEPOSITS + 5.0) * f32::EPSILON * x_max;
    for kernel in DrizzleKernel::ALL {
        let config = DrizzleConfig {
            fill_value: -1.0,
            ..usual_config(kernel)
        };
        let mut acc = accumulator(ImageDimensions::new(size, 1), config.clone());
        for (image, transform) in images.iter().zip(&transforms) {
            acc.add_image(image.clone(), transform, None);
        }
        let single = acc.finalize();
        let combined = drizzle_plain(drizzle_frames(images.clone(), &transforms), &config)
            .unwrap()
            .product;
        let lobes = if kernel == DrizzleKernel::Lanczos {
            LOBE_EXCESS
        } else {
            1.0
        };
        let out = single.image.dimensions().size();
        assert_eq!(combined.image.dimensions().size(), out, "{kernel:?}");
        for y in MARGIN..out.height - MARGIN {
            for x in MARGIN..out.width - MARGIN {
                let expected = single.image.channel(0)[(x, y)];
                let actual = combined.image.channel(0)[(x, y)];
                assert!(
                    (actual - expected).abs() <= bound * lobes,
                    "{kernel:?} ({x}, {y}): {actual}, single pass {expected}"
                );
            }
        }
    }
}

/// A frame that reaches a cell by a sliver weighs in by its sliver. Frame A, a flat 2, copies onto
/// the grid at weight 1. Frame B, a flat 8, is shifted by 0.9 along x: column 0 meets only the
/// first tenth of B's pixel 0, at weight 0.1, so it reads `(1·2 + 0.1·8) / 1.1 = 28/11`, and every
/// other column meets 0.9 and 0.1 of two pixels, weight 1, and reads `(2 + 8) / 2 = 5`. An equal
/// vote per frame would read 5 at column 0 too.
///
/// Four roundings reach the result: the sliver weight's two (the overlap in f64, then into f32),
/// B's drizzled value and the mean. Each moves it by less than 3ε: `6/1.1²·0.1ε`, `8·0.1/1.1·ε`
/// and half an ulp of 28/11.
#[test]
fn a_sliver_frame_weighs_in_by_its_sliver() {
    let size = Size2us::new(8, 8);
    let product = drizzle_plain(
        drizzle_frames(
            vec![constant_image(size, 2.0), constant_image(size, 8.0)],
            &[
                Transform::identity(),
                Transform::translation(DVec2::new(0.9, 0.0)),
            ],
        ),
        &kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
    )
    .unwrap()
    .product;
    let out = product.image.channel(0);
    let bound = 4.0 * 3.0 * f32::EPSILON;
    for y in 0..8 {
        let sliver = out[(0, y)];
        assert!(
            (sliver - 28.0 / 11.0).abs() <= bound,
            "row {y}: column 0 reads {sliver}"
        );
        for x in 1..8 {
            assert!(
                (out[(x, y)] - 5.0).abs() <= bound,
                "({x}, {y}): {}",
                out[(x, y)]
            );
        }
    }
}

/// A satellite trail in one frame of ten is rejected. Ten dithered frames of a sky at 1 with a
/// noise of ±0.005, and frame 3 crossed by a trail of 100 along input row 64, columns 40 to 87. A
/// cell the trail reaches by any share `a` reads `1 + 99a` in that frame, hundreds of noise σ off
/// for any share the dither makes, so the light preset's σ-clip removes it: every cell reads the
/// sky to within the noise. The plain mean of the same frames shows the trail, so the fixture
/// does reach the combine. The frame leaves sky beyond the trail's 30-pixel reach in the noise
/// estimate's coarsest layer, which the light preset's noise weighting needs.
#[test]
fn a_satellite_trail_in_one_frame_is_rejected() {
    const FRAMES: usize = 10;
    const TRAIL_FRAME: usize = 3;

    let size = Size2us::new(128, 128);
    let images: Vec<LinearImage> = (0..FRAMES)
        .map(|f| {
            let mut pixels: Vec<f32> = (0..size.pixel_count())
                .map(|i| 1.0 + 0.01 * (hash_unit(i + f * size.pixel_count()) - 0.5))
                .collect();
            if f == TRAIL_FRAME {
                for x in 40..88 {
                    pixels[size.index_of(Vec2us::new(x, 64))] = 100.0;
                }
            }
            gray_image(size, pixels)
        })
        .collect();
    let transforms: Vec<Transform> = (0..FRAMES)
        .map(|f| {
            Transform::translation(DVec2::new(
                (f as f64 * 0.37).fract() - 0.5,
                (f as f64 * 0.71).fract() - 0.5,
            ))
        })
        .collect();
    let config = DrizzleConfig {
        fill_value: -1.0,
        ..usual_config(DrizzleKernel::Turbo)
    };
    let stack = StackConfig {
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let rejected = drizzle_images(
        drizzle_frames(images.clone(), &transforms),
        &config,
        &stack,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .product;
    let plain = drizzle_plain(drizzle_frames(images, &transforms), &config)
        .unwrap()
        .product;
    for (index, &value) in rejected.image.channel(0).pixels().iter().enumerate() {
        assert!(
            value == -1.0 || (value - 1.0).abs() <= 0.005,
            "pixel {index}: {value}"
        );
    }
    let brightest = plain
        .image
        .channel(0)
        .pixels()
        .iter()
        .copied()
        .fold(0.0f32, f32::max);
    assert!(
        brightest > 5.0,
        "the plain mean shows no trail: {brightest}"
    );
}
