#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use std::f64::consts::PI;

use super::*;

/// Output pixels a drop's deposits share at most: the Lanczos-3 and Gaussian neighbourhoods are
/// 7×7.
const MAX_DEPOSITS: f32 = 49.0;

/// How far `Σ|wᵢ|` can exceed `Σwᵢ` at an interior pixel: Lanczos-3's negative lobes, about 1.3 per
/// axis. The other kernels' weights are all positive, so it is 1 for them.
const LOBE_EXCESS: f32 = 1.7;

/// An image of `value` with `pixel` set to `bright`.
fn one_pixel_image(size: Size2us, value: f32, pixel: Vec2us, bright: f32) -> LinearImage {
    let mut pixels = vec![value; size.pixel_count()];
    pixels[size.index_of(pixel)] = bright;
    gray_image(size, pixels)
}

/// A pixel-weight map that lets only `pixel` deposit.
fn only(size: Size2us, pixel: Vec2us) -> Buffer2<f32> {
    let mut weights = Buffer2::new_filled(size.width, size.height, 0.0);
    weights[(pixel.x, pixel.y)] = 1.0;
    weights
}

/// Every pixel holding at least `gate` of the deepest weight reads `value`, and every other the
/// fill: the gate is `min_weight_fraction`, and the product was made with it.
///
/// For a constant, `Σ v·wᵢ / Σwᵢ` leaves only rounding. A pixel sums at most [`MAX_DEPOSITS`]
/// products, each product and sum rounding once against a running total of at most `Σ|wᵢ|·|v|`,
/// and the division once more. `Σ|wᵢ|` is at most [`LOBE_EXCESS`] times the deepest weight, and
/// a pixel that passed the gate holds at least `gate` of it, so the ratio carries the rounding of
/// `Σ|wᵢ|` over `Σwᵢ` by at most `LOBE_EXCESS / gate`.
fn assert_constant_or_fill(product: &StackProduct, value: f32, fill: f32, gate: f32, case: &str) {
    let weight = weight_plane(product);
    let deepest = weight.pixels().iter().copied().fold(0.0f32, f32::max);
    let threshold = (gate * deepest).max(f32::MIN_POSITIVE);
    let bound = (2.0 * MAX_DEPOSITS + 1.0) * f32::EPSILON * value.abs() * LOBE_EXCESS / gate;
    let mut covered = 0;
    for (index, (&actual, &w)) in product
        .image
        .channel(0)
        .pixels()
        .iter()
        .zip(weight.pixels())
        .enumerate()
    {
        if w >= threshold {
            covered += 1;
            assert!(
                (actual - value).abs() <= bound,
                "{case} pixel {index}: {actual}, expected {value} (weight {w})"
            );
        } else {
            assert_eq!(actual, fill, "{case} pixel {index} (weight {w})");
        }
    }
    assert!(covered > 0, "{case}: nothing was covered");
}

/// A constant reads back as itself wherever enough weight landed, and as the fill everywhere else:
/// every kernel, under an identity, a sub-pixel shift and a rotation, for a positive and a negative
/// constant — background-subtracted data sits around zero, and a clamp would bias it upwards.
///
/// The gate is half the deepest weight: where a rotated Lanczos window cancels to a sliver at the
/// edge, the ratio would amplify its rounding without limit, and the gate is what production has
/// for that.
#[test]
fn a_constant_reads_back_wherever_weight_landed() {
    const GATE: f32 = 0.5;
    const FILL: f32 = -7.0;

    let size = Size2us::new(20, 20);
    let transforms = [
        ("identity", Transform::identity()),
        ("shift", Transform::translation(DVec2::new(0.3, -0.2))),
        (
            "rotation",
            Transform::rotation_around(DVec2::splat(9.5), 15.0f64.to_radians()),
        ),
    ];
    for kernel in DrizzleKernel::ALL {
        for (name, transform) in &transforms {
            for value in [3.0, -0.5] {
                let config = DrizzleConfig {
                    fill_value: FILL,
                    min_weight_fraction: GATE,
                    ..usual_config(kernel)
                };
                let product =
                    drizzle_one(size, config, constant_image(size, value), transform, None);
                assert_constant_or_fill(&product, value, FILL, GATE, &format!("{kernel:?} {name}"));
            }
        }
    }
}

