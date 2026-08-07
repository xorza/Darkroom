//! Drizzle reconstruction tests on forward-model dithered frame sets.
//!
//! The unit tests in `tests.rs` cover the kernel geometry, weight/linear-variance/coverage maps, and
//! pixel masks on hand-built frames. These verify the *reconstruction outcome* on realistic
//! sub-pixel-dithered renders: total flux is conserved, a source lands at its scale-mapped truth
//! position, and dithering recovers resolution a single undersampled frame cannot.

use common::CancelToken;
use glam::DVec2;

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::drizzle::accumulator::DrizzleFrame;
use crate::stacking::drizzle::config::DrizzleConfig;
use crate::stacking::drizzle::stack::drizzle_images;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::transform::Transform;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

/// Render one sub-pixel-dithered frame per offset, with the drizzle transform that registers it
/// back onto the common grid (`output = transform.apply(input)·scale`, so a frame whose star is
/// dithered to `pos + d` uses `translation(-d)` to land it at `pos·scale`).
fn dithered_frames(
    scene: &Scene,
    camera: &Camera,
    dithers: &[DVec2],
) -> (Vec<LinearImage>, Vec<Transform>) {
    dithers
        .iter()
        .map(|&d| {
            let obs = Observation {
                transform: Transform::translation(d),
                ..Observation::reference(0)
            };
            (
                render(scene, camera, &obs).image,
                Transform::translation(-d),
            )
        })
        .unzip()
}

fn drizzle_frames(
    images: Vec<LinearImage>,
    transforms: &[Transform],
) -> Vec<DrizzleFrame<LinearImage>> {
    assert_eq!(images.len(), transforms.len());
    images
        .into_iter()
        .zip(transforms.iter().copied())
        .map(|(source, transform)| DrizzleFrame::new(source, transform))
        .collect()
}

fn sum(px: &[f32]) -> f64 {
    px.iter().map(|&v| v as f64).sum()
}

fn peak(px: &[f32]) -> f32 {
    px.iter().copied().fold(f32::MIN, f32::max)
}

/// Flux-weighted centroid of a star on a (near-)zero background.
fn star_centroid(px: &[f32], size: Size2us) -> DVec2 {
    let mut s = 0.0;
    let mut sx = 0.0;
    let mut sy = 0.0;
    for y in 0..size.height {
        for x in 0..size.width {
            let v = px[size.index_of(Vec2us::new(x, y))] as f64;
            s += v;
            sx += v * x as f64;
            sy += v * y as f64;
        }
    }
    DVec2::new(sx / s, sy / s)
}

