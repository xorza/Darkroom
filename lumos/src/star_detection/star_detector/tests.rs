use glam::DVec2;

use crate::internals::images::gray_image;
use crate::internals::synthetic::scene::Placement;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::extraction_config::ExtractionConfig;
use crate::star_detection::star_detection_config::fwhm_config::FwhmMode;
use crate::star_detection::star_detector::detection_result::*;
use crate::star_detection::star_detector::*;
use crate::star_detection::tests::Scenario;

/// A Gaussian star of FWHM 2 keeps all its flux on a demosaiced frame: the
/// detection plane takes the 3×3 median there, and measurement reads the plane no filter
/// touched, so the frame read as demosaiced and as measured gives the same star, bit for bit.
///
/// Amplitude 1 on a sky of 0.1, the same in all three channels with a noise of 0.001. The flux
/// is the rendered star's sum, 4.53, plus the noise over the 15 × 15 stamp of FWHM 4,
/// 0.015 of standard deviation: within 5 of them.
#[test]
fn a_demosaiced_frame_measures_its_stars_unfiltered() {
    use glam::Vec2;
    use imaginarium::Buffer2;

    use crate::image::image_provenance::{
        ColorProvenance, DemosaicProvenance, ImageProvenance, RowOrder, SourceProvenance,
    };
    use crate::image::linear_image::LinearImage;

    use crate::internals::images::rgb_image;
    use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
    use crate::internals::test_rng::TestRng;
    use crate::math::size2us::Size2us;

    let size = Size2us::new(64, 64);
    let mut star = Buffer2::new_filled(size.width, size.height, 0.0f32);
    SyntheticStar::new(
        Vec2::new(32.3, 31.6),
        1.0,
        StarProfile::gaussian(2.0 / 2.354_82),
    )
    .add_to(&mut star);
    let truth: f64 = star.pixels().iter().map(|&value| f64::from(value)).sum();
    let mut rng = TestRng::new(3);
    let channel: Vec<f32> = star
        .pixels()
        .iter()
        .map(|&value| 0.1 + value + 0.001 * rng.next_gaussian_f32())
        .collect();
    let frame = |demosaic| {
        let mut image = rgb_image(size, [channel.clone(), channel.clone(), channel.clone()]);
        image.metadata.derived.set_provenance(Some(ImageProvenance {
            source: SourceProvenance::CameraRaw,
            color: ColorProvenance::SensorRgb,
            clipped: true,
            demosaic,
            row_order: RowOrder::TopDown,
        }));
        image
    };
    // A star this narrow, off the pixel grid, reads lopsided marginals; its shape is not what
    // this measures.
    let mut config = StarDetectionConfig::default();
    config.filter.max_roundness = 2.0;
    let brightest = |image: &LinearImage| {
        StarDetector::from_config(config.clone())
            .unwrap()
            .detect(image)
            .stars
            .into_iter()
            .next()
            .expect("the star is detected")
    };
    let demosaiced = brightest(&frame(DemosaicProvenance::LumosRcd));
    let measured = brightest(&frame(DemosaicProvenance::None));
    assert_eq!(demosaiced.pos, measured.pos);
    assert_eq!(demosaiced.flux.to_bits(), measured.flux.to_bits());
    assert_eq!(demosaiced.peak.to_bits(), measured.peak.to_bits());
    assert_eq!(demosaiced.fwhm.to_bits(), measured.fwhm.to_bits());
    assert!(
        (f64::from(demosaiced.flux) - truth).abs() <= 5.0 * 0.015,
        "flux {} of {truth}",
        demosaiced.flux
    );
}

