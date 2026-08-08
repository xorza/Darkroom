use crate::testing::synthetic::camera::{BiasField, FlatField, SensorDefects};
use crate::testing::synthetic::metrics::pixel_stats;
use crate::testing::synthetic::observe::*;
use crate::testing::synthetic::scene::{BackgroundField, Scene};

fn argmax_xy(pixels: &[f32], width: usize) -> (usize, usize) {
    let (i, _) = pixels
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    (i % width, i / width)
}

#[test]
fn ideal_render_equals_clean() {
    // Dim scene so the clean peak stays below saturation (no clamp difference).
    let scene = Scene::single(
        Size2us::new(64, 64),
        DVec2::new(32.0, 32.0),
        2.0,
        BackgroundField::Uniform { level: 0.1 },
    );
    let frame = render(&scene, &Camera::ideal(3.0), &Observation::reference(1));
    let img = frame.image.channel(0).pixels();
    let clean = frame.truth.clean.pixels();
    assert!(img.iter().all(|&p| p <= 1.0));
    for (a, b) in img.iter().zip(clean) {
        assert_eq!(a, b, "ideal render must be noiseless");
    }
}

#[test]
fn source_lands_at_transformed_position() {
    let scene = Scene::single(
        Size2us::new(64, 64),
        DVec2::new(20.0, 20.0),
        5.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let obs = Observation {
        transform: Transform::translation(DVec2::new(5.0, -3.0)),
        ..Observation::reference(1)
    };
    let frame = render(&scene, &Camera::ideal(3.0), &obs);
    assert_eq!(frame.truth.sources[0].pos, DVec2::new(25.0, 17.0));
    let (px, py) = argmax_xy(frame.image.channel(0).pixels(), 64);
    assert_eq!((px, py), (25, 17));
}

#[test]
fn flux_conserved_in_clean() {
    let scene = Scene::single(
        Size2us::new(81, 81),
        DVec2::new(40.0, 40.0),
        50.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let frame = render(&scene, &Camera::ideal(4.0), &Observation::reference(1));
    let sum: f32 = frame.truth.clean.pixels().iter().sum();
    assert!((sum - 50.0).abs() < 1.0, "sum {sum}");
}

#[test]
fn flat_applied_to_clean_truth() {
    // Uniform 0.3 sky, vignette flat: clean = bg × flat, so center (≈0.3) > darkened corner.
    let scene = Scene {
        size: Size2us::new(64, 64),
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.3 },
    };
    let camera = Camera {
        flat: FlatField {
            vignette: Some((1.0, 0.5, 2.0)),
            channel_gain: [1.0; 3],
        },
        ..Camera::ideal(3.0)
    };
    let frame = render(&scene, &camera, &Observation::reference(1));
    let clean = frame.truth.clean.pixels();
    assert!(
        clean[32 * 64 + 32] > clean[0],
        "vignette must darken corners"
    );
    assert!((clean[32 * 64 + 32] - 0.3).abs() < 0.02);
}

#[test]
fn noise_raises_variance_but_keeps_mean() {
    let scene = Scene {
        size: Size2us::new(128, 128),
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.2 },
    };
    let ideal = render(&scene, &Camera::ideal(3.0), &Observation::reference(7));
    let noisy = render(&scene, &Camera::realistic(3.0), &Observation::reference(7));
    let s_ideal = pixel_stats(ideal.image.channel(0).pixels());
    let s_noisy = pixel_stats(noisy.image.channel(0).pixels());
    assert!(s_ideal.std < 1e-6, "ideal std {}", s_ideal.std);
    assert!(s_noisy.std > 1e-3, "noisy std {}", s_noisy.std);
    // Mean preserved (shot+read are zero-mean perturbations; dark adds ~0.05·1/50000 ≈ 1e-6).
    assert!(
        (s_noisy.mean - 0.2).abs() < 2e-3,
        "noisy mean {}",
        s_noisy.mean
    );
}