/// Two frames of 2 and 6 at frame weights 1 and 3 combine to `(2·1 + 6·3) / 4` = 5 wherever they
/// landed, every kernel: both deposit through the same geometry, which divides out. The point
/// kernel at scale 2 leaves three cells in four to the fill.
#[test]
fn two_frames_combine_to_their_weighted_mean() {
    let size = Size2us::new(12, 12);
    for kernel in DrizzleKernel::ALL {
        let mut acc = accumulator(ImageDimensions::new(size, 1), usual_config(kernel));
        acc.add_image(constant_image(size, 2.0), &Transform::identity(), 1.0, None);
        acc.add_image(constant_image(size, 6.0), &Transform::identity(), 3.0, None);
        let product = acc.finalize().product;
        // Two frames' deposits, at the larger value.
        let bound = (4.0 * MAX_DEPOSITS + 1.0) * f32::EPSILON * 6.0 * LOBE_EXCESS;
        for (index, (&value, &weight)) in product
            .image
            .channel(0)
            .pixels()
            .iter()
            .zip(weight_plane(&product).pixels())
            .enumerate()
        {
            let expected = if weight > 0.0 { 5.0 } else { 0.0 };
            assert!(
                (value - expected).abs() <= bound,
                "{kernel:?} pixel {index}: {value}"
            );
        }
    }
}

/// At scale 2 and pixfrac 0.8 a drop is 1.6 output pixels wide, centred at `2p + ½`: input pixel
/// `p`'s drop covers `[2p − 0.3, 2p + 1.3]`, 0.8 of cells `2p` and `2p + 1` and nothing of the
/// cells beside them. Every cell holds a quarter of one drop — `0.8²/1.6²` — so it weighs 0.25 and
/// reads its own pixel undiluted: the bright pixel (1, 1) fills cells 2 and 3 on each axis, and its
/// neighbours' cells, covered just as much, read their 0. A shift of ½ input pixel moves every
/// block one cell on, and leaves cell 0 to the fill. The square kernel's clipped quadrilateral is
/// the same box while the map stays axis-aligned. Every quantity is exact in f32.
#[test]
fn one_bright_pixel_fills_its_own_block() {
    let size = Size2us::new(6, 6);
    let image = one_pixel_image(size, 0.0, Vec2us::new(1, 1), 2.0);
    for kernel in [DrizzleKernel::Turbo, DrizzleKernel::Square] {
        for shift in [0, 1] {
            let transform = Transform::translation(DVec2::splat(0.5 * f64::from(shift)));
            let config = DrizzleConfig {
                fill_value: -9.0,
                ..kernel_config(kernel, 2.0, 0.8)
            };
            let product = drizzle_one(size, config, image.clone(), &transform, None);
            let weight = weight_plane(&product);
            let block = 2 + shift as usize..4 + shift as usize;
            for y in 0..12 {
                for x in 0..12 {
                    let covered = x >= shift as usize && y >= shift as usize;
                    let expected = match (block.contains(&x) && block.contains(&y), covered) {
                        (true, _) => (2.0, 0.25),
                        (false, true) => (0.0, 0.25),
                        (false, false) => (-9.0, 0.0),
                    };
                    assert_eq!(
                        (product.image.channel(0)[(x, y)], weight[(x, y)]),
                        expected,
                        "{kernel:?} shift {shift} cell ({x}, {y}): value, weight"
                    );
                }
            }
        }
    }
}

