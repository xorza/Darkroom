//! End-to-end stacking tests on forward-model frame sets.
//!
//! The unit tests in `stack.rs` / `rejection.rs` cover the combine and rejection math on
//! uniform and hand-built pixel stacks. These verify the *statistical* behaviour on realistic
//! frame sets with ground truth: stacking noise falls as `1/√N`, injected outliers are
//! rejected (the master recovers the clean truth where a plain mean is contaminated), and
//! inverse-noise weighting lowers the output variance on a mixed-quality set.

use common::CancelToken;

use crate::stacking::combine::config::{StackConfig, Weighting};
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::progress::ProgressCallback;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics::rms_diff;
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use crate::{ImageDimensions, LinearImage};
use imaginarium::Buffer2;

const W: usize = 128;
const H: usize = 128;

fn demo_scene(seed: u64) -> Scene {
    Scene::random_field(
        W,
        H,
        20,
        (4.0, 10.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        seed,
    )
}

/// Render `n` noisy frames of one scene with independent per-frame noise; the clean truth
/// (the noiseless signal every frame is a noisy realization of) is identical across frames.
fn frame_set(
    scene: &Scene,
    camera: &Camera,
    n: usize,
    base_seed: u64,
) -> (Vec<SimFrame>, Buffer2<f32>) {
    let sims: Vec<SimFrame> = (0..n)
        .map(|i| {
            render(
                scene,
                camera,
                &Observation::reference(base_seed.wrapping_add(i as u64 * 7919)),
            )
        })
        .collect();
    let clean = sims[0].truth.clean.clone();
    (sims, clean)
}

fn stack_frames(sims: &[SimFrame], config: StackConfig) -> LinearImage {
    let frames: Vec<StackFrame> = sims.iter().map(|s| s.image.clone().into()).collect();
    stack_images(
        frames,
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("stack")
    .image
}

/// Overwrite one pixel of one frame with a bright cosmic-ray-like spike.
fn inject_spike(sim: &mut SimFrame, x: usize, y: usize, value: f32) {
    let mut px = sim.image.channel(0).pixels().to_vec();
    px[y * W + x] = value;
    sim.image = LinearImage::from_planar_channels(ImageDimensions::new((W, H), 1), [px]);
}

/// Background pixels (clear of the margin-16 star field), one per injected frame.
fn outlier_sites() -> [(usize, usize, usize); 4] {
    // (frame, x, y) — corners are background (stars sit within [16, 112]).
    [(2, 6, 6), (5, 120, 6), (8, 6, 120), (11, 120, 120)]
}

#[test]
fn mean_stack_reduces_noise_as_sqrt_n() {
    let scene = demo_scene(1);
    let camera = Camera::realistic(4.0);
    let n = 16;
    let (sims, clean) = frame_set(&scene, &camera, n, 100);

    // Residual RMS vs the clean truth: a single frame vs the N-frame mean.
    let single_rms = rms_diff(sims[0].image.channel(0).pixels(), clean.pixels());
    let stack = stack_frames(&sims, StackConfig::mean());
    let stack_rms = rms_diff(stack.channel(0).pixels(), clean.pixels());

    // Averaging N independent frames shrinks the noise by √N.
    let ratio = single_rms / stack_rms;
    let expected = (n as f64).sqrt();
    assert!(
        (ratio - expected).abs() < expected * 0.2,
        "noise-reduction ratio {ratio:.2} should be ≈ √{n} = {expected:.2} \
         (single {single_rms:.5}, stack {stack_rms:.5})"
    );
}

#[test]
fn sigma_clip_rejects_injected_outliers_where_mean_is_contaminated() {
    let scene = demo_scene(2);
    let camera = Camera::realistic(4.0);
    let n = 14;
    let (mut sims, clean) = frame_set(&scene, &camera, n, 200);

    let sites = outlier_sites();
    for &(f, x, y) in &sites {
        // Precondition: these are background pixels (so the spike is unambiguous).
        assert!(
            clean.pixels()[y * W + x] < 0.2,
            "outlier site ({x},{y}) must be background"
        );
        inject_spike(&mut sims[f], x, y, 1.0);
    }

    let mean = stack_frames(&sims, StackConfig::mean());
    let clipped = stack_frames(&sims, StackConfig::sigma_clipped(2.5));

    for &(_, x, y) in &sites {
        let idx = y * W + x;
        let truth = clean.pixels()[idx];
        // A plain mean is dragged toward the spike: ≈ (1.0 + (n-1)·bg)/n above truth.
        let mean_err = (mean.channel(0).pixels()[idx] - truth).abs();
        // Sigma-clip discards the spike and recovers the clean background.
        let clip_err = (clipped.channel(0).pixels()[idx] - truth).abs();
        assert!(
            mean_err > 0.04,
            "mean should be contaminated at ({x},{y}): err {mean_err:.4}"
        );
        assert!(
            clip_err < 0.01,
            "sigma-clip should recover truth at ({x},{y}): err {clip_err:.4}"
        );
    }
}

#[test]
fn all_rejection_methods_remove_outliers() {
    let scene = demo_scene(3);
    let camera = Camera::realistic(4.0);
    let n = 14;
    let (mut sims, clean) = frame_set(&scene, &camera, n, 300);

    let sites = outlier_sites();
    for &(f, x, y) in &sites {
        inject_spike(&mut sims[f], x, y, 1.0);
    }

    // Every rejecting combine (and the robust median) recovers the clean background.
    let configs: [(&str, StackConfig); 6] = [
        ("sigma_clip", StackConfig::sigma_clipped(2.5)),
        ("winsorized", StackConfig::winsorized(2.5)),
        ("linear_fit", StackConfig::linear_fit(2.5)),
        ("percentile", StackConfig::percentile(20.0)),
        ("gesd", StackConfig::gesd()),
        ("median", StackConfig::median()),
    ];
    for (name, config) in configs {
        let stacked = stack_frames(&sims, config);
        for &(_, x, y) in &sites {
            let idx = y * W + x;
            let err = (stacked.channel(0).pixels()[idx] - clean.pixels()[idx]).abs();
            assert!(
                err < 0.03,
                "{name} should reject the outlier at ({x},{y}): err {err:.4}"
            );
        }
    }
}

#[test]
fn noise_weighting_beats_equal_on_mixed_quality_frames() {
    let scene = demo_scene(4);
    // Six low-noise frames (deep well) + six noisy frames (shallow well, high read noise).
    let low = Camera::realistic(4.0);
    let high = Camera {
        full_well_e: 2_000.0,
        read_noise_e: 30.0,
        ..Camera::realistic(4.0)
    };
    let mut sims: Vec<SimFrame> = (0..6)
        .map(|i| render(&scene, &low, &Observation::reference(400 + i * 7919)))
        .collect();
    sims.extend((0..6).map(|i| render(&scene, &high, &Observation::reference(900 + i * 7919))));
    let clean = sims[0].truth.clean.clone();

    let equal = stack_frames(
        &sims,
        StackConfig {
            weighting: Weighting::Equal,
            ..StackConfig::mean()
        },
    );
    let weighted = stack_frames(
        &sims,
        StackConfig {
            weighting: Weighting::Noise,
            ..StackConfig::mean()
        },
    );

    let equal_rms = rms_diff(equal.channel(0).pixels(), clean.pixels());
    let weighted_rms = rms_diff(weighted.channel(0).pixels(), clean.pixels());
    // The noisy frames carry ~12× the per-pixel σ, so inverse-variance weighting nearly ignores
    // them — the residual must drop by a clear margin, not just epsilon.
    let ratio = equal_rms / weighted_rms;
    assert!(
        ratio > 1.5,
        "inverse-noise weighting should clearly beat equal: ratio {ratio:.2} \
         (weighted {weighted_rms:.5} vs equal {equal_rms:.5})"
    );
}

#[test]
fn rejection_methods_preserve_clean_frames() {
    // With no injected outliers, a rejecting combine must not damage the result: its residual vs
    // truth stays close to the plain mean's. This is the precision complement to the recall
    // tests — rejection must not eat good pixels.
    let scene = demo_scene(5);
    let camera = Camera::realistic(4.0);
    let (sims, clean) = frame_set(&scene, &camera, 14, 500);
    let mean_rms = rms_diff(
        stack_frames(&sims, StackConfig::mean()).channel(0).pixels(),
        clean.pixels(),
    );
    for (name, config) in [
        ("sigma_clip", StackConfig::sigma_clipped(2.5)),
        ("winsorized", StackConfig::winsorized(2.5)),
        ("gesd", StackConfig::gesd()),
    ] {
        let rms = rms_diff(
            stack_frames(&sims, config).channel(0).pixels(),
            clean.pixels(),
        );
        assert!(
            rms < mean_rms * 1.5,
            "{name} must not damage clean frames: rms {rms:.5} vs mean {mean_rms:.5}"
        );
    }
}