/// Review phase 7 and S9: detection does not depend on the frame's scale, its zero point or
/// its orientation. The star field is put on the exact grid of `Affine::CASES`; its saturation
/// is flagged by the decoder, with no flags, so no level stated in the frame's units enters.
/// - Under a power-of-two scale every sum, median and product scales exactly, so the stars
///   are the same, bit for bit, their flux scaled.
/// - Under a pedestal each sample rounds by up to half an ulp of the largest sample, `u`,
///   before the sky takes the pedestal out. Against the faintest star's peak `A`, scaled, that
///   moves a centroid by at most the stamp radius, 7 px at FWHM 4, times `u / (s·A)`. The sky
///   itself is computed at the pedestal's magnitude in f32 — the Pearson mode `2.5·median −
///   1.5·mean`, the plane fit, the bicubic spline — and rounds by a few ulps of it, which every
///   pixel of a flux's 15 × 15 stamp carries alike: 32 `u` per pixel bound a flux's change by
///   `225·32·u / (s·F)` of the faintest flux `F`.
/// - Rotated by 180°, on a frame of whole tiles, the mesh's tiles and samples map onto
///   themselves, so every star maps onto its own; only the order of a few sums changes, which
///   moves a centroid by under 1e-9 px.
#[test]
fn detection_is_invariant_to_scale_zero_point_and_rotation() {
    use crate::internals::invariance::Affine;

    let frame = Scenario::default().frame();
    let size = Size2us::new(frame.image.width(), frame.image.height());
    let base: Vec<f32> = frame
        .image
        .channel(0)
        .pixels()
        .iter()
        .map(|&value| Affine::quantize(value))
        .collect();
    let detect = |pixels: Vec<f32>| {
        let mut image = gray_image(size, pixels);
        image.metadata.derived.set_saturation_flagged(true);
        StarDetector::default().detect(&image).stars
    };
    let reference = detect(base.clone());
    assert!(reference.len() > 10, "{}", reference.len());
    let faintest = |of: fn(&Star) -> f32| reference.iter().map(of).fold(f32::INFINITY, f32::min);
    let (peak, flux) = (faintest(|star| star.peak), faintest(|star| star.flux));

    for case in Affine::CASES {
        let stars = detect(case.apply_all(&base));
        assert_eq!(stars.len(), reference.len(), "{case:?}");
        for (star, expected) in stars.iter().zip(&reference) {
            let ratio = star.flux / case.scale / expected.flux - 1.0;
            if case.offset == 0.0 {
                assert_eq!(star.pos, expected.pos, "{case:?}");
                assert_eq!(ratio, 0.0, "{case:?}");
            } else {
                let rounding = f64::from((case.offset + case.scale) * f32::EPSILON / 2.0);
                let moved = (star.pos - expected.pos).length();
                assert!(
                    moved <= 7.0 * rounding / f64::from(case.scale * peak),
                    "{case:?}: {moved}"
                );
                assert!(
                    f64::from(ratio.abs())
                        <= 225.0 * 32.0 * rounding / f64::from(case.scale * flux),
                    "{case:?}: {ratio}"
                );
            }
        }
    }

    let flip = |pos: DVec2| {
        DVec2::new(
            size.width as f64 - 1.0 - pos.x,
            size.height as f64 - 1.0 - pos.y,
        )
    };
    let order = |a: &DVec2, b: &DVec2| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x));
    let mut rotated: Vec<DVec2> = detect(base.iter().rev().copied().collect())
        .iter()
        .map(|star| flip(star.pos))
        .collect();
    let mut expected: Vec<DVec2> = reference.iter().map(|star| star.pos).collect();
    rotated.sort_by(order);
    expected.sort_by(order);
    assert_eq!(rotated.len(), expected.len());
    for (rotated, expected) in rotated.iter().zip(&expected) {
        assert!(
            (*rotated - *expected).length() < 1e-9,
            "{rotated} vs {expected}"
        );
    }
}

#[test]
fn fwhm_source_distinguishes_measured_from_supplied() {
    // `value` reports what the detector ran with; only a measured one is `was_estimated`.
    assert_eq!(FwhmSource::Configured(3.5).value(), Some(3.5));
    assert_eq!(
        FwhmSource::Estimated {
            fwhm: 3.5,
            stars_used: 12
        }
        .value(),
        Some(3.5)
    );
    assert_eq!(FwhmSource::Disabled.value(), None);
    // A default-constructed `DetectionDiagnostics` reports no FWHM rather than a bogus 0.0.
    assert_eq!(DetectionDiagnostics::default().fwhm, FwhmSource::Disabled);
}

#[test]
fn constructor_rejects_invalid_configuration() {
    let error = StarDetector::from_config(StarDetectionConfig {
        extraction: ExtractionConfig {
            sigma_threshold: 0.0,
            ..Default::default()
        },
        ..StarDetectionConfig::default()
    })
    .unwrap_err();
    assert_eq!((error.field, error.value), ("sigma_threshold", 0.0));
}

