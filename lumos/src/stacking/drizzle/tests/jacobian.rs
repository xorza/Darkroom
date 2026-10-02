use super::*;

// Strategy: Two frames contribute to the same output pixel with different
// local Jacobians. Frame A uses identity (jaco=1), Frame B uses scale-2x
// transform (jaco=4). Both have frame_weight=1.
//
// For Point kernel at scale=1:
//   Frame A pixel (5,5) → center (5.5, 5.5) → output (5,5), jaco_A = 1.
//   Frame B pixel (2,2) → center (2.5, 2.5) → transformed (5.0, 5.0) → output (5,5), jaco_B = 4.
//   Weight_A = 1/jaco_A = 1.0
//   Weight_B = 1/jaco_B = 0.25
//   output = (val_A * 1.0 + val_B * 0.25) / (1.0 + 0.25) = (val_A + 0.25 * val_B) / 1.25
//
// With val_A=10, val_B=0: output = 10/1.25 = 8.0  (with Jacobian)
//   vs output = (10+0)/2 = 5.0 if both had equal weight (no Jacobian).

/// Build a scale-2x transform: maps (x,y) → (2x, 2y).
fn make_scale2x_transform() -> Transform {
    Transform::scale(DVec2::splat(2.0))
}

#[test]
fn point_jacobian_two_frame_weighted_mean() {
    // Frame A: identity, val=10. Pixel(5,5) → output(5,5), jaco=1, w=1.
    // Frame B: scale-2x, val=0. Pixel(2,2) → (5,5) → output(5,5), jaco=4, w=0.25.
    // Expected output(5,5) = (10 * 1 + 0 * 0.25) / (1 + 0.25) = 8.0
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[4 * w + 4] = 10.0;
    let image_a = mono_image(Size2us::new(w, h), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 0.0; // explicitly zero
    let image_b = mono_image(Size2us::new(w, h), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Point,
        ..Default::default()
    };

    let mut acc = accumulator(ImageDimensions::new((w, h), 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // output(5,5) = (10*1 + 0*0.25) / (1+0.25) = 8.0
    let out = result.image.channel(0);
    let val = out[4 * w + 4];
    assert!(
        (val - 8.0).abs() < 0.01,
        "Point Jacobian: pixel (5,5) = {val}, expected 8.0"
    );
}

#[test]
fn point_jacobian_two_frame_both_nonzero() {
    // Frame A: identity, val=10. Pixel(5,5) → output(5,5), jaco=1, w=1.
    // Frame B: scale-2x, val=2. Pixel(2,2) → output(5,5), jaco=4, w=0.25.
    // Expected = (10*1 + 2*0.25) / (1+0.25) = 10.5/1.25 = 8.4
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[4 * w + 4] = 10.0;
    let image_a = mono_image(Size2us::new(w, h), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 2.0;
    let image_b = mono_image(Size2us::new(w, h), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Point,
        ..Default::default()
    };

    let mut acc = accumulator(ImageDimensions::new((w, h), 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // output(5,5) = (10*1 + 2*0.25) / (1+0.25) = 10.5/1.25 = 8.4
    let out = result.image.channel(0);
    let val = out[4 * w + 4];
    assert!(
        (val - 8.4).abs() < 0.01,
        "Point Jacobian: pixel (5,5) = {val}, expected 8.4"
    );
}

#[test]
fn turbo_jacobian_two_frame_weighted_mean() {
    // Turbo kernel, pixfrac=1, scale=1. Drop size = 1×1.
    // Frame A: identity, val=10. Pixel(5,5) center→(5.5,5.5). Drop covers output(5,5) fully.
    //   overlap=1.0, inv_area=1.0, jaco=1. Weight = 1*1*1/1 = 1.0
    // Frame B: scale-2x, val=2. Pixel(2,2) center→(5.0,5.0). Drop [4.5,5.5]×[4.5,5.5].
    //   overlap with output(5,5)=[5,6]: min(5.5,6)-max(4.5,5)=0.5 in x, same in y → 0.25.
    //   inv_area=1.0, jaco=4. Weight = 1*0.25*1/4 = 0.0625
    // output(5,5) = (10*1.0 + 2*0.0625) / (1.0+0.0625) = 10.125/1.0625 = 9.5294...
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[4 * w + 4] = 10.0;
    let image_a = mono_image(Size2us::new(w, h), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 2.0;
    let image_b = mono_image(Size2us::new(w, h), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        ..Default::default()
    };

    let mut acc = accumulator(ImageDimensions::new((w, h), 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // Integer-center: Frame B drop (centered (4,4), drop_size 1) fully covers output (4,4),
    // so overlap = 1.0 (not the old 0.25). data = 10*1.0 + 2*0.25 = 10.5,
    // weight = 1.0 + 0.25 = 1.25, output = 10.5/1.25 = 8.4.
    let expected = 10.5 / 1.25;
    let out = result.image.channel(0);
    let val = out[4 * w + 4];
    assert!(
        (val - expected as f32).abs() < 0.02,
        "Turbo Jacobian: pixel (4,4) = {val}, expected {expected:.4}"
    );
}

#[test]
fn turbo_matches_square_affine_with_jacobian() {
    // For affine transforms (constant Jacobian), Turbo with Jacobian and Square
    // should produce identical output on a gradient image (non-trivial content).
    // Using a translation of (0.3, 0.7) — axis-aligned, so Turbo drop = true quad.
    let w = 16;
    let h = 16;
    // Gradient: val = x + y*0.5
    let pixels: Vec<f32> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            x + y * 0.5
        })
        .collect();

    let transform = Transform::translation(DVec2::new(0.3, 0.7));

    let config_turbo = DrizzleConfig {
        scale: 1.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Turbo,
        ..Default::default()
    };
    let config_square = DrizzleConfig {
        scale: 1.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Square,
        ..Default::default()
    };

    let image_turbo = mono_image(Size2us::new(w, h), pixels.clone());
    let image_square = mono_image(Size2us::new(w, h), pixels);

    let mut acc_turbo = accumulator(ImageDimensions::new((w, h), 1), config_turbo);
    acc_turbo.add_image(image_turbo, &transform, 1.0, None);
    let result_turbo = acc_turbo.finalize();

    let mut acc_square = accumulator(ImageDimensions::new((w, h), 1), config_square);
    acc_square.add_image(image_square, &transform, 1.0, None);
    let result_square = acc_square.finalize();

    let out_turbo = result_turbo.image.channel(0);
    let out_square = result_square.image.channel(0);
    let ow = result_turbo.image.width();

    // Check interior pixels where both kernels have full coverage
    let mut max_diff = 0.0f32;
    let mut checked = 0;
    for y in 2..14 {
        for x in 2..14 {
            let vt = out_turbo[y * ow + x];
            let vs = out_square[y * ow + x];
            if vt > 0.0 && vs > 0.0 {
                let diff = (vt - vs).abs();
                max_diff = max_diff.max(diff);
                checked += 1;
            }
        }
    }
    assert!(
        checked > 50,
        "Need sufficient interior pixels, got {checked}"
    );
    assert!(
        max_diff < 1e-3,
        "Turbo vs Square (affine translation): max_diff={max_diff:.6}, expected <1e-3"
    );
}

#[test]
fn gaussian_jacobian_two_frame_weighted_mean() {
    // Two uniform frames combined at output (4,4): A=10 (identity), B=2. Frame B's transform
    // sets its local Jacobian, which must down-weight a magnified frame. Isolate that effect
    // by combining B two ways — scale-2x (jaco=4, magnified → down-weighted) vs identity
    // (jaco=1, full weight). The Jacobian pulls the magnified-B result toward A's 10, so it
    // must read higher than the identity-B run.
    let w = 12;
    let h = 12;
    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Gaussian,
        ..Default::default()
    };
    let combine = |b_transform: &Transform| -> f32 {
        let image_a = constant_mono_image(Size2us::new(w, h), 10.0);
        let image_b = constant_mono_image(Size2us::new(w, h), 2.0);
        let mut acc = accumulator(ImageDimensions::new((w, h), 1), config.clone());
        acc.add_image(image_a, &Transform::identity(), 1.0, None);
        acc.add_image(image_b, b_transform, 1.0, None);
        let r = acc.finalize();
        r.image.channel(0)[4 * w + 4]
    };

    let magnified_b = combine(&make_scale2x_transform());
    let identity_b = combine(&Transform::identity());

    // Identity B (equal density, equal Jacobian) → simple mean (10 + 2) / 2 = 6.0.
    assert!(
        (identity_b - 6.0).abs() < 0.1,
        "identity B → simple mean ~6.0: {identity_b}"
    );
    // The Jacobian down-weights the magnified frame B, pulling the result toward A's 10.
    assert!(
        (2.0..=10.0).contains(&magnified_b),
        "weighted mean in [2,10]: {magnified_b}"
    );
    assert!(
        magnified_b > identity_b + 1.0,
        "Jacobian must down-weight magnified B: {magnified_b} vs {identity_b}"
    );
}

#[test]
fn lanczos_jacobian_two_frame_weighted_mean() {
    // Same setup as the Gaussian test, Lanczos-3 kernel: combine uniform B=2 two ways and
    // confirm the Jacobian down-weights the magnified (scale-2x) frame relative to identity.
    let w = 12;
    let h = 12;
    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        ..Default::default()
    };
    let combine = |b_transform: &Transform| -> f32 {
        let image_a = constant_mono_image(Size2us::new(w, h), 10.0);
        let image_b = constant_mono_image(Size2us::new(w, h), 2.0);
        let mut acc = accumulator(ImageDimensions::new((w, h), 1), config.clone());
        acc.add_image(image_a, &Transform::identity(), 1.0, None);
        acc.add_image(image_b, b_transform, 1.0, None);
        let r = acc.finalize();
        r.image.channel(0)[4 * w + 4]
    };

    let magnified_b = combine(&make_scale2x_transform());
    let identity_b = combine(&Transform::identity());

    // Identity B (equal density, equal Jacobian) → simple mean (10 + 2) / 2 = 6.0.
    assert!(
        (identity_b - 6.0).abs() < 0.1,
        "identity B → simple mean ~6.0: {identity_b}"
    );
    assert!(
        (2.0..=10.0).contains(&magnified_b),
        "weighted mean in [2,10]: {magnified_b}"
    );
    assert!(
        magnified_b > identity_b + 1.0,
        "Jacobian must down-weight magnified B: {magnified_b} vs {identity_b}"
    );
}

#[test]
fn all_kernels_jacobian_matches_square_affine() {
    // For a pure translation (affine, constant Jacobian=1), all kernels with
    // pixfrac=1, scale=1 should produce similar output to Square on a gradient
    // image. This tests that Jacobian correction doesn't break normal operation.
    let w = 20;
    let h = 20;
    let pixels: Vec<f32> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            1.0 + x * 0.1 + y * 0.05
        })
        .collect();

    // Small sub-pixel shift
    let transform = Transform::translation(DVec2::new(0.25, 0.15));

    // Square kernel as reference
    let config_sq = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        ..Default::default()
    };
    let image_sq = mono_image(Size2us::new(w, h), pixels.clone());
    let mut acc_sq = accumulator(ImageDimensions::new((w, h), 1), config_sq);
    acc_sq.add_image(image_sq, &transform, 1.0, None);
    let result_sq = acc_sq.finalize();
    assert_product_finite(&result_sq);
    let out_sq = result_sq.image.channel(0);

    for kernel in [
        DrizzleKernel::Turbo,
        DrizzleKernel::Point,
        DrizzleKernel::Gaussian,
        DrizzleKernel::Lanczos,
    ] {
        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel,
            ..Default::default()
        };
        let image = mono_image(Size2us::new(w, h), pixels.clone());
        let mut acc = accumulator(ImageDimensions::new((w, h), 1), config);
        acc.add_image(image, &transform, 1.0, None);
        let result = acc.finalize();
        assert_product_finite(&result);
        let out = result.image.channel(0);
        let ow = result.image.width();

        // Check interior pixels, allowing different interpolation effects per kernel
        let mut max_diff = 0.0f32;
        let mut checked = 0;
        for y in 5..15 {
            for x in 5..15 {
                let idx = y * ow + x;
                let vs = out_sq[idx];
                let vk = out[idx];
                if vs > 0.0 && vk > 0.0 {
                    let diff = (vs - vk).abs();
                    max_diff = max_diff.max(diff);
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 50,
            "{kernel:?}: not enough covered pixels ({checked})"
        );
        // Different kernels interpolate differently, but on a smooth gradient with
        // sub-pixel shift, they should all be within ~0.15 of Square kernel.
        // The important thing is Jacobian doesn't introduce large systematic errors.
        assert!(
            max_diff < 0.2,
            "{kernel:?} vs Square: max_diff={max_diff:.4}, expected < 0.2"
        );
    }
}

#[test]
fn local_jacobian_identity_scale1() {
    // Identity transform, scale=1: one input pixel maps to exactly one output pixel.
    // Jacobian = |det(I)| * 1² = 1.0
    let transform = Transform::identity();
    let center = transform.apply(DVec2::new(5.0, 5.0));
    let jaco = local_jacobian(&transform, center, Vec2us::new(5, 5));
    assert!(
        (jaco - 1.0).abs() < 1e-10,
        "Identity scale=1: expected 1.0, got {jaco}"
    );
}

#[test]
fn local_jacobian_identity_scale2() {
    // Identity transform, scale=2: one input pixel maps to a 2×2 area in output.
    // Jacobian = |det(I)| * 2² = 4.0
    let transform = output_transform(Transform::identity(), 2.0);
    let center = transform.apply(DVec2::new(5.0, 5.0));
    let jaco = local_jacobian(&transform, center, Vec2us::new(5, 5));
    assert!(
        (jaco - 4.0).abs() < 1e-10,
        "Identity scale=2: expected 4.0, got {jaco}"
    );
}

#[test]
fn local_jacobian_rotation_preserves_area() {
    // Pure rotation around origin: area is preserved, Jacobian = scale².
    // Rotation by 30° around (0, 0), scale=1.
    let angle = 30.0_f64.to_radians();
    let transform = Transform::euclidean(DVec2::ZERO, angle);
    let center = transform.apply(DVec2::new(50.0, 50.0));
    let jaco = local_jacobian(&transform, center, Vec2us::new(50, 50));
    // Rotation preserves area: Jacobian = 1.0
    assert!(
        (jaco - 1.0).abs() < 1e-10,
        "30° rotation: expected 1.0, got {jaco}"
    );
}

#[test]
fn local_jacobian_anisotropic_scale() {
    // Scale by 2x in x, 3x in y: area magnification = 6.
    let transform = Transform::scale(DVec2::new(2.0, 3.0));
    let center = transform.apply(DVec2::new(5.0, 5.0));
    // Jacobian = |det([2,0;0,3])| * scale² = 6 * 1 = 6.0
    let jaco = local_jacobian(&transform, center, Vec2us::new(5, 5));
    assert!(
        (jaco - 6.0).abs() < 1e-10,
        "2x×3x scale: expected 6.0, got {jaco}"
    );
}

#[test]
fn local_jacobian_perspective_varies_spatially() {
    // Perspective transform: Jacobian should differ at different image locations.
    // Homography with small perspective terms.
    let transform = Transform::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1e-4, 0.0]);

    // At x=0: w ≈ 1, minimal distortion
    let c0 = transform.apply(DVec2::new(0.0, 0.0));
    let jaco_left = local_jacobian(&transform, c0, Vec2us::ZERO);

    // At x=1000: w ≈ 1.1, noticeable distortion
    let c1000 = transform.apply(DVec2::new(1000.0, 0.0));
    let jaco_right = local_jacobian(&transform, c1000, Vec2us::new(1000, 0));

    // Jacobians must differ for perspective transform
    assert!(
        (jaco_left - jaco_right).abs() > 0.01,
        "Perspective Jacobian should vary: left={jaco_left:.6}, right={jaco_right:.6}"
    );
    // At x=0, w≈1 so jaco≈1. At x=1000, w≈1.1 so area shrinks → jaco < 1.
    assert!(
        jaco_left > jaco_right,
        "Left side should have larger Jacobian than right: {jaco_left:.6} vs {jaco_right:.6}"
    );
}

/// The per-frame factor and the per-pixel finite difference measure the same quantity for every
/// linear model — that agreement is what lets the kernels resolve it once instead of at every input
/// pixel. Checked far from the origin, where the finite difference subtracts two large coordinates
/// and the matrix form does not.
#[test]
fn area_magnification_agrees_with_the_per_pixel_jacobian_for_linear_models() {
    let cases = [
        (
            "translation",
            Transform::translation(DVec2::new(17.0, -9.0)),
        ),
        (
            "euclidean",
            Transform::euclidean(DVec2::new(3.0, 4.0), 0.21),
        ),
        (
            "similarity",
            Transform::similarity(DVec2::new(3.0, 4.0), 0.21, 1.37),
        ),
        (
            "affine",
            Transform::affine([1.02, 0.03, 12.0, -0.04, 0.98, -7.0]),
        ),
    ];

    for (name, transform) in cases {
        let to_output = output_transform(transform, 2.0);
        let magnification = AreaMagnification::new(&to_output);
        for pixel in [Vec2us::ZERO, Vec2us::new(7, 3), Vec2us::new(6000, 4000)] {
            let center = to_output.apply(DVec2::new(pixel.x as f64, pixel.y as f64));
            let uniform = magnification.at(center, pixel);
            let per_pixel = local_jacobian(&to_output, center, pixel);
            assert!(
                (uniform - per_pixel).abs() < 1e-9 * per_pixel,
                "{name} at {pixel:?}: uniform {uniform}, per-pixel {per_pixel}"
            );
        }
    }

    // Hand-computed: det([1.02, 0.03; -0.04, 0.98]) = 1.02·0.98 − 0.03·(−0.04) = 1.0008, and the
    // factor is that times scale².
    let affine = Transform::affine([1.02, 0.03, 12.0, -0.04, 0.98, -7.0]);
    let at_origin = |scale: f32| {
        let to_output = output_transform(affine, scale);
        AreaMagnification::new(&to_output).at(to_output.apply(DVec2::ZERO), Vec2us::ZERO)
    };
    assert!((at_origin(2.0) - 1.0008 * 4.0).abs() < 1e-12);
    // Scale is not ignored: composed into the matrix, it is quadratic in the determinant.
    assert!((at_origin(1.0) - 1.0008).abs() < 1e-12);
}

/// A projective transform keeps the per-pixel path, because its factor genuinely varies.
#[test]
fn area_magnification_stays_per_pixel_for_a_homography() {
    let transform = output_transform(
        Transform::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1e-4, 0.0]),
        1.0,
    );
    let magnification = AreaMagnification::new(&transform);

    let near = Vec2us::ZERO;
    let far = Vec2us::new(1000, 0);
    let at = |pixel: Vec2us| {
        let center = transform.apply(DVec2::new(pixel.x as f64, pixel.y as f64));
        magnification.at(center, pixel)
    };

    assert!(
        (at(near) - at(far)).abs() > 0.01,
        "a homography's magnification must vary: {} vs {}",
        at(near),
        at(far)
    );
}