/// The pixel fraction sets the drop, and so which cells it reaches and with what share.
///
/// At scale 2 a shift of 0.1 input pixel puts pixel (2, 2) at output (4.7, 4.7). At pixfrac 1 its
/// drop is `[3.7, 5.7]` per axis: 0.8 of cell 4, all of cell 5 and 0.2 of cell 6, shares 0.4, 0.5
/// and 0.1 of its side 2 — nine cells. At pixfrac 0.3 it is `[4.4, 5.0]`: 0.1 of cell 4 and 0.5 of
/// cell 5, shares 1/6 and 5/6 of its side 0.6 — four cells. A cell's weight is the product of its
/// two shares. Turbo and the square kernel agree, the drop being axis-aligned.
///
/// 0.3 is not exact in f32: its rounding moves the drop's edges and area by a relative 4e-8, and
/// the product of shares rounds once more, so 4ε holds.
#[test]
fn the_pixel_fraction_sets_the_cells_a_drop_reaches() {
    let size = Size2us::new(6, 6);
    let pixel = Vec2us::new(2, 2);
    let transform = Transform::translation(DVec2::splat(0.1));
    for kernel in [DrizzleKernel::Turbo, DrizzleKernel::Square] {
        for (pixfrac, first, shares) in [
            (1.0, 4, &[0.4, 0.5, 0.1][..]),
            (0.3, 4, &[1.0 / 6.0, 5.0 / 6.0][..]),
        ] {
            let product = drizzle_one(
                size,
                kernel_config(kernel, 2.0, pixfrac),
                constant_image(size, 1.0),
                &transform,
                Some(&only(size, pixel)),
            );
            let weight = weight_plane(&product);
            let reached = weight.pixels().iter().filter(|&&w| w != 0.0).count();
            assert_eq!(
                reached,
                shares.len() * shares.len(),
                "{kernel:?} pixfrac {pixfrac}"
            );
            for (j, &share_y) in shares.iter().enumerate() {
                for (i, &share_x) in shares.iter().enumerate() {
                    let actual = f64::from(weight[(first + i, first + j)]);
                    let expected = share_x * share_y;
                    assert!(
                        (actual - expected).abs() <= 4.0 * f64::from(f32::EPSILON),
                        "{kernel:?} pixfrac {pixfrac} cell ({}, {}): {actual}, expected {expected}",
                        first + i,
                        first + j
                    );
                }
            }
        }
    }
}

/// `min_weight_fraction` gates against the deepest weight.
///
/// pixfrac=0.5 at scale=2: every drop is one output pixel wide, and a shift of (0.1, 0) puts input
/// pixel (i, j) at output (2i + 0.7, 2j + ½). Along x the drop [2i + 0.2, 2i + 1.2] covers 0.3 of
/// cell 2i and 0.7 of cell 2i + 1; along y, [2j, 2j + 1] covers half of cells 2j and 2j + 1. So
/// every cell's weight is 0.3·0.5 = 0.15 or 0.7·0.5 = 0.35: the maximum is 0.35, the threshold
/// 0.6·0.35 = 0.21. The bright pixel (0,0)'s cells (1,0) and (1,1) weigh 0.35 and read 1.0; its
/// cells (0,0) and (0,1) weigh 0.15 — covered, but below the threshold — and take the fill.
#[test]
fn min_weight_fraction_gates_against_the_deepest_weight() {
    let size = Size2us::new(4, 4);
    let config = DrizzleConfig {
        min_weight_fraction: 0.6,
        ..kernel_config(DrizzleKernel::Turbo, 2.0, 0.5)
    };
    let product = drizzle_one(
        size,
        config,
        one_pixel_image(size, 0.0, Vec2us::new(0, 0), 1.0),
        &Transform::translation(DVec2::new(0.1, 0.0)),
        None,
    );
    let out = product.image.channel(0);
    let weight = weight_plane(&product);
    for (x, y) in [(1, 0), (1, 1)] {
        assert_eq!(out[(x, y)], 1.0, "cell ({x},{y}) kept");
    }
    for (x, y) in [(0, 0), (0, 1)] {
        assert_eq!(out[(x, y)], 0.0, "cell ({x},{y}) below the threshold");
        assert!(weight[(x, y)] > 0.0, "cell ({x},{y}) is covered");
    }
}