/// Stars up to 2.5× the 4 px seed come back at their own width: the estimate measures again
/// at the stamp the first pass implies. Measured once at the seed's stamp they read narrow —
/// 6.78 for a 10 px star.
///
/// Flux scales with FWHM² to hold each star's peak; the brightest have SNR in the hundreds,
/// so a star's width carries about 1% noise and the median of 13 or more well under that.
/// 1% bounds it (measured: within 0.02%).
#[test]
fn auto_fwhm_recovers_wide_stars() {
    for fwhm in [5.0f32, 6.0, 8.0, 10.0] {
        let scale = fwhm * fwhm / 16.0;
        let frame = Scenario {
            size: Size2us::new(384, 384),
            num_stars: 20,
            flux: (5.0 * scale, 14.0 * scale),
            fwhm,
            placement: Placement::Uniform { margin: 40.0 },
            ..Default::default()
        }
        .frame();
        let mut config = StarDetectionConfig::default();
        config.fwhm.mode = Some(FwhmMode::Auto { fallback: 4.0 });
        let result = StarDetector::from_config(config)
            .unwrap()
            .detect(&frame.image);
        assert!(
            matches!(result.diagnostics.fwhm, FwhmSource::Estimated { .. }),
            "FWHM {fwhm}"
        );
        let estimate = result.diagnostics.fwhm.value().unwrap();
        assert!(
            (estimate - fwhm).abs() < 0.01 * fwhm,
            "FWHM {fwhm} estimated as {estimate}"
        );
    }
}

#[test]
fn auto_estimated_fwhm_is_used_for_final_measurement() {
    for (actual_fwhm, configured_seed, flux) in [(2.5, 8.0, (3.0, 8.0)), (7.0, 1.0, (10.0, 30.0))] {
        let frame = Scenario {
            num_stars: 40,
            flux,
            fwhm: actual_fwhm,
            ..Default::default()
        }
        .frame();
        let mut auto_config = StarDetectionConfig::default();
        auto_config.fwhm.mode = Some(FwhmMode::Auto {
            fallback: configured_seed,
        });
        auto_config.fwhm.min_stars = 5;
        auto_config.filter.min_snr = 1.0;
        auto_config.filter.max_eccentricity = 1.0;
        auto_config.filter.max_sharpness = 1.0;
        auto_config.filter.max_roundness = 2.0;
        auto_config.filter.max_fwhm_deviation = None;
        auto_config.filter.duplicate_min_separation = 0.0;

        let auto_result = StarDetector::from_config(auto_config.clone())
            .unwrap()
            .detect(&frame.image);
        assert!(
            matches!(auto_result.diagnostics.fwhm, FwhmSource::Estimated { .. }),
            "FWHM {actual_fwhm} fixture must produce a genuine estimate"
        );
        let effective_fwhm = auto_result
            .diagnostics
            .fwhm
            .value()
            .expect("an estimated FWHM has a value");
        assert!(
            (effective_fwhm - configured_seed).abs() > 1.0,
            "fixture must estimate far from its configured seed: estimate {effective_fwhm}, seed {configured_seed}"
        );

        let mut manual_config = auto_config;
        manual_config.fwhm.mode = Some(FwhmMode::Fixed(effective_fwhm));
        let manual_result = StarDetector::from_config(manual_config)
            .unwrap()
            .detect(&frame.image);

        assert_eq!(
            auto_result.stars.len(),
            manual_result.stars.len(),
            "auto and equivalent manual FWHM must retain the same stars for PSF {actual_fwhm}"
        );
        for (auto, manual) in auto_result.stars.iter().zip(&manual_result.stars) {
            assert_eq!(auto.pos, manual.pos);
            assert_eq!(auto.flux.to_bits(), manual.flux.to_bits());
            assert_eq!(auto.fwhm.to_bits(), manual.fwhm.to_bits());
            assert_eq!(auto.eccentricity.to_bits(), manual.eccentricity.to_bits());
            assert_eq!(auto.snr.to_bits(), manual.snr.to_bits());
            assert_eq!(auto.peak.to_bits(), manual.peak.to_bits());
            assert_eq!(auto.sharpness.to_bits(), manual.sharpness.to_bits());
            assert_eq!(
                auto.roundness.ground.to_bits(),
                manual.roundness.ground.to_bits()
            );
            assert_eq!(
                auto.roundness.sround.to_bits(),
                manual.roundness.sround.to_bits()
            );
        }
    }
}
