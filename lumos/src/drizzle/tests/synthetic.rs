//! Drizzle reconstruction on forward-model dithered frame sets.
//!
//! The rest of the drizzle tests cover the kernel geometry, the quality maps and pixel masks on
//! hand-built frames. These check the reconstruction on sub-pixel-dithered renders: total flux, a
//! source's position, the quality maps' closed form, and the resolution dithering recovers.

use super::*;
use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::observe::{Observation, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};

/// One rendered frame per dither offset, and the transform that registers each.
#[derive(Debug)]
struct DitheredFrames {
    images: Vec<LinearImage>,
    transforms: Vec<Transform>,
}

/// Render one sub-pixel-dithered frame per offset, with the drizzle transform that registers it
/// back onto the common grid: a frame whose star is dithered to `pos + d` uses `translation(−d)`.
fn dithered_frames(scene: &Scene, camera: &Camera, dithers: &[DVec2]) -> DitheredFrames {
    let images = dithers
        .iter()
        .map(|&d| {
            let obs = Observation {
                transform: Transform::translation(d),
                ..Observation::reference(0)
            };
            render(scene, camera, &obs).image
        })
        .collect();
    let transforms = dithers
        .iter()
        .map(|&d| Transform::translation(-d))
        .collect();
    DitheredFrames { images, transforms }
}

/// The four half-pixel dithers. At scale 2 and pixfrac 0.8, and at scale 1 and pixfrac 1, they
/// leave every interior output cell the same weight from every frame: each input pixel's drop
/// splits evenly over a fixed block of cells, so the drizzle is a plain average of block-replicated
/// frames.
const HALF_PIXEL_DITHERS: [DVec2; 4] = [
    DVec2::ZERO,
    DVec2::new(0.5, 0.0),
    DVec2::new(0.0, 0.5),
    DVec2::new(0.5, 0.5),
];

fn sum(px: &[f32]) -> f64 {
    px.iter().map(|&v| f64::from(v)).sum()
}

fn peak(px: &[f32]) -> f32 {
    px.iter().copied().fold(f32::MIN, f32::max)
}

/// Flux-weighted centroid of a star on a zero background.
fn star_centroid(image: &Buffer2<f32>) -> DVec2 {
    let mut s = 0.0;
    let mut sx = 0.0;
    let mut sy = 0.0;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let v = f64::from(image[(x, y)]);
            s += v;
            sx += v * x as f64;
            sy += v * y as f64;
        }
    }
    DVec2::new(sx / s, sy / s)
}

