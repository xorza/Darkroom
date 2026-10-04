//! End-to-end stacking tests on forward-model frame sets.
//!
//! The unit tests in `stack.rs` / `rejection.rs` cover the combine and rejection math on
//! uniform and hand-built pixel stacks. These verify the *statistical* behaviour on realistic
//! frame sets with ground truth: stacking noise falls as `1/√N`, injected outliers are
//! rejected (the master recovers the clean truth where a plain mean is contaminated), and
//! inverse-noise weighting lowers the output variance on a mixed-quality set.

mod mem_budget;
mod mem_budget_probe;

use crate::combine::config::{StackConfig, Weighting};
use crate::combine::stack::{StackFrame, stack_images};
use crate::internals::prelude::*;
use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::metrics::rms_diff;
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::progress::progress_callback::ProgressCallback;

const W: usize = 128;
const H: usize = 128;

fn demo_scene(seed: u64) -> Scene {
    Scene::random_field(
        Size2us::new(W, H),
        20,
        (4.0, 10.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        seed,
    )
}

/// The noisy frames and the clean truth they share.
#[derive(Debug)]
struct FrameSet {
    sims: Vec<SimFrame>,
    clean: Buffer2<f32>,
}

/// Render `n` noisy frames of one scene with independent per-frame noise; the clean truth
/// (the noiseless signal every frame is a noisy realization of) is identical across frames.
fn frame_set(scene: &Scene, camera: &Camera, n: usize, base_seed: u64) -> FrameSet {
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
    FrameSet { sims, clean }
}

fn stack_frames(sims: &[SimFrame], config: &StackConfig) -> LinearImage {
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
fn inject_spike(sim: &mut SimFrame, pos: Vec2us, value: f32) {
    let mut px = sim.image.channel(0).pixels().to_vec();
    px[Size2us::new(W, H).index_of(pos)] = value;
    sim.image = LinearImage::from_planar_channels(ImageDimensions::new((W, H), 1), [px]);
}

/// A background pixel's noise in one frame of `Camera::realistic`: shot noise on the 0.1 sky over a
/// 50 000 e⁻ well, and 3 e⁻ of read noise — `√(0.1/50 000 + (3/50 000)²)`.
const SKY_SIGMA: f64 = 0.001_415_5;

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
    let FrameSet { sims, clean } = frame_set(&scene, &camera, n, 100);

    // Residual RMS vs the clean truth: a single frame vs the N-frame mean.
    let single_rms = rms_diff(sims[0].image.channel(0).pixels(), clean.pixels());
    let stack = stack_frames(&sims, &StackConfig::mean());
    let stack_rms = rms_diff(stack.channel(0).pixels(), clean.pixels());

    // Averaging N independent frames shrinks the noise by √N. Each RMS over 16 384 pixels carries
    // a relative sampling error of 1/√(2 · 16 384) = 0.55%, their ratio 0.8%; 5 of those is 4%.
    let ratio = single_rms / stack_rms;
    let expected = (n as f64).sqrt();
    assert!(
        (ratio - expected).abs() < expected * 0.04,
        "noise-reduction ratio {ratio:.2} should be ≈ √{n} = {expected:.2} \
         (single {single_rms:.5}, stack {stack_rms:.5})"
    );
}

#[test]
fn sigma_clip_rejects_injected_outliers_where_mean_is_contaminated() {
    let scene = demo_scene(2);
    let camera = Camera::realistic(4.0);
    let n = 14;
    let FrameSet { mut sims, clean } = frame_set(&scene, &camera, n, 200);

    let sites = outlier_sites();
    for &(f, x, y) in &sites {
        // Precondition: these are background pixels (so the spike is unambiguous).
        assert!(
            clean.pixels()[y * W + x] < 0.2,
            "outlier site ({x},{y}) must be background"
        );
        inject_spike(&mut sims[f], Vec2us::new(x, y), 1.0);
    }

    let mean = stack_frames(&sims, &StackConfig::mean());
    let clipped = stack_frames(&sims, &StackConfig::sigma_clipped(2.5));

    // A plain mean is dragged toward the spike by (1 − 0.1)/14 = 0.0643, give or take the 14
    // frames' noise, SKY_SIGMA/√14 = 3.8e-4. Sigma clipping drops the spike, and the 13 frames
    // left sit within 5 × SKY_SIGMA/√13 = 2e-3 of the truth.
    let noise = |frames: f64| 5.0 * SKY_SIGMA / frames.sqrt();
    for &(_, x, y) in &sites {
        let idx = y * W + x;
        let truth = clean.pixels()[idx];
        let mean_err = f64::from(mean.channel(0).pixels()[idx] - truth);
        let clip_err = f64::from(clipped.channel(0).pixels()[idx] - truth);
        assert!(
            (mean_err - (1.0 - f64::from(truth)) / 14.0).abs() < noise(14.0),
            "mean should be contaminated at ({x},{y}): err {mean_err:.4}"
        );
        assert!(
            clip_err.abs() < noise(13.0),
            "sigma-clip should recover truth at ({x},{y}): err {clip_err:.4}"
        );
    }
}

/// Every rejecting combine, and the median, drops the spikes. Fifteen frames clear every method's
/// small-stack floor — GESD's is 15 — so each runs as configured rather than as the median. The
/// fewest frames any keeps is 9, percentile's at 20% of 15, whose mean sits within
/// 5 × `SKY_SIGMA`/√9 = 2.4e-3 of the truth; a median's √(π/2) wider spread over 15 is less.
/// A spike kept would be 0.9/9 = 0.1 off.
#[test]
fn all_rejection_methods_remove_outliers() {
    let scene = demo_scene(3);
    let camera = Camera::realistic(4.0);
    let n = 15;
    let FrameSet { mut sims, clean } = frame_set(&scene, &camera, n, 300);

    let sites = outlier_sites();
    for &(f, x, y) in &sites {
        inject_spike(&mut sims[f], Vec2us::new(x, y), 1.0);
    }

    // Every rejecting combine (and the robust median) recovers the clean background.
    let configs: [(&str, StackConfig); 6] = [
        ("sigma_clip", StackConfig::sigma_clipped(2.5)),
        ("winsorized", StackConfig::winsorized(2.5)),
        ("linear_fit", StackConfig::linear_fit(2.5)),
        ("trim", StackConfig::trim(20.0)),
        ("gesd", StackConfig::gesd()),
        ("median", StackConfig::median()),
    ];
    for (name, config) in configs {
        assert_eq!(
            config.small_n.resolve(config.method, n),
            config.method,
            "{name} must run as configured"
        );
        let stacked = stack_frames(&sims, &config);
        for &(_, x, y) in &sites {
            let idx = y * W + x;
            let err = f64::from(stacked.channel(0).pixels()[idx] - clean.pixels()[idx]).abs();
            assert!(
                err < 5.0 * SKY_SIGMA / 3.0,
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
        &StackConfig {
            weighting: Weighting::Equal,
            ..StackConfig::mean()
        },
    );
    let weighted = stack_frames(
        &sims,
        &StackConfig {
            weighting: Weighting::Noise,
            ..StackConfig::mean()
        },
    );

    let equal_rms = rms_diff(equal.channel(0).pixels(), clean.pixels());
    let weighted_rms = rms_diff(weighted.channel(0).pixels(), clean.pixels());
    // The camera model predicts both: at a pixel of clean signal s a frame's variance is
    // s/well + (read/well)², the weights are the inverse variances at the 0.1 sky, and a mean's
    // variance is Σ wᵢ²vᵢ/(Σ wᵢ)². The ratio of the two predicted RMSs over the frame — about 5.7,
    // the noisy frames carrying 12× the σ — holds to the two estimates' sampling error, 0.8%,
    // times 5.
    let variance = |signal: f64, well: f64, read: f64| signal / well + (read / well).powi(2);
    let (low_weight, high_weight) = (
        1.0 / variance(0.1, 50_000.0, 3.0),
        1.0 / variance(0.1, 2_000.0, 30.0),
    );
    let (mut equal_variance, mut weighted_variance) = (0.0, 0.0);
    for &signal in clean.pixels() {
        let (low, high) = (
            variance(f64::from(signal), 50_000.0, 3.0),
            variance(f64::from(signal), 2_000.0, 30.0),
        );
        equal_variance += 6.0 * (low + high) / 144.0;
        weighted_variance += 6.0 * (low_weight.powi(2) * low + high_weight.powi(2) * high)
            / (6.0 * (low_weight + high_weight)).powi(2);
    }
    let predicted = (equal_variance / weighted_variance).sqrt();
    let ratio = equal_rms / weighted_rms;
    assert!(
        (ratio / predicted - 1.0).abs() < 0.04,
        "inverse-noise weighting: ratio {ratio:.3} against the predicted {predicted:.3} \
         (weighted {weighted_rms:.5} vs equal {equal_rms:.5})"
    );
}

/// With no injected outliers a rejecting combine must not damage the result — the precision
/// complement to the recall tests. Clipping Gaussian noise at 2.5σ estimated from 15 samples drops
/// a few percent of them, raising the stack's variance by as much and its RMS by half that; with
/// the two RMS estimates' sampling error, 0.8% times 5, a rejecting stack stays within 5% of the
/// plain mean's.
#[test]
fn rejection_methods_preserve_clean_frames() {
    let scene = demo_scene(5);
    let camera = Camera::realistic(4.0);
    let FrameSet { sims, clean } = frame_set(&scene, &camera, 15, 500);
    let mean_rms = rms_diff(
        stack_frames(&sims, &StackConfig::mean())
            .channel(0)
            .pixels(),
        clean.pixels(),
    );
    for (name, config) in [
        ("sigma_clip", StackConfig::sigma_clipped(2.5)),
        ("winsorized", StackConfig::winsorized(2.5)),
        ("gesd", StackConfig::gesd()),
    ] {
        let rms = rms_diff(
            stack_frames(&sims, &config).channel(0).pixels(),
            clean.pixels(),
        );
        assert!(
            rms < mean_rms * 1.05,
            "{name} must not damage clean frames: rms {rms:.5} vs mean {mean_rms:.5}"
        );
    }
}
