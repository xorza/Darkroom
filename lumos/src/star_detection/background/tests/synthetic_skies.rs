//! Background estimation on rendered skies under stars and camera noise, each saved as its input,
//! background and residual images for inspection.

use crate::internals::init_tracing;
use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::synthetic::backgrounds::{NebulaConfig, Vignette};
use crate::internals::synthetic::scene::BackgroundField;
use crate::internals::visual::{ToneMap, save};
use crate::math::statistics::median_mut;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::tests::Scenario;

/// Each sky against the truth it was rendered from, in three parts.
///
/// The estimate of the bare sky — no stars, no noise — differs from the truth only by what the
/// tile mesh cannot follow: nothing on a plane (to 8ε of the largest value), and on a curved sky
/// the smoothing of its curvature at the 64-px tile scale, pinned at its measured size (vignette
/// 0.04995, nebula 0.2006) so that a change in it shows.
///
/// Camera noise then moves the estimate from the bare sky's by the sampling scatter of each tile's
/// sky: the Pearson mode `2.5·median − 1.5·mean` of N Gaussian samples of spread σ scatters by
/// `√(6.25·π/2 + 2.25 − 7.5)·σ/√N` = 2.14σ/√N, with N = 1024 samples and σ the estimate's own map
/// (which carries the sky's spread across each tile as well as the noise). Five of those bound the
/// largest move.
///
/// The stars then move it once more, by what the clip about each tile's plane leaves of their
/// wings: pinned at its measured size on each sky.
#[test]
fn rendered_skies_are_recovered() {
    struct Case {
        name: &'static str,
        sky: BackgroundField,
        num_stars: usize,
        /// `None` on a plane, held to rounding.
        model_error: Option<f32>,
        star_move: f32,
    }

    init_tracing();
    let cases = [
        Case {
            name: "uniform",
            sky: BackgroundField::Uniform { level: 0.15 },
            num_stars: 30,
            model_error: None,
            star_move: 0.000_165,
        },
        Case {
            name: "gradient",
            sky: BackgroundField::Gradient {
                start: 0.05,
                end: 0.25,
                angle: 0.0,
            },
            num_stars: 30,
            model_error: None,
            star_move: 0.000_190,
        },
        Case {
            name: "vignette",
            sky: BackgroundField::Vignette(Vignette {
                center: 0.2,
                edge: 0.05,
                falloff: 2.0,
            }),
            num_stars: 30,
            model_error: Some(0.049_95),
            star_move: 0.000_421,
        },
        Case {
            name: "nebula",
            sky: BackgroundField::Nebula(NebulaConfig {
                center: Vec2::splat(0.5),
                radius: 0.3,
                amplitude: 0.3,
                softness: 2.0,
                aspect_ratio: 1.2,
                angle: 0.3,
            }),
            num_stars: 40,
            model_error: Some(0.2006),
            star_move: 0.005_03,
        },
    ];
    let config = BackgroundConfig::default();
    for case in cases {
        let pixels = Scenario {
            num_stars: case.num_stars,
            background: case.sky.clone(),
            ..Default::default()
        }
        .frame()
        .image
        .channel(0)
        .clone();
        let size = Size2us::new(pixels.width(), pixels.height());
        let truth = case.sky.render(size);
        let bare = background_map::estimate(
            &Buffer2::new(size.width, size.height, truth.clone()),
            &config,
        );
        let background = background_map::estimate(&pixels, &config);
        let noise_only = Scenario {
            num_stars: 0,
            background: case.sky.clone(),
            ..Default::default()
        }
        .frame()
        .image
        .channel(0)
        .clone();
        let noisy = background_map::estimate(&noise_only, &config);

        let residual: Vec<f32> = pixels
            .iter()
            .zip(background.background.iter())
            .map(|(&p, &sky)| (p - sky).max(0.0))
            .collect();
        for (suffix, image) in [
            ("input", pixels.pixels()),
            ("background", background.background.pixels()),
            ("subtracted", &residual[..]),
        ] {
            save(
                image,
                size,
                &format!("synthetic_starfield/stage_bg_{}_{suffix}", case.name),
                ToneMap::Clamp,
            );
        }

        let largest = |values: &mut dyn Iterator<Item = f32>| values.fold(0.0f32, f32::max);
        let model = largest(
            &mut bare
                .background
                .iter()
                .zip(&truth)
                .map(|(a, b)| (a - b).abs()),
        );
        let model_bound = case
            .model_error
            .unwrap_or(8.0 * f32::EPSILON * largest(&mut truth.iter().copied()));
        assert!(
            model <= model_bound,
            "{}: the bare sky's estimate is {model} off the truth, past {model_bound}",
            case.name
        );

        let mut spread = noisy.noise.pixels().to_vec();
        let sigma = median_mut(&mut spread);
        let scatter_bound = 5.0 * 2.14 * sigma / 32.0;
        let scatter = largest(
            &mut noisy
                .background
                .iter()
                .zip(bare.background.iter())
                .map(|(a, b)| (a - b).abs()),
        );
        assert!(
            scatter <= scatter_bound,
            "{}: noise moves the sky by {scatter}, past {scatter_bound}",
            case.name
        );

        let star_move = largest(
            &mut background
                .background
                .iter()
                .zip(noisy.background.iter())
                .map(|(a, b)| (a - b).abs()),
        );
        assert!(
            star_move <= case.star_move,
            "{}: the stars move the sky by {star_move}, past {}",
            case.name,
            case.star_move
        );
    }
}
