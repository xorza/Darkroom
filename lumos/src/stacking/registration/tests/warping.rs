//! Warps of synthetic star fields: roundtrips through every `TransformType`, the quality order of
//! the kernels, SIP, and the quality maps at the border.

use crate::stacking::registration::config::{self, InterpolationMethod, WarpParams};
use crate::stacking::registration::resample::{self, internals};
use crate::stacking::registration::transform::{Transform, TransformType, WarpTransform};
use crate::testing::prelude::*;
use crate::testing::synthetic::fixtures::star_field;
use crate::testing::synthetic::metrics;

/// Helper to warp and return a new buffer (for test convenience).
/// Visually applies the transform to the image content (stars move by T).
/// Passes T⁻¹ to the plane warp since it uses output→input coordinate mapping.
fn do_warp(
    input: &Buffer2<f32>,
    transform: &Transform,
    method: InterpolationMethod,
) -> Buffer2<f32> {
    let inverse = transform.inverse();
    let mut output = Buffer2::new_default(input.width(), input.height());
    internals::warp_plane(
        input,
        &mut output,
        &WarpTransform::new(inverse),
        &config::internals::warp_params(method),
    );
    output
}

/// Peak signal-to-noise ratio between two images of peak `max_val`, in dB; infinite for equal ones.
fn compute_psnr(a: &[f32], b: &[f32], max_val: f32) -> f64 {
    20.0 * (f64::from(max_val) / metrics::rms_diff(a, b)).log10()
}

/// Compute normalized cross-correlation between two images.
/// Returns value in [-1, 1], where 1 means perfect correlation.
fn compute_ncc(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let mean_a: f64 = a.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
    let mean_b: f64 = b.iter().map(|&x| f64::from(x)).sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;

    for (&x, &y) in a.iter().zip(b.iter()) {
        let dx = f64::from(x) - mean_a;
        let dy = f64::from(y) - mean_b;
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }

    if var_a < 1e-10 || var_b < 1e-10 {
        return 0.0;
    }

    cov / (var_a.sqrt() * var_b.sqrt())
}

/// Per-method PSNR and NCC thresholds for roundtrip warp tests.
type MethodThresholds = &'static [(InterpolationMethod, f64, f64)];

/// Common helper for roundtrip warp tests.
///
/// Creates a star field, warps forward then inverse, and checks that the
/// central region matches the original within per-method thresholds.
fn assert_roundtrip(
    seed: u64,
    forward: Transform,
    label: &str,
    margin: usize,
    thresholds: MethodThresholds,
) {
    let ref_buf = star_field(Size2us::new(256, 256), 30, seed)
        .image
        .channel(0)
        .clone();
    let width = ref_buf.width();
    let height = ref_buf.height();
    let inverse = forward.inverse();

    for &(method, min_psnr, min_ncc) in thresholds {
        let warped = do_warp(&ref_buf, &forward, method);
        let restored = do_warp(&warped, &inverse, method);

        let CentralRegions {
            a: central_ref,
            b: central_restored,
        } = extract_central_region(
            ref_buf.pixels(),
            restored.pixels(),
            Size2us::new(width, height),
            margin,
        );

        let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
        let ncc = compute_ncc(&central_ref, &central_restored);

        assert!(
            psnr > min_psnr,
            "{label} {method:?}: PSNR {psnr} < {min_psnr} dB",
        );
        assert!(ncc > min_ncc, "{label} {method:?}: NCC {ncc} < {min_ncc}");
    }
}

#[test]
fn warp_translation_roundtrip() {
    assert_roundtrip(
        11111,
        Transform::translation(DVec2::new(10.5, -7.3)),
        "Translation",
        20,
        &[
            (InterpolationMethod::Nearest, 15.0, 0.25),
            (InterpolationMethod::Bilinear, 22.0, 0.70),
            (InterpolationMethod::Lanczos3, 25.0, 0.80),
        ],
    );
}

