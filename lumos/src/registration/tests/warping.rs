//! Warps of synthetic star fields: roundtrips through every `TransformType`, the quality order of
//! the kernels, SIP, and the quality maps at the border.

use common::Buffer2;
use glam::DVec2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::linear_image::LinearImage;
use crate::internals::synthetic::fixtures::star_field;
use crate::internals::synthetic::metrics;
use crate::registration::resample::warp_config::{InterpolationMethod, WarpConfig};
use crate::registration::transform::{Transform, TransformType};
use crate::registration::warp_transform::WarpTransform;
use common::{Size2us, Vec2us};

/// Helper to warp and return a new buffer (for test convenience).
/// Visually applies the transform to the image content (stars move by T).
/// Passes T⁻¹ to the plane warp since it uses output→input coordinate mapping.
fn do_warp(
    input: &Buffer2<f32>,
    transform: &Transform,
    method: InterpolationMethod,
) -> Buffer2<f32> {
    warp_with(input, transform, WarpConfig::with_method(method))
}

fn warp_with(input: &Buffer2<f32>, transform: &Transform, params: WarpConfig) -> Buffer2<f32> {
    params.warp_plane(input, &WarpTransform::new(transform.inverse()))
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

/// A roundtrip warp test: a star field of `seed` warped by `forward` then by its inverse, whose
/// central region, `margin` pixels in from each edge, matches the original within per-method
/// thresholds.
#[derive(Debug, Clone, Copy)]
struct Roundtrip {
    seed: u64,
    forward: Transform,
    margin: usize,
    thresholds: MethodThresholds,
}

impl Roundtrip {
    fn check(self) {
        let ref_buf = star_field(Size2us::new(256, 256), 30, self.seed)
            .image
            .channel(0)
            .clone();
        let inverse = self.forward.inverse();
        let model = self.forward.transform_type();

        for &(method, min_psnr, min_ncc) in self.thresholds {
            let warped = do_warp(&ref_buf, &self.forward, method);
            let restored = do_warp(&warped, &inverse, method);

            let CentralRegions {
                a: central_ref,
                b: central_restored,
            } = extract_central_region(&ref_buf, &restored, self.margin);

            let psnr = compute_psnr(&central_ref, &central_restored, 1.0);
            let ncc = compute_ncc(&central_ref, &central_restored);

            assert!(
                psnr > min_psnr,
                "{model:?} {method:?}: PSNR {psnr} < {min_psnr} dB",
            );
            assert!(ncc > min_ncc, "{model:?} {method:?}: NCC {ncc} < {min_ncc}");
        }
    }
}

#[test]
fn warp_translation_roundtrip() {
    Roundtrip {
        seed: 11111,
        forward: Transform::translation(DVec2::new(10.5, -7.3)),
        margin: 20,
        thresholds: &[
            (InterpolationMethod::Nearest, 15.0, 0.25),
            (InterpolationMethod::Bilinear, 22.0, 0.70),
            (InterpolationMethod::Lanczos3, 25.0, 0.80),
        ],
    }
    .check();
}

#[test]
fn warp_euclidean_roundtrip() {
    Roundtrip {
        seed: 22222,
        forward: Transform::euclidean(DVec2::new(5.0, -3.0), 2.0_f64.to_radians()),
        margin: 30,
        thresholds: &[
            (InterpolationMethod::Nearest, 12.0, 0.25),
            (InterpolationMethod::Bilinear, 25.0, 0.85),
            (InterpolationMethod::Lanczos3, 30.0, 0.90),
        ],
    }
    .check();
}

#[test]
fn warp_similarity_roundtrip() {
    Roundtrip {
        seed: 33333,
        forward: Transform::similarity(DVec2::new(8.0, -5.0), 1.5_f64.to_radians(), 1.02),
        margin: 40,
        thresholds: &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3, 28.0, 0.85),
        ],
    }
    .check();
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

    Roundtrip {
        seed: 44444,
        forward,
        margin: 40,
        thresholds: &[
            (InterpolationMethod::Nearest, 12.0, 0.85),
            (InterpolationMethod::Bilinear, 22.0, 0.85),
            (InterpolationMethod::Lanczos3, 26.0, 0.85),
        ],
    }
    .check();
}

#[test]
fn warp_homography_roundtrip() {
    // Mild perspective distortion
    let forward = Transform::homography([1.0, 0.0, 5.0, 0.0, 1.0, -3.0, 0.00005, 0.00003]);
    assert_eq!(forward.transform_type(), TransformType::Homography);

    Roundtrip {
        seed: 55555,
        forward,
        margin: 50,
        thresholds: &[
            (InterpolationMethod::Nearest, 10.0, 0.80),
            (InterpolationMethod::Bilinear, 20.0, 0.80),
            (InterpolationMethod::Lanczos3, 24.0, 0.80),
        ],
    }
    .check();
}

