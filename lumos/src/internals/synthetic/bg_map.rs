//! `BgEstimate` generation for testing.
//!
//! Provides utilities to create `BgEstimate` instances for benchmarks and tests.

use common::Buffer2;

use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::scratch_pool::Pooled;
use crate::star_detection::bg::bg_estimate::BgEstimate;
use crate::star_detection::bg::sky_noise::{SkyNoise, noise_floor_for};
use crate::star_detection::detection_filter::Prefilter;
use crate::star_detection::detection_plane::PlaneSpec;
use crate::star_detection::refined_bg::RefinedBg;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::bg_config::BgConfig;
use common::USize2;

/// Create a uniform `BgEstimate` with constant background and noise values.
pub(crate) fn uniform(size: USize2, bg: f32, noise: f32) -> BgEstimate {
    BgEstimate {
        bg: Pooled::loose(MeshSurface::uniform(bg, size), ()),
        sky: SkyNoise {
            noise: Pooled::loose(MeshSurface::uniform(noise, size), ()),
            // The map is uniform, so the frame's typical σ is `noise` itself and the estimator's
            // median-tile-σ derivation reduces to exactly this.
            floor: noise_floor_for(noise),
        },
    }
}

/// Run the real background estimator over `pixels`, managing the buffer pool for the caller,
/// with `config`'s refinement when it asks for one, on the unfiltered residual.
///
/// The counterpart to [`uniform`]: that one hands back a flat map, this one measures the image.
pub(crate) fn estimate(pixels: &Buffer2<f32>, config: &BgConfig) -> BgEstimate {
    let mut pool = DetectionResources::new(pixels.size());
    estimate_in(pixels, config, &mut pool)
}

/// [`estimate`] with the caller's pool, so a bench or a reuse test can recycle its planes.
pub(crate) fn estimate_in(
    pixels: &Buffer2<f32>,
    config: &BgConfig,
    pool: &mut DetectionResources,
) -> BgEstimate {
    RefinedBg::measure(
        pixels,
        PlaneSpec {
            prefilter: Prefilter::None,
            matched: None,
            mask: None,
            bg: config,
        },
        pool,
    )
    .estimate
}

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::bg_map::*;

    /// Every pixel of a uniform estimate reads its two values exactly.
    #[test]
    fn uniform_fills_both_the_background_and_noise_planes() {
        let bg = uniform(USize2::new(100, 70), 0.1, 0.01);
        assert!(bg.bg.plane().pixels().iter().all(|&sky| sky == 0.1));
        assert!(
            bg.sky
                .noise
                .plane()
                .pixels()
                .iter()
                .all(|&sigma| sigma == 0.01)
        );
    }
}