/// Lanczos's negative lobes survive into the image. Shifted half a pixel along x, a lone 100 at
/// (10, 10) lands at 10.5 and reaches output column 12 at distance 1½, where `L(1½) = −4/(3π²)`.
/// Its taps run over distances ½, 1½, 2½ either side and sum to `(2/π²)(6 − 4/3 + 6/25)`, so the
/// normalized tap is `(−4/3)/(2·368/75)` = −100/736, and every column receives the same sum of
/// normalized taps from the whole frame — 1 — so pixel (12, 10) reads −10000/736.
///
/// Each tap is the f32 kernel, within 1e-6 of the true one (see `math::lanczos`'s tests): the
/// numerator moves by that and the sum by six of them, at sizes 0.136 and 0.98. The deposits round
/// to 49·ε of the bright one.
#[test]
fn lanczos_keeps_its_negative_lobes() {
    let size = Size2us::new(20, 20);
    let product = drizzle_one(
        size,
        kernel_config(DrizzleKernel::Lanczos, 1.0, 1.0),
        one_pixel_image(size, 0.0, Vec2us::new(10, 10), 100.0),
        &Transform::translation(DVec2::new(0.5, 0.0)),
        None,
    );
    let actual = f64::from(product.image.channel(0)[(12, 10)]);
    let expected: f64 = -10000.0 / 736.0;
    let normalizer = 2.0 * 368.0 / (75.0 * PI * PI);
    let tap = 1e-6;
    let bound = 100.0 * (tap + 0.136 * 6.0 * tap / normalizer) / normalizer
        + f64::from(MAX_DEPOSITS * f32::EPSILON) * expected.abs();
    assert!(
        (actual - expected).abs() <= bound,
        "{actual}, expected {expected}"
    );
}

/// The point kernel puts a pixel's whole weight on the one cell nearest its centre. At scale 2 that
/// centre is `2p + ½`, the corner its 2×2 block of cells shares, a tie `f64::round` breaks upward:
/// every input pixel lands on the odd cell `(2p + 1)`, at weight 1 and read back exactly, and the
/// even cells take the fill with no weight and no coverage.
#[test]
fn the_point_kernel_lands_each_pixel_on_one_odd_cell() {
    let size = Size2us::new(4, 4);
    let image = gray_image(size, (0..16).map(|i| i as f32 + 1.0).collect());
    let config = DrizzleConfig {
        fill_value: -999.0,
        ..kernel_config(DrizzleKernel::Point, 2.0, 0.8)
    };
    let product = drizzle_one(size, config, image, &Transform::identity(), None);
    let weight = weight_plane(&product);
    let coverage = product.coverage.as_ref().unwrap();
    for y in 0..8 {
        for x in 0..8 {
            let expected = if x % 2 == 1 && y % 2 == 1 {
                ((x / 2 + y / 2 * 4) as f32 + 1.0, 1.0, 1.0)
            } else {
                (-999.0, 0.0, 0.0)
            };
            assert_eq!(
                (
                    product.image.channel(0)[(x, y)],
                    weight[(x, y)],
                    coverage[(x, y)]
                ),
                expected,
                "cell ({x}, {y}): value, weight, coverage"
            );
        }
    }
}

/// At scale 1 and pixfrac 1 a drop is its own output pixel, whole: the compact kernels copy the
/// input and weigh every pixel 1, exactly.
#[test]
fn unit_sampling_copies_the_input() {
    let size = Size2us::new(5, 5);
    let pixels: Vec<f32> = (0..25).map(|i| i as f32 * 0.75 - 4.0).collect();
    for kernel in [
        DrizzleKernel::Turbo,
        DrizzleKernel::Square,
        DrizzleKernel::Point,
    ] {
        let product = drizzle_one(
            size,
            kernel_config(kernel, 1.0, 1.0),
            gray_image(size, pixels.clone()),
            &Transform::identity(),
            None,
        );
        assert_eq!(product.image.channel(0).pixels(), &pixels[..], "{kernel:?}");
        assert!(
            weight_plane(&product).pixels().iter().all(|&w| w == 1.0),
            "{kernel:?}"
        );
    }
}

/// An unmagnified drop deposits its frame weight in total, whatever its kernel spreads it over:
/// the shares of a drop sum to 1, and a frame registered at unit scale is not magnified. One pixel
/// at frame weight 2.5, well inside the grid, so nothing of it falls off.
///
/// The weight plane's sum adds at most 49 deposits, each a share rounded once.
#[test]
fn an_unmagnified_drop_deposits_its_frame_weight() {
    let size = Size2us::new(16, 16);
    let pixel = Vec2us::new(7, 7);
    for kernel in DrizzleKernel::ALL {
        let mut acc = accumulator(ImageDimensions::new(size, 1), usual_config(kernel));
        acc.add_image(
            constant_image(size, 1.0),
            &Transform::translation(DVec2::new(0.3, -0.2)),
            2.5,
            Some(&only(size, pixel)),
        );
        let product = acc.finalize().product;
        let total: f32 = weight_plane(&product).pixels().iter().sum();
        assert!(
            (total - 2.5).abs() <= 2.0 * MAX_DEPOSITS * f32::EPSILON * 2.5 * LOBE_EXCESS,
            "{kernel:?}: {total}"
        );
    }
}

