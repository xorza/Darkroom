//! A single simulated exposure: map a [`Scene`] through a [`Camera`] and an [`Observation`]
//! geometry into a raw frame, capturing the per-frame ground truth a lumos stage is graded on.
//!
//! Render applies layers in the physical order light accumulates (the ccdproc CCD equation):
//! geometry → PSF + background (the clean signal) → flat → shot noise → dark current → bias →
//! read noise → defects → saturate. A noiseless [`Camera::ideal`] collapses this to the clean
//! image, so the *same* code path produces both the stimulus and its truth.

use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::stacking::registration::transform::Transform;
use crate::testing::TestRng;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::noise::{add_dark_current, add_read_noise, apply_shot_noise};
use crate::testing::synthetic::scene::Scene;
use glam::DVec2;
use imaginarium::Buffer2;

/// Geometry + exposure parameters for one frame.
#[derive(Debug, Clone)]
pub(crate) struct Observation {
    /// Maps sky/reference coordinates → this frame's sensor coordinates.
    pub(crate) transform: Transform,
    /// Exposure time in seconds (scales dark current).
    pub(crate) exposure_s: f32,
    /// Per-frame PSF width scale (seeing jitter); 1.0 == nominal.
    pub(crate) seeing_scale: f32,
    /// Seed for this frame's noise streams.
    pub(crate) seed: u64,
}

impl Observation {
    /// A reference exposure: identity transform, 1 s, nominal seeing.
    pub(crate) fn reference(seed: u64) -> Self {
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
pub(crate) struct ObservedSource {
    pub(crate) pos: DVec2,
    pub(crate) flux: f32,
    pub(crate) fwhm: f32,
}

/// Ground truth captured alongside a rendered frame.
#[derive(Debug, Clone)]
pub(crate) struct FrameTruth {
    /// Noiseless, flat-fielded signal `(background + sources) × flat` — the detection target.
    pub(crate) clean: Buffer2<f32>,
    /// Sources as they land on the sensor (post-transform).
    pub(crate) sources: Vec<ObservedSource>,
}

/// A rendered frame plus its ground truth.
#[derive(Debug, Clone)]
pub(crate) struct SimFrame {
    pub(crate) image: LinearImage,
    pub(crate) truth: FrameTruth,
}

/// Render `scene` through `camera` for one `obs` into a grayscale [`SimFrame`].
pub(crate) fn render(scene: &Scene, camera: &Camera, obs: &Observation) -> SimFrame {
    let width = scene.size.width;
    let height = scene.size.height;

    // 1 + 2. Geometry + PSF + background → the clean (pre-flat) signal, and the truth catalog.
    let mut clean = scene.background.render(scene.size);
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
    let flat = camera.flat.render(scene.size, 0);
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
pub(super) fn observe_dithered(
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
mod tests;