#[test]
fn warp_euclidean_roundtrip() {
    assert_roundtrip(
        22222,
        Transform::euclidean(DVec2::new(5.0, -3.0), 2.0_f64.to_radians()),
        "Euclidean",
        30,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.25),
            (InterpolationMethod::Bilinear, 25.0, 0.85),
            (InterpolationMethod::Lanczos3, 30.0, 0.90),
        ],
    );
}

#[test]
fn warp_similarity_roundtrip() {
    assert_roundtrip(
        33333,
        Transform::similarity(DVec2::new(8.0, -5.0), 1.5_f64.to_radians(), 1.02),
        "Similarity",
        40,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3, 28.0, 0.85),
        ],
    );
}

#[test]
fn warp_affine_roundtrip() {
    // Affine with slight differential scaling
    let angle_rad = 0.5_f64.to_radians();
    let (cos_a, sin_a) = (angle_rad.cos(), angle_rad.sin());
    let forward = Transform::affine([
        1.01 * cos_a,
        -0.99 * sin_a,
        6.0,
        1.01 * sin_a,
        0.99 * cos_a,
        -4.0,
    ]);
    assert_eq!(forward.transform_type(), TransformType::Affine);

    assert_roundtrip(
        44444,
        forward,
        "Affine",
        40,
        &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3, 26.0, 0.85),
        ],
    );
}

#[test]
fn warp_homography_roundtrip() {
    // Mild perspective distortion
    let forward = Transform::homography([1.0, 0.0, 5.0, 0.0, 1.0, -3.0, 0.00005, 0.00003]);
    assert_eq!(forward.transform_type(), TransformType::Homography);

    assert_roundtrip(
        55555,
        forward,
        "Homography",
        50,
        &[
            (InterpolationMethod::Nearest, 10.0, 0.80),
            (InterpolationMethod::Bilinear, 20.0, 0.80),
            (InterpolationMethod::Lanczos3, 24.0, 0.80),
        ],
    );
}