#[test]
fn drizzle_conserves_total_flux() {
    let size = Size2us::new(64, 64);
    let scene = Scene::single(
        size,
        DVec2::new(32.0, 32.0),
        5.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let camera = Camera::ideal(3.5);
    let dithers = [
        DVec2::ZERO,
        DVec2::new(0.5, 0.0),
        DVec2::new(0.0, 0.5),
        DVec2::new(0.5, 0.5),
    ];
    let (images, transforms) = dithered_frames(&scene, &camera, &dithers);
    let single_flux = sum(images[0].channel(0).pixels());

    // Drizzle preserves surface brightness, so Σ over the output ≈ scale²·Σ over an input frame.
    for scale in [1.0f32, 2.0] {
        let config = DrizzleConfig {
            scale,
            pixfrac: if scale == 1.0 { 1.0 } else { 0.8 },
            ..DrizzleConfig::default()
        };
        let result = drizzle_images(
            drizzle_frames(images.clone(), &transforms),
            &config,
            ProgressCallback::default(),
            &CancelToken::never(),
        )
        .unwrap();
        let out_flux = sum(result.image.channel(0).pixels());
        let expected = single_flux * (scale * scale) as f64;
        assert!(
            (out_flux - expected).abs() < expected * 0.05,
            "scale {scale}: Σ_out {out_flux:.3} vs scale²·Σ_in {expected:.3}"
        );
    }
}

#[test]
fn drizzle_places_star_at_scaled_truth_position() {
    let size = Size2us::new(64, 64);
    let pos = DVec2::new(28.0, 36.0);
    let scene = Scene::single(size, pos, 5.0, BackgroundField::Uniform { level: 0.0 });
    let camera = Camera::ideal(3.5);
    let dithers = [
        DVec2::ZERO,
        DVec2::new(0.4, 0.0),
        DVec2::new(0.0, 0.4),
        DVec2::new(0.4, 0.4),
        DVec2::new(-0.3, 0.2),
    ];
    let (images, transforms) = dithered_frames(&scene, &camera, &dithers);

    let scale = 2.0;
    let config = DrizzleConfig {
        scale,
        ..DrizzleConfig::default()
    };
    let result = drizzle_images(
        drizzle_frames(images, &transforms),
        &config,
        ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap();
    let out = result.image.channel(0);
    let center = star_centroid(out.pixels(), Size2us::new(out.width(), out.height()));
    assert!(
        (center.x - pos.x * scale as f64).abs() < 1.0,
        "centroid x {:.2} vs pos·scale {}",
        center.x,
        pos.x * scale as f64
    );
    assert!(
        (center.y - pos.y * scale as f64).abs() < 1.0,
        "centroid y {:.2} vs pos·scale {}",
        center.y,
        pos.y * scale as f64
    );
}

#[test]
fn drizzle_dithering_recovers_resolution() {
    let size = Size2us::new(48, 48);
    // Undersampled PSF (fwhm 1.8 < Nyquist 2) at a sub-pixel centre. Flux kept low so the tight
    // PSF peak stays unsaturated (otherwise both peaks clip at 1.0 and the comparison is moot).
    let scene = Scene::single(
        size,
        DVec2::new(24.3, 24.7),
        2.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let camera = Camera::ideal(1.8);
    let offs = [-1.0 / 3.0, 0.0, 1.0 / 3.0];
    let dithers: Vec<DVec2> = offs
        .iter()
        .flat_map(|&dx| offs.iter().map(move |&dy| DVec2::new(dx, dy)))
        .collect();
    let (images, transforms) = dithered_frames(&scene, &camera, &dithers);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.6,
        ..DrizzleConfig::default()
    };

    // Distinct sub-pixel dithers vs the same single frame replicated N times: same frame count and
    // flux, so the only difference is sub-pixel diversity. Recovering it sharpens the peak.
    let multi = drizzle_images(
        drizzle_frames(images.clone(), &transforms),
        &config,
        ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap();
    let replicated_imgs: Vec<LinearImage> = (0..dithers.len()).map(|_| images[4].clone()).collect();
    let replicated_tf: Vec<Transform> = (0..dithers.len()).map(|_| transforms[4]).collect();
    let replicated = drizzle_images(
        drizzle_frames(replicated_imgs, &replicated_tf),
        &config,
        ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap();

    let multi_peak = peak(multi.image.channel(0).pixels());
    let single_peak = peak(replicated.image.channel(0).pixels());
    assert!(
        multi_peak > single_peak,
        "dithered reconstruction should recover a higher peak: {multi_peak:.4} vs single {single_peak:.4}"
    );
}

#[test]
fn drizzle_emits_coverage_weight_and_linear_variance_maps() {
    // The coverage, weight, and linear-variance maps are drizzle's science deliverable; verify them
    // against the closed form for N equal-weight frames at full interior coverage.
    let size = Size2us::new(64, 64);
    let scene = Scene::single(
        size,
        DVec2::new(32.0, 32.0),
        5.0,
        BackgroundField::Uniform { level: 0.1 },
    );
    let camera = Camera::ideal(3.5);
    let dithers = [
        DVec2::ZERO,
        DVec2::new(0.5, 0.0),
        DVec2::new(0.0, 0.5),
        DVec2::new(0.5, 0.5),
    ];
    let (images, transforms) = dithered_frames(&scene, &camera, &dithers);
    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        ..DrizzleConfig::default()
    };
    let result = drizzle_images(
        drizzle_frames(images, &transforms),
        &config,
        ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap();

    // Coverage is normalized to [0,1]; the interior is fully covered by all 4 frames.
    let cov = result.coverage.as_ref().unwrap().pixels();
    assert!(
        cov.iter().all(|&c| (-1e-4..=1.0001).contains(&c)),
        "coverage must stay in [0,1]"
    );
    assert!(
        (cov[size.index_of(Vec2us::new(32, 32))] - 1.0).abs() < 0.05,
        "interior coverage {} should be ~1",
        cov[size.index_of(Vec2us::new(32, 32))]
    );

    // weight = Σwᵢ ≈ the 4 frames' total. variance = Σwᵢ²/(Σwᵢ)² = 1/N_eff; drizzle pools each
    // frame's drop across neighbouring output pixels, so N_eff ≥ the frame count (variance
    // smaller than a naive 1/4) — that pooling is the whole point of the WHT.
    let weight_c =
        result.weight.as_ref().unwrap().channel(0).pixels()[size.index_of(Vec2us::new(32, 32))];
    let var_c = result.linear_variance.as_ref().unwrap().channel(0).pixels()
        [size.index_of(Vec2us::new(32, 32))];
    let n_eff = 1.0 / var_c;
    println!("interior weight {weight_c:.3}, variance {var_c:.4}, N_eff {n_eff:.1}");
    assert!(
        (3.5..=4.5).contains(&weight_c),
        "interior weight {weight_c} should equal the 4 frames"
    );
    assert!(
        var_c > 0.0 && n_eff >= 4.0,
        "effective contributions {n_eff:.1} should be ≥ the 4 frames (drop pooling)"
    );
}