/// A zero pixel weight keeps that pixel out of every kernel: a 100 under it never reaches the
/// image, which reads the surrounding 4 wherever enough weight landed. And the cells it would have
/// reached are each kernel's own footprint — which a frame with only that pixel weighted shows.
/// Shifted by (0.3, −0.2), pixel (5, 5) lands at output (11.1, 10.1) at scale 2: a 1.6-wide box
/// reaches cells 10–12 and 9–11, nine for Turbo and the square kernel; the point kernel takes one
/// cell; the Gaussian (σ = 1.6/2.3548, radius ⌈3σ⌉ = 3) all 49 of its neighbourhood. At scale 1 it
/// lands at (5.3, 4.8), and Lanczos-3, zero from distance 3 on, reaches six columns (2.3 to the
/// left, 2.7 to the right) and six rows: 36.
///
/// The gate is a quarter of the deepest weight: next to the excluded pixel a Lanczos window keeps
/// little more than its lobes.
#[test]
fn a_zero_pixel_weight_keeps_the_pixel_out() {
    const GATE: f32 = 0.25;

    let size = Size2us::new(12, 12);
    let pixel = Vec2us::new(5, 5);
    let transform = Transform::translation(DVec2::new(0.3, -0.2));
    let mut excluded = Buffer2::new_filled(size.width, size.height, 1.0);
    excluded[(pixel.x, pixel.y)] = 0.0;
    for (kernel, footprint) in [
        (DrizzleKernel::Turbo, 9),
        (DrizzleKernel::Square, 9),
        (DrizzleKernel::Point, 1),
        (DrizzleKernel::Gaussian, 49),
        (DrizzleKernel::Lanczos, 36),
    ] {
        let config = DrizzleConfig {
            fill_value: -1.0,
            min_weight_fraction: GATE,
            ..usual_config(kernel)
        };
        let product = drizzle_one(
            size,
            config.clone(),
            one_pixel_image(size, 4.0, pixel, 100.0),
            &transform,
            Some(&excluded),
        );
        assert_constant_or_fill(&product, 4.0, -1.0, GATE, &format!("{kernel:?}"));

        let alone = drizzle_one(
            size,
            config,
            constant_image(size, 4.0),
            &transform,
            Some(&only(size, pixel)),
        );
        let reached = weight_plane(&alone)
            .pixels()
            .iter()
            .filter(|&&w| w != 0.0)
            .count();
        assert_eq!(reached, footprint, "{kernel:?}");
    }
}

/// Per-pixel weights scale a pixel's share: two frames of 2 and 6, the first's pixel (1, 1) at
/// weight ½, combine there to `(2·½ + 6·1) / (½ + 1)` = 14/3 and elsewhere to 4 — at scale 1 and
/// pixfrac 1, where every drop is its own pixel. A few roundings of values near 6.
#[test]
fn pixel_weights_scale_their_pixel_share() {
    let size = Size2us::new(4, 4);
    let mut half = Buffer2::new_filled(4, 4, 1.0f32);
    half[(1, 1)] = 0.5;
    let mut acc = accumulator(
        ImageDimensions::new(size, 1),
        kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
    );
    acc.add_image(
        constant_image(size, 2.0),
        &Transform::identity(),
        1.0,
        Some(&half),
    );
    acc.add_image(constant_image(size, 6.0), &Transform::identity(), 1.0, None);
    let product = acc.finalize().product;
    let out = product.image.channel(0);
    assert!((out[(1, 1)] - 14.0 / 3.0).abs() <= 4.0 * f32::EPSILON * 6.0);
    assert_eq!(out[(0, 0)], 4.0);
}