#[test]
fn dither_shifts_peak() {
    let scene = Scene::single(
        Size2us::new(64, 64),
        DVec2::new(20.0, 32.0),
        5.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let frames = observe_dithered(
        &scene,
        &Camera::ideal(3.0),
        &[DVec2::new(0.0, 0.0), DVec2::new(8.0, 0.0)],
        1.0,
        1,
    );
    let (x0, _) = argmax_xy(frames[0].image.channel(0).pixels(), 64);
    let (x1, _) = argmax_xy(frames[1].image.channel(0).pixels(), 64);
    assert_eq!(x0, 20);
    assert_eq!(x1, 28);
}

#[test]
fn bias_and_defects_applied() {
    let scene = Scene {
        size: Size2us::new(32, 32),
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.0 },
    };
    let camera = Camera {
        bias: BiasField {
            offset: 0.05,
            bad_columns: vec![],
        },
        defects: SensorDefects {
            hot: vec![(10, 10, 0.5)],
            dead: vec![(5, 5)],
        },
        ..Camera::ideal(3.0)
    };
    let frame = render(&scene, &camera, &Observation::reference(1));
    let px = frame.image.channel(0).pixels();
    // Ordinary pixel = bias only.
    assert!((px[0] - 0.05).abs() < 1e-6);
    // Hot pixel = bias + excess.
    assert!((px[10 * 32 + 10] - 0.55).abs() < 1e-6);
    // Dead pixel forced low (applied after bias).
    assert_eq!(px[5 * 32 + 5], 0.0);
}

#[test]
fn saturation_clamps_bright_source() {
    // A flux-20 source at fwhm 3 has a clean peak ~2.0; the well clips it at `saturation`.
    let scene = Scene::single(
        Size2us::new(64, 64),
        DVec2::new(32.0, 32.0),
        20.0,
        BackgroundField::Uniform { level: 0.1 },
    );
    let camera = Camera {
        saturation: 0.8,
        ..Camera::ideal(3.0)
    };
    let frame = render(&scene, &camera, &Observation::reference(1));
    let px = frame.image.channel(0).pixels();
    let max = px.iter().copied().fold(0.0f32, f32::max);
    assert!(
        (max - 0.8).abs() < 1e-6,
        "saturation must clamp the peak to 0.8, got {max}"
    );
    assert!(
        px.iter().all(|&p| p <= 0.8 + 1e-6),
        "no pixel may exceed the saturation level"
    );
    // Truth keeps the unclamped clean signal (peak well above the clip).
    let clean_max = frame
        .truth
        .clean
        .pixels()
        .iter()
        .copied()
        .fold(0.0f32, f32::max);
    assert!(
        clean_max > 1.0,
        "clean truth peak {clean_max} should be unclamped"
    );
}

#[test]
fn bad_columns_raise_their_column() {
    let scene = Scene {
        size: Size2us::new(32, 32),
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.1 },
    };
    let camera = Camera {
        bias: BiasField {
            offset: 0.0,
            bad_columns: vec![(7, 0.2)],
        },
        ..Camera::ideal(3.0)
    };
    let frame = render(&scene, &camera, &Observation::reference(1));
    let px = frame.image.channel(0).pixels();
    // Column 7 sits 0.2 above the 0.1 sky; its neighbour stays at sky level.
    for y in 0..32 {
        assert!(
            (px[y * 32 + 7] - 0.3).abs() < 1e-6,
            "bad column y={y}: {}",
            px[y * 32 + 7]
        );
        assert!(
            (px[y * 32 + 6] - 0.1).abs() < 1e-6,
            "neighbour y={y}: {}",
            px[y * 32 + 6]
        );
    }
}

#[test]
fn exposure_scales_dark_current() {
    // Dark pedestal = dark_rate·exposure/full_well. With read noise off and an empty sky, the
    // mean pixel is that pedestal, so a 100× exposure scales it ~100×.
    let scene = Scene {
        size: Size2us::new(128, 128),
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.0 },
    };
    let camera = Camera {
        read_noise_e: 0.0,
        dark_current_e_per_s: 5.0,
        ..Camera::realistic(3.0)
    };
    let short = render(
        &scene,
        &camera,
        &Observation {
            exposure_s: 1.0,
            ..Observation::reference(5)
        },
    );
    let long = render(
        &scene,
        &camera,
        &Observation {
            exposure_s: 100.0,
            ..Observation::reference(5)
        },
    );
    let m_short = pixel_stats(short.image.channel(0).pixels()).mean;
    let m_long = pixel_stats(long.image.channel(0).pixels()).mean;
    // 5·100/50000 = 0.01 for the long exposure; 1e-4 for the short.
    assert!(
        (m_long - 0.01).abs() < 1e-3,
        "long-exposure dark mean {m_long}"
    );
    assert!(
        m_long > m_short * 50.0,
        "dark current must scale with exposure: {m_short} → {m_long}"
    );
}
