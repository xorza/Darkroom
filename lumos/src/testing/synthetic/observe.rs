//! A single simulated exposure: map a [`Scene`] through a [`Camera`] and an [`Observation`]
//! geometry into a raw frame, capturing the per-frame ground truth a lumos stage is graded on.
//!
//! Render applies layers in the physical order light accumulates (the ccdproc CCD equation):
//! geometry → PSF + background (the clean signal) → flat → shot noise → dark current → bias →
//! read noise → defects → saturate. A noiseless [`Camera::ideal`] collapses this to the clean
//! image, so the *same* code path produces both the stimulus and its truth.

use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::stacking::registration::transform::Transform;
use crate::testing::TestRng;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::noise::{add_dark_current, add_read_noise, apply_shot_noise};
use crate::testing::synthetic::scene::Scene;
use glam::DVec2;
use imaginarium::Buffer2;

/// Geometry + exposure parameters for one frame.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Maps sky/reference coordinates → this frame's sensor coordinates.
    pub transform: Transform,
    /// Exposure time in seconds (scales dark current).
    pub exposure_s: f32,
    /// Per-frame PSF width scale (seeing jitter); 1.0 == nominal.
    pub seeing_scale: f32,
    /// Seed for this frame's noise streams.
    pub seed: u64,
}

impl Observation {
    /// A reference exposure: identity transform, 1 s, nominal seeing.
    pub fn reference(seed: u64) -> Self {
        Self {
            transform: Transform::identity(),
            exposure_s: 1.0,
            seeing_scale: 1.0,
            seed,
        }
    }
}

/// A source as it actually lands on the sensor (post-transform) — the truth a detector recovers.
#[derive(Debug, Clone, Copy)]
pub struct ObservedSource {
    pub pos: DVec2,
    pub flux: f32,
    pub fwhm: f32,
}

/// Ground truth captured alongside a rendered frame.
#[derive(Debug, Clone)]
pub struct FrameTruth {
    /// Noiseless, flat-fielded signal `(background + sources) × flat` — the detection target.
    pub clean: Buffer2<f32>,
    /// Sources as they land on the sensor (post-transform).
    pub sources: Vec<ObservedSource>,
}

/// A rendered frame plus its ground truth.
#[derive(Debug, Clone)]
pub struct SimFrame {
    pub image: LinearImage,
    pub truth: FrameTruth,
}

/// Render `scene` through `camera` for one `obs` into a grayscale [`SimFrame`].
pub fn render(scene: &Scene, camera: &Camera, obs: &Observation) -> SimFrame {
    let width = scene.width;
    let height = scene.height;

    // 1 + 2. Geometry + PSF + background → the clean (pre-flat) signal, and the truth catalog.
    let mut clean = scene.background.render(width, height);
    let mut observed = Vec::with_capacity(scene.sources.len());
    let recovered_fwhm = camera.psf.fwhm() * obs.seeing_scale;
    for src in &scene.sources {
        let p = obs.transform.apply(src.pos);
        camera.psf.render(
            &mut clean,
            width,
            p.x as f32,
            p.y as f32,
            src.flux,
            obs.seeing_scale,
        );
        observed.push(ObservedSource {
            pos: p,
            flux: src.flux,
            fwhm: recovered_fwhm,
        });
    }

    // 3 + 4. Flat field (multiplicative sensor response) → clean becomes the on-sensor signal.
    let flat = camera.flat.render(width, height, 0);
    for (c, f) in clean.iter_mut().zip(flat.iter()) {
        *c *= *f;
    }

    // The raw frame starts from the clean signal; sensor effects pile on from here.
    let mut raw = clean.clone();
    let mut rng = TestRng::new(obs.seed);

    // 5 + 6 + 8. Shot noise, dark current, read noise — skipped for a noiseless sensor.
    if !camera.noiseless {
        apply_shot_noise(&mut raw, camera.full_well_e, &mut rng);
        add_dark_current(
            &mut raw,
            camera.dark_current_e_per_s,
            obs.exposure_s,
            camera.full_well_e,
            &mut rng,
        );
        add_read_noise(&mut raw, camera.read_noise_e, camera.full_well_e, &mut rng);
    }

    // 7. Bias pedestal + bad columns (deterministic structure, always applied).
    if camera.bias.offset != 0.0 {
        for p in raw.iter_mut() {
            *p += camera.bias.offset;
        }
    }
    for &(col, excess) in &camera.bias.bad_columns {
        if col < width {
            for y in 0..height {
                raw[y * width + col] += excess;
            }
        }
    }

    // 9. Defects: dead pixels forced low, hot pixels spiked.
    for &(x, y) in &camera.defects.dead {
        if x < width && y < height {
            raw[y * width + x] = 0.0;
        }
    }
    for &(x, y, excess) in &camera.defects.hot {
        if x < width && y < height {
            raw[y * width + x] += excess;
        }
    }

    // 11. Saturate / clamp to the valid normalized range.
    for p in raw.iter_mut() {
        *p = p.clamp(0.0, camera.saturation);
    }

    let dims = ImageDimensions::new((width, height), 1);
    let mut image = LinearImage::from_planar_channels(dims, [raw]);
    image.metadata.image_type = Some("Light".to_string());
    image.metadata.exposure_time = Some(obs.exposure_s as f64);

    SimFrame {
        image,
        truth: FrameTruth {
            clean: Buffer2::new(width, height, clean),
            sources: observed,
        },
    }
}

/// Render one `scene` through `camera` as `dithers.len()` frames, each translated by its
/// dither offset and given an independent noise seed derived from `base_seed`.
pub fn observe_dithered(
    scene: &Scene,
    camera: &Camera,
    dithers: &[DVec2],
    exposure_s: f32,
    base_seed: u64,
) -> Vec<SimFrame> {
    dithers
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let obs = Observation {
                transform: Transform::translation(d),
                exposure_s,
                seeing_scale: 1.0,
                seed: base_seed.wrapping_add(i as u64 * 7919),
            };
            render(scene, camera, &obs)
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
            64,
            64,
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
            64,
            64,
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
            81,
            81,
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
            width: 64,
            height: 64,
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
            width: 128,
            height: 128,
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
            64,
            64,
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
            width: 32,
            height: 32,
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
            64,
            64,
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
            width: 32,
            height: 32,
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
            width: 128,
            height: 128,
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
}