/// A wider kernel restores a sub-pixel roundtrip better: bilinear below Catmull-Rom, and each
/// Lanczos order below the next. Catmull-Rom and Lanczos-2 share a 4-tap window and land within a
/// fraction of a dB of each other, so they are not ordered against each other. The kernels are
/// compared as linear filters, without the ringing clamp: beside a star it cuts the negative
/// lobes, and the most from the widest kernel, which has the most to cut.
#[test]
fn interpolation_quality_ordering() {
    let ref_buf = star_field(Size2us::new(256, 256), 30, 77777)
        .image
        .channel(0)
        .clone();
    let forward = Transform::similarity(DVec2::new(3.7, -2.3), 1.0_f64.to_radians(), 1.01);
    let inverse = forward.inverse();
    let psnr = |method| {
        let params = WarpConfig {
            clamping_threshold: None,
            ..WarpConfig::with_method(method)
        };
        let restored = warp_with(&warp_with(&ref_buf, &forward, params), &inverse, params);
        let central = extract_central_region(&ref_buf, &restored, 50);
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
    use imaginarium::ChannelLayout;

    use crate::image::metadata::derived_facts::DerivedFacts;
    use crate::image::metadata::image_metadata::ImageMetadata;
    use crate::image::metadata::observation_header::ObservationHeader;

    let pixels = star_field(Size2us::new(256, 256), 30, 11111)
        .image
        .channel(0)
        .clone();
    let width = pixels.width();
    let height = pixels.height();
    let mut image = LinearImage::from_interleaved(
        PlanarDesc::new((width, height), ChannelLayout::Mono),
        pixels.into_vec(),
    );
    image.metadata = ImageMetadata {
        header: ObservationHeader {
            object: Some("M42".to_string()),
            exposure_time: Some(120.0),
            ..Default::default()
        },
        derived: DerivedFacts::default(),
    };

    let transform = Transform::translation(DVec2::new(5.0, 5.0));
    let warp_config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    let warped = warp_config
        .warp(&image, &WarpTransform::new(transform))
        .image;

    // Verify the input's metadata is carried into the warped output (warp only
    // produces new pixel data).
    assert_eq!(warped.metadata.header.object, Some("M42".to_string()));
    assert_eq!(warped.metadata.header.exposure_time, Some(120.0));
}

/// The central regions of two images, row by row.
#[derive(Debug)]
struct CentralRegions {
    a: Vec<f32>,
    b: Vec<f32>,
}

/// Extract central region of two images for comparison (avoids border artifacts).
fn extract_central_region(a: &Buffer2<f32>, b: &Buffer2<f32>, margin: usize) -> CentralRegions {
    let size = a.size();
    debug_assert_eq!(size, b.size(), "two images of one size");
    let inner_width = size.width - 2 * margin;
    let inner_height = size.height - 2 * margin;

    let mut central_a = Vec::with_capacity(inner_width * inner_height);
    let mut central_b = Vec::with_capacity(inner_width * inner_height);

    for y in margin..(size.height - margin) {
        for x in margin..(size.width - margin) {
            let idx = size.idx_of(Vec2us::new(x, y));
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
/// `WarpTransform` bit for bit — whose positions are `T(p + c(p))` (see `RowPoss`' and
/// `WarpTransform`'s tests) — for every method, on a field whose correction reaches past 0.1 px.
#[test]
fn the_public_warp_samples_through_the_sip_correction() {
    use crate::internals::synthetic::distortion::{Grid, RadialField, RadialPairs};
    use crate::registration::distortion::sip::SipModel;
    use crate::registration::distortion::sip::sip_order::SipOrder;

    let size = Size2us::new(128, 128);
    let transform = Transform::translation(DVec2::new(5.0, -3.0));
    let field = RadialField {
        transform,
        ..RadialField::new(DVec2::new(64.0, 64.0), 3e-6)
    };
    let RadialPairs {
        reference: ref_points,
        target: target_points,
    } = field.pairs(Grid {
        start: 10,
        step: 11,
        extent: 109,
    });
    let sip = SipModel {
        order: SipOrder::Three,
        origin: field.centre,
    }
    .fitted_under(&transform, &ref_points, &target_points);
    assert!(sip.max_grid_correction(size, 10.0) > 0.1);
    let warp_transform = WarpTransform::with_sip(transform, sip);

    let pixels = star_field(size, 20, 12321).image.channel(0).clone();
    let image = LinearImage::from_interleaved(
        PlanarDesc::new((size.width, size.height), ChannelLayout::Mono),
        pixels.pixels().to_vec(),
    );
    for method in InterpolationMethod::ALL {
        let params = WarpConfig::with_method(method);
        let warped = params.warp(&image, &warp_transform).image;
        let plane = params.warp_plane(&pixels, &warp_transform);
        for (idx, (a, e)) in warped.channel(0).iter().zip(plane.iter()).enumerate() {
            assert_eq!(
                a.to_bits(),
                e.to_bits(),
                "{method:?} pixel {idx}: {a} against {e}"
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
    let image = LinearImage::from_interleaved(
        PlanarDesc::new((size.width, size.height), ChannelLayout::Mono),
        vec![V; size.pixel_count()],
    );

    // output(x,y) samples source (x + 2.5, y): columns 0..=12 are fully in
    // bounds, column 13 is half-covered (its right bilinear tap is off the
    // edge), columns 14..=15 fall entirely outside.
    let transform = Transform::translation(DVec2::new(2.5, 0.0));
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };
    assert_eq!(config.border_value, 0.0, "test assumes a zero border");

    let result = config.warp(&image, &WarpTransform::new(transform));
    let cov = result.coverage.pixels();
    let confidence = result.confidence.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| size.idx_of(Vec2us::new(x, y));

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
    let image = LinearImage::from_interleaved(
        PlanarDesc::new((size.width, size.height), ChannelLayout::Mono),
        vec![V; size.pixel_count()],
    );

    // src = (x + 3.5, y): a Lanczos3 (6-tap) kernel reaches off the right edge
    // for x ≳ 26 and lands entirely outside by x = 31.
    let transform = Transform::translation(DVec2::new(3.5, 0.0));
    let config = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        ..Default::default()
    };

    let result = config.warp(&image, &WarpTransform::new(transform));
    let cov = result.coverage.pixels();
    let confidence = result.confidence.pixels();
    let val = result.image.channel(0).pixels();
    let at = |x: usize, y: usize| size.idx_of(Vec2us::new(x, y));
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
