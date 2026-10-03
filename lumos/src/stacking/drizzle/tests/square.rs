use super::*;

/// Two input pixels of different values mixing in one output cell, through the square kernel's
/// clipped quadrilaterals.
///
/// scale=2, pixfrac=1.0, shifted a quarter of an input pixel along x, so each quad lands half an
/// output pixel off the grid. Pixel (0,0)=10 spans input x [−0.25, 0.75] → output [0, 2], and pixel
/// (1,0)=20 spans [0.75, 1.75] → [2, 4]; along y both cover output rows 0 and 1 whole. Both quads
/// have area (pixfrac·scale)² = 4, so each contributes its overlap over 4:
///   cell (1,0) = [0.5, 1.5]: only pixel (0,0), whole → 10.0
///   cell (2,0) = [1.5, 2.5]: half of each → equal-weight mean (10·0.125 + 20·0.125) / 0.25 = 15.0
///   cell (3,0) = [2.5, 3.5]: only pixel (1,0), whole → 20.0
#[test]
fn square_kernel_mixes_two_pixels_by_their_overlap() {
    let mut pixels = vec![0.0f32; 16];
    pixels[0] = 10.0;
    pixels[1] = 20.0;
    let product = drizzle_one(
        Size2us::new(4, 4),
        kernel_config(DrizzleKernel::Square, 2.0, 1.0),
        gray_image(Size2us::new(4, 4), pixels),
        &Transform::translation(DVec2::new(0.25, 0.0)),
        None,
    );
    // The overlaps are halves and wholes of quads whose area is a power of two, so every weight,
    // product and sum is exact and so is the mean.
    for (cell, expected) in [(1, 10.0), (2, 15.0), (3, 20.0)] {
        assert_eq!(
            product.image.channel(0)[(cell, 0)],
            expected,
            "cell ({cell},0)"
        );
    }
}

/// Square agrees with Turbo while the drop stays axis-aligned — image and weight both — and
/// diverges once it does not, which is the whole reason the polygon-overlap kernel exists.
///
/// A translation leaves every drop an axis-aligned rectangle, the only shape Turbo can represent,
/// so the two compute the same shares by different arithmetic: a pixel of at most four deposits of
/// values up to 9 differs by the two roundings of `2·4 + 1` operations each. A rotation makes the
/// drop a general quadrilateral, and they apportion flux differently — by far more than that.
#[test]
fn square_matches_turbo_only_while_the_drop_stays_axis_aligned() {
    let size = Size2us::new(10, 10);
    // A horizontal gradient: uniform data would agree under any kernel and prove nothing.
    let gradient: Vec<f32> = (0..100).map(|i| (i % 10) as f32).collect();
    let agreement = 2.0 * (2.0 * 4.0 + 1.0) * f32::EPSILON * 9.0;

    for (transform_name, transform, axis_aligned) in [
        (
            "translation",
            Transform::translation(DVec2::new(0.3, -0.2)),
            true,
        ),
        (
            "15 degree rotation",
            Transform::rotation_around(DVec2::splat(5.0), 15.0_f64.to_radians()),
            false,
        ),
    ] {
        for (scale, pixfrac) in [(1.0, 1.0), (2.0, 0.8)] {
            let case = format!("{transform_name} at scale {scale}, pixfrac {pixfrac}");
            let render = |kernel| {
                drizzle_one(
                    size,
                    kernel_config(kernel, scale, pixfrac),
                    gray_image(size, gradient.clone()),
                    &transform,
                    None,
                )
            };
            let turbo = render(DrizzleKernel::Turbo);
            let square = render(DrizzleKernel::Square);

            let mut max_diff = 0.0f32;
            let mut max_weight_diff = 0.0f32;
            for (((&t, &s), &tw), &sw) in turbo
                .image
                .channel(0)
                .iter()
                .zip(square.image.channel(0).iter())
                .zip(weight_plane(&turbo).iter())
                .zip(weight_plane(&square).iter())
            {
                if tw > 0.0 && sw > 0.0 {
                    max_diff = max_diff.max((t - s).abs());
                }
                max_weight_diff = max_weight_diff.max((tw - sw).abs());
            }

            if axis_aligned {
                assert!(
                    max_diff <= agreement && max_weight_diff <= agreement,
                    "{case}: kernels must agree on an axis-aligned drop, image {max_diff}, \
                     weight {max_weight_diff}"
                );
            } else {
                assert!(
                    max_diff > 100.0 * agreement,
                    "{case}: kernels must diverge on a rotated drop, max diff {max_diff}"
                );
            }
        }
    }
}

/// A rotated drop keeps its flux: every quadrilateral's shares sum to 1, so with every lit pixel's
/// quadrilateral on the grid the deposited flux is the input's. A 4×4 patch of 10 at (8..12)² in a
/// zero frame, turned 15° about (10, 10), lands well inside the 20×20 grid: Σ = 160. Each cell sums
/// at most nine deposits, a relative 2·9·ε, and the total is taken in f64.
///
/// The rotation fixes (10, 10), and the cell there reaches no further than 0.71 from it — inside the
/// quadrilaterals of the patch's pixels around it — so it reads 10.
#[test]
fn a_rotated_square_drop_keeps_its_flux() {
    let size = Size2us::new(20, 20);
    let mut pixels = vec![0.0f32; size.pixel_count()];
    for y in 8..12 {
        for x in 8..12 {
            pixels[y * 20 + x] = 10.0;
        }
    }
    let mut acc = accumulator(
        ImageDimensions::new(size, 1),
        kernel_config(DrizzleKernel::Square, 1.0, 1.0),
    );
    acc.add_image(
        gray_image(size, pixels),
        &Transform::rotation_around(DVec2::new(10.0, 10.0), 15.0_f64.to_radians()),
        1.0,
        None,
    );
    let flux = acc.accumulated_flux_sum(0);
    assert!(
        (flux - 160.0).abs() <= 18.0 * f64::from(f32::EPSILON) * 160.0,
        "Σ flux·w {flux}"
    );
    let centre = acc.finalize().product.image.channel(0)[(10, 10)];
    assert!(
        (centre - 10.0).abs() <= 19.0 * f32::EPSILON * 10.0,
        "{centre}"
    );
}