fn drizzle(frames: &DitheredFrames, config: &DrizzleConfig) -> StackProduct {
    drizzle_images(
        drizzle_frames(frames.images.clone(), &frames.transforms),
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .product
}

/// Drizzle keeps surface brightness, so the output holds `s²` times an input frame's flux: with
/// the half-pixel dithers every input pixel's drop splits evenly over `s²` cells, and every frame
/// weighs the same at every cell, so the output sums to `s²` times the frames' mean flux. The star
/// sits far from the border, where frames reach unevenly. A cell sums at most 16 deposits, a
/// relative 33·ε, and the totals are taken in f64.
#[test]
fn drizzle_conserves_total_flux() {
    let size = Size2us::new(64, 64);
    let scene = Scene::single(
        size,
        DVec2::new(32.0, 32.0),
        5.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let frames = dithered_frames(&scene, &Camera::ideal(3.5), &HALF_PIXEL_DITHERS);
    let mean_flux = frames
        .images
        .iter()
        .map(|image| sum(image.channel(0).pixels()))
        .sum::<f64>()
        / frames.images.len() as f64;

    for (scale, pixfrac) in [(1.0f32, 1.0), (2.0, 0.8)] {
        let product = drizzle(
            &frames,
            &kernel_config(DrizzleKernel::Turbo, scale, pixfrac),
        );
        let out_flux = sum(product.image.channel(0).pixels());
        let expected = mean_flux * f64::from(scale * scale);
        assert!(
            (out_flux - expected).abs() <= 33.0 * f64::from(f32::EPSILON) * expected,
            "scale {scale}: Σ_out {out_flux} against s²·Σ_in {expected}"
        );
    }
}

/// A star lands at its reference position on the output grid, `s·p + (s − 1)/2`, measured against
/// the centroid of the undithered frame itself so the PSF's pixel sampling cancels. With the
/// half-pixel dithers each frame's pixel fills its own 2×2 block of cells about its drop's centre,
/// so the output's centroid is the drop centres' — to the f32 rounding of each value, a relative
/// 9·ε, at offsets up to the 128-pixel width, twice over for the input's centroid.
#[test]
fn drizzle_places_star_at_its_reference_position() {
    let size = Size2us::new(64, 64);
    let scene = Scene::single(
        size,
        DVec2::new(28.0, 36.0),
        5.0,
        BackgroundField::Uniform { level: 0.0 },
    );
    let frames = dithered_frames(&scene, &Camera::ideal(3.5), &HALF_PIXEL_DITHERS);
    let scale = 2.0;
    let product = drizzle(
        &frames,
        &kernel_config(DrizzleKernel::Turbo, scale as f32, 0.8),
    );
    let truth =
        scale * star_centroid(frames.images[0].channel(0)) + DVec2::splat((scale - 1.0) / 2.0);
    let center = star_centroid(product.image.channel(0));
    let bound = 2.0 * 9.0 * f64::from(f32::EPSILON) * 128.0;
    assert!(
        (center - truth).abs().max_element() <= bound,
        "centroid {center:?} against {truth:?}"
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
    let offs = [-1.0 / 3.0, 0.0, 1.0 / 3.0];
    let dithers: Vec<DVec2> = offs
        .iter()
        .flat_map(|&dx| offs.iter().map(move |&dy| DVec2::new(dx, dy)))
        .collect();
    let frames = dithered_frames(&scene, &Camera::ideal(1.8), &dithers);
    let config = kernel_config(DrizzleKernel::Turbo, 2.0, 0.6);

    // Distinct sub-pixel dithers vs the same single frame replicated N times: same frame count and
    // flux, so the only difference is sub-pixel diversity. Recovering it sharpens the peak.
    let multi = drizzle(&frames, &config);
    let replicated = drizzle(
        &DitheredFrames {
            images: vec![frames.images[4].clone(); dithers.len()],
            transforms: vec![frames.transforms[4]; dithers.len()],
        },
        &config,
    );
    let multi_peak = peak(multi.image.channel(0).pixels());
    let single_peak = peak(replicated.image.channel(0).pixels());
    assert!(
        multi_peak > single_peak,
        "dithered reconstruction should recover a higher peak: {multi_peak:.4} vs single {single_peak:.4}"
    );
}

/// The quality maps' closed form for the half-pixel dithers at scale 1 and pixfrac 1, where a drop
/// is one output pixel. The undithered frame puts one whole drop on a pixel, weight 1; a frame
/// dithered half a pixel along one axis puts two half drops, `½ + ½`; along both, four quarters.
/// So `Σw` = 4, `Σw²` = 1 + 2·(¼ + ¼) + 4·1/16 = 2.25, and with unit sample variance the variance
/// is 2.25/16 = 9/64 — below the 1/4 of four whole drops, the pooling of neighbouring pixels that
/// is the point of the variance plane. The frames are noiseless, and a quantization σ of 1 is what
/// gives each sample unit variance. Every frame reaches every pixel. All exact. The last row and
/// column are left out: the half-dithered frames reach them with half a drop.
#[test]
fn drizzle_quality_maps_have_their_closed_form() {
    let size = Size2us::new(64, 64);
    let scene = Scene::single(
        size,
        DVec2::new(32.0, 32.0),
        5.0,
        BackgroundField::Uniform { level: 0.1 },
    );
    let mut frames = dithered_frames(&scene, &Camera::ideal(3.5), &HALF_PIXEL_DITHERS);
    for image in &mut frames.images {
        image.metadata.quantization_sigma = Some(1.0);
    }
    let product = drizzle(&frames, &kernel_config(DrizzleKernel::Turbo, 1.0, 1.0));
    let coverage = product.coverage.as_ref().unwrap();
    let weight = weight_plane(&product);
    let variance = product.variance.as_ref().unwrap().channel(0);
    for y in 0..63 {
        for x in 0..63 {
            assert_eq!(
                (coverage[(x, y)], weight[(x, y)], variance[(x, y)]),
                (1.0, 4.0, 9.0 / 64.0),
                "({x}, {y}): coverage, weight, variance"
            );
        }
    }
}