/// A wider kernel restores a sub-pixel roundtrip better: bilinear below Catmull-Rom, and each
/// Lanczos order below the next. Catmull-Rom and Lanczos-2 share a 4-tap window and land within a
/// fraction of a dB of each other, so they are not ordered against each other.
#[test]
fn interpolation_quality_ordering() {
    let ref_buf = star_field(Size2us::new(256, 256), 30, 77777)
        .image
        .channel(0)
        .clone();
    let size = Size2us::new(ref_buf.width(), ref_buf.height());
    let forward = Transform::similarity(DVec2::new(3.7, -2.3), 1.0_f64.to_radians(), 1.01);
    let inverse = forward.inverse();
    let psnr = |method| {
        let restored = do_warp(&do_warp(&ref_buf, &forward, method), &inverse, method);
        let central = extract_central_region(ref_buf.pixels(), restored.pixels(), size, 50);
        compute_psnr(&central.a, &central.b, 1.0)
    };
    for pair in [
        [InterpolationMethod::Bilinear, InterpolationMethod::Bicubic],
        [InterpolationMethod::Lanczos2, InterpolationMethod::Lanczos3],
        [InterpolationMethod::Lanczos3, InterpolationMethod::Lanczos4],
    ] {
        let [narrow, wide] = pair.map(psnr);
        assert!(
            narrow < wide,
            "{:?} at {narrow:.2} dB is not below {:?} at {wide:.2} dB",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn warp_preserves_output_metadata() {
    use crate::io::image::image_metadata::ImageMetadata;

    let pixels = star_field(Size2us::new(256, 256), 30, 11111)
        .image
        .channel(0)
        .clone();
    let width = pixels.width();
    let height = pixels.height();
    let mut image =
        LinearImage::from_pixels(ImageDimensions::new((width, height), 1), pixels.into_vec());
    image.metadata = ImageMetadata {
        object: Some("M42".to_string()),
        exposure_time: Some(120.0),
        ..Default::default()
    };

    let transform = Transform::translation(DVec2::new(5.0, 5.0));
    let warp_config = WarpParams {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    let warped = resample::warp(&image, &WarpTransform::new(transform), &warp_config).image;

    // Verify the input's metadata is carried into the warped output (warp only
    // produces new pixel data).
    assert_eq!(warped.metadata.object, Some("M42".to_string()));
    assert_eq!(warped.metadata.exposure_time, Some(120.0));
}

/// The central regions of two images, row by row.
#[derive(Debug)]
struct CentralRegions {
    a: Vec<f32>,
    b: Vec<f32>,
}

/// Extract central region of two images for comparison (avoids border artifacts).
fn extract_central_region(a: &[f32], b: &[f32], size: Size2us, margin: usize) -> CentralRegions {
    let inner_width = size.width - 2 * margin;
    let inner_height = size.height - 2 * margin;

    let mut central_a = Vec::with_capacity(inner_width * inner_height);
    let mut central_b = Vec::with_capacity(inner_width * inner_height);

    for y in margin..(size.height - margin) {
        for x in margin..(size.width - margin) {
            let idx = size.index_of(Vec2us::new(x, y));
            central_a.push(a[idx]);
            central_b.push(b[idx]);
        }
    }

    CentralRegions {
        a: central_a,
        b: central_b,
    }
}

/// The public `warp` samples through the SIP correction: it is the plane warp of the same
/// `WarpTransform` bit for bit — whose positions are `T(p + c(p))` (see `RowPositions`' and
/// `WarpTransform`'s tests) — for every method, on a field whose correction reaches past 0.1 px.
#[test]
fn the_public_warp_samples_through_the_sip_correction() {
    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
    use crate::testing::synthetic::distortion::{RadialField, RadialPairs};

    let size = Size2us::new(128, 128);
    let transform = Transform::translation(DVec2::new(5.0, -3.0));
    let field = RadialField {
        transform,
        start: 10,
        step: 11,
        extent: 109,
        ..RadialField::new(DVec2::new(64.0, 64.0), 3e-6)
    };
    let RadialPairs {
        reference: ref_points,
        target: target_points,
    } = field.pairs();
    let sip_config = SipConfig {
        order: 3,
        reference_point: Some(field.centre),
        ..Default::default()
    };
    let sip =
        SipPolynomial::fit_from_transform(&ref_points, &target_points, &transform, &sip_config)
            .unwrap()
            .polynomial;
    assert!(sip.max_grid_correction(size, 10.0) > 0.1);
    let warp_transform = WarpTransform::with_sip(transform, sip);

    let pixels = star_field(size, 20, 12321).image.channel(0).clone();
    let image = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 1),
        pixels.pixels().to_vec(),
    );
    for method in InterpolationMethod::ALL {
        let params = config::internals::warp_params(method);
        let warped = resample::warp(&image, &warp_transform, &params).image;
        let mut plane = Buffer2::new_default(size.width, size.height);
        internals::warp_plane(&pixels, &mut plane, &warp_transform, &params);
        for (index, (a, e)) in warped.channel(0).iter().zip(plane.iter()).enumerate() {
            assert_eq!(
                a.to_bits(),
                e.to_bits(),
                "{method:?} pixel {index}: {a} against {e}"
            );
        }
    }
}

/// `warp` emits independent geometric support and interpolation-confidence maps and renormalizes
/// partially-covered bilinear border pixels back to the in-bounds average.
#[test]
fn warp_emits_coverage_and_renormalizes_bilinear_border() {
    // Constant image so any darkening is unambiguous: a covered output pixel
    // must read back exactly V.
    const V: f32 = 0.5;
    let size = Size2us::new(16usize, 8usize);
    let image = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 1),
        vec![V; size.pixel_count()],
    );

    // output(x,y) samples source (x + 2.5, y): columns 0..=12 are fully in
    // bounds, column 13 is half-covered (its right bilinear tap is off the
    // edge), columns 14..=15 fall entirely outside.
    let transform = Transform::translation(DVec2::new(2.5, 0.0));
    let config = WarpParams {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    assert_eq!(config.border_value, 0.0, "test assumes a zero border");

    let result = resample::warp(&image, &WarpTransform::new(transform), &config);
    let cov = result.coverage.pixels();
    let confidence = result.confidence.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| size.index_of(Vec2us::new(x, y));

    // Every row shares the column pattern. Two equal x taps have confidence 1²/(0.5² + 0.5²) = 2
    // and one surviving tap 0.5²/0.5² = 1; a covered pixel — the half-covered column 13 too — is
    // renormalized back to V, not darkened to 0.5·V. All dyadic, so exact.
    for y in 0..size.height {
        for x in 0..size.width {
            let expected = match x {
                0..=12 => (1.0, 2.0, V),
                13 => (0.5, 1.0, V),
                _ => (0.0, 0.0, 0.0),
            };
            assert_eq!(
                (cov[at(x, y)], confidence[at(x, y)], val[at(x, y)]),
                expected,
                "({x}, {y}): coverage, confidence, value"
            );
        }
    }
}

/// The default negative-lobe kernel (Lanczos3) emits magnitude-based coverage and independent
/// interpolation confidence while preserving a flat field through its edge fallback.
#[test]
fn warp_renormalizes_lanczos_edges_and_emits_coverage() {
    const V: f32 = 0.5;
    let size = Size2us::new(32usize, 8usize);
    let image = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 1),
        vec![V; size.pixel_count()],
    );

    // src = (x + 3.5, y): a Lanczos3 (6-tap) kernel reaches off the right edge
    // for x ≳ 26 and lands entirely outside by x = 31.
    let transform = Transform::translation(DVec2::new(3.5, 0.0));
    let config = WarpParams {
        method: InterpolationMethod::Lanczos3,
        ..Default::default()
    };

    let result = resample::warp(&image, &WarpTransform::new(transform), &config);
    let cov = result.coverage.pixels();
    let confidence = result.confidence.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| size.index_of(Vec2us::new(x, y));
    let y = 4;

    // Interior: every kernel tap is in bounds, so the magnitude support fraction is 1 exactly.
    assert_eq!(cov[at(10, y)], 1.0);
    // Far past the edge: every tap is outside.
    assert_eq!(cov[at(31, y)], 0.0, "column 31 is fully extrapolated");
    // A fractional border band exists between the two.
    let partial = (24..32)
        .filter(|&x| cov[at(x, y)] > 0.0 && cov[at(x, y)] < 1.0)
        .count();
    assert!(
        partial >= 2,
        "expected a fractional coverage band, got {partial} columns"
    );

    // Fully supported Lanczos and its edge-extended bilinear fallback both preserve a flat field,
    // to the rounding of 36 products, their sum and the normalization: 39·ε·V.
    let tolerance = 39.0 * f32::EPSILON * V;
    assert!(
        (val[at(10, y)] - V).abs() <= tolerance,
        "interior value should be V, got {}",
        val[at(10, y)]
    );
    let edge_x = (24..31)
        .find(|&x| cov[at(x, y)] > 0.05 && cov[at(x, y)] < 0.95)
        .expect("a partially-covered edge column");
    assert!(
        (val[at(edge_x, y)] - V).abs() <= tolerance,
        "renormalized Lanczos edge value should recover V, got {} at col {edge_x} (cov {})",
        val[at(edge_x, y)],
        cov[at(edge_x, y)]
    );
    assert!(
        confidence[at(10, y)].is_finite() && confidence[at(10, y)] > 0.0,
        "interior confidence should be finite and positive"
    );
    assert!(
        confidence[at(edge_x, y)].is_finite() && confidence[at(edge_x, y)] > 0.0,
        "partial-kernel confidence should follow the finite bilinear fallback"
    );

    for (&c, &q) in cov.iter().zip(confidence.iter()) {
        assert!((0.0..=1.0).contains(&c), "coverage {c} out of range");
        assert!(q.is_finite() && q >= 0.0, "invalid confidence {q}");
    }
}