/// A frame of weight zero deposits nothing and does not count as covering anything: one frame of 3
/// and one of 100 at weight 0 read 3, at coverage one frame in two.
#[test]
fn a_zero_weight_frame_is_ignored() {
    let size = Size2us::new(8, 8);
    let mut acc = accumulator(
        ImageDimensions::new(size, 1),
        usual_config(DrizzleKernel::Turbo),
    );
    acc.add_image(constant_image(size, 3.0), &Transform::identity(), 1.0, None);
    acc.add_image(
        constant_image(size, 100.0),
        &Transform::identity(),
        0.0,
        None,
    );
    let product = acc.finalize().product;
    assert!(product.image.channel(0).pixels().iter().all(|&v| v == 3.0));
    assert_eq!(product.coverage.as_ref().unwrap()[(8, 8)], 0.5);
}

/// A radial drop that hangs off the output grid loses the part that missed it.
///
/// The alternative — normalizing by the taps that survived clipping — would keep every input pixel
/// depositing its whole weight, so a border pixel would claim the same depth as an interior one
/// while only a fraction of its drop landed. The compact kernels drop the overhang, and the ratio
/// image is unaffected either way (flux and weight fall together), so the weight map is where the
/// two conventions differ.
#[test]
fn radial_drops_lose_the_flux_that_misses_the_grid() {
    const SIDE: i64 = 8;
    let size = Size2us::new(SIDE as usize, SIDE as usize);
    let config = kernel_config(DrizzleKernel::Gaussian, 1.0, 1.0);
    let product = drizzle_one(
        size,
        config,
        constant_image(size, 1.0),
        &Transform::identity(),
        None,
    );

    // At scale 1 / pixfrac 1 the drop size is 1 output pixel, so σ = 1/2.3548 and the kernel is
    // truncated at ceil(3σ) = 2. Every drop centre lands on an integer output pixel, so the tap
    // grid is separable and the deposited weight is a product of per-axis sums.
    let sigma = 1.0 / 2.3548;
    let tap = |d: i64| (-((d * d) as f64) / (2.0 * sigma * sigma)).exp();
    let full: f64 = (-2..=2).map(tap).sum();
    let on_grid: f64 = (0..SIDE)
        .map(|i| {
            (-2..=2)
                .filter(|d| (0..SIDE).contains(&(i + d)))
                .map(tap)
                .sum::<f64>()
                / full
        })
        .sum();
    // 8 input pixels per axis, of which the two outermost lose part of their kernel: the axis sums
    // to 7.8888 rather than 8, so the grid holds 62.23 of the 64 it would with renormalization.
    let expected = (on_grid * on_grid) as f32;

    // Each weight sums up to 25 deposits, and the plane's sum adds 64 of them: 89 roundings.
    let weight: f32 = weight_plane(&product).pixels().iter().sum();
    assert!(
        (weight - expected).abs() <= 89.0 * f32::EPSILON * expected,
        "Σ weight {weight} must be the on-grid share {expected}, not the 64 renormalization would keep"
    );
    // The image is untouched by the shortfall: flux and weight fell together.
    let out = product.image.channel(0);
    for (x, y) in [(0, 0), (4, 4)] {
        assert!(
            (out[(x, y)] - 1.0).abs() <= 51.0 * f32::EPSILON,
            "({x}, {y})"
        );
    }
}

/// Each channel of an RGB frame drizzles on its own: pixel (1, 1) of (1, 2, 3) fills its block of
/// cells with those three values, exactly — see `one_bright_pixel_fills_its_own_block`.
#[test]
fn rgb_channels_drizzle_independently() {
    let size = Size2us::new(4, 4);
    let plane = |value: f32| {
        let mut pixels = vec![0.0f32; 16];
        pixels[5] = value;
        pixels
    };
    let image = rgb_image(size, plane(1.0), plane(2.0), plane(3.0));
    let mut acc = accumulator(
        ImageDimensions::new(size, 3),
        kernel_config(DrizzleKernel::Turbo, 2.0, 0.8),
    );
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let product = acc.finalize().product;
    for (channel, expected) in [1.0, 2.0, 3.0].into_iter().enumerate() {
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            assert_eq!(
                product.image.channel(channel)[(x, y)],
                expected,
                "channel {channel} ({x}, {y})"
            );
        }
    }
}
