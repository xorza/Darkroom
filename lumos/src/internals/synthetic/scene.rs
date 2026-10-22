//! The true sky: a resolution-independent catalog of point sources plus an astrophysical
//! background, in reference-frame ("sky") coordinates.
//!
//! A `Scene` is ground truth. A [`Camera`](crate::internals::synthetic::camera::Camera) and
//! [`Observation`](crate::internals::synthetic::observe::Observation) render it forward into a
//! frame; a lumos stage runs backward and is graded against this truth.

use imaginarium::Buffer2;

use crate::internals::synthetic::bgs::{Gradient, NebulaConfig, Vignette, add_uniform_bg};
use crate::internals::test_rng::TestRng;
use crate::math::size2us::Size2us;
use glam::DVec2;

/// A true point source (star) in sky coordinates. Its on-sensor shape comes entirely from
/// the [`Camera`](crate::internals::synthetic::camera::Camera) PSF.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TrueSource {
    /// Position in sky / reference-frame pixel coordinates (sub-pixel).
    pub(crate) pos: DVec2,
    /// Total integrated flux in normalized units (1.0 == one full-well pixel of light).
    pub(crate) flux: f32,
}

/// Astrophysical background (sky glow / light pollution / nebulosity), additive in
/// normalized units. Delegates to the existing `bgs` generators.
#[derive(Debug, Clone)]
pub(crate) enum BgField {
    Uniform { level: f32 },
    Gradient(Gradient),
    Vignette(Vignette),
    Nebula(NebulaConfig),
}

impl BgField {
    /// Render this background into a fresh `size`-sized buffer of normalized values.
    pub(crate) fn render(&self, size: Size2us) -> Buffer2<f32> {
        let mut pixels = Buffer2::new_default(size.width, size.height);
        match self {
            Self::Uniform { level } => add_uniform_bg(&mut pixels, *level),
            Self::Gradient(gradient) => gradient.add_to(&mut pixels),
            Self::Vignette(vignette) => vignette.add_to(&mut pixels),
            Self::Nebula(cfg) => cfg.add_to(&mut pixels),
        }
        pixels
    }
}

/// Log-uniform flux sampler over a positive `(min, max)` range — a realistic brightness spread
/// rather than a flat one. Validates the range once and caches its logs.
#[derive(Debug, Clone, Copy)]
struct LogUniformFlux {
    log_min: f32,
    log_max: f32,
}

impl LogUniformFlux {
    fn new(range: (f32, f32)) -> Self {
        let (min, max) = range;
        assert!(min > 0.0 && max >= min, "invalid flux range");
        Self {
            log_min: min.ln(),
            log_max: max.ln(),
        }
    }

    fn sample(self, rng: &mut TestRng) -> f32 {
        (self.log_min + rng.next_f32() * (self.log_max - self.log_min)).exp()
    }
}

/// The true sky for a simulated observation session.
#[derive(Debug, Clone)]
pub(crate) struct Scene {
    /// Sky extent in reference-frame pixels; sources/background live within this.
    pub(crate) size: Size2us,
    pub(crate) sources: Vec<TrueSource>,
    pub(crate) bg: BgField,
}

/// Where [`Scene::scatter`] puts its sources.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Placement {
    /// Uniformly over the frame, `margin` pixels clear of every edge.
    Uniform { margin: f64 },
    /// Most sources packed into a central Gaussian core, the rest a sparse halo — a crowded
    /// cluster for deblend/labeling stress.
    Cluster,
}

/// A random population of sources: how many, how bright and where.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scatter {
    pub(crate) count: usize,
    /// Fluxes are drawn **log-uniformly** in this range: a realistic brightness spread, not a
    /// flat one.
    pub(crate) flux: (f32, f32),
    pub(crate) placement: Placement,
    pub(crate) seed: u64,
}

impl Scene {
    /// A single `source` over `bg`. Useful for precise photometric/astrometric
    /// verification.
    pub(crate) fn single(size: Size2us, source: TrueSource, bg: BgField) -> Self {
        Self {
            size,
            sources: vec![source],
            bg,
        }
    }

    /// The sources `scatter` draws over `bg`. Deterministic in its seed.
    #[expect(
        clippy::cast_sign_loss,
        reason = "synthetic fixtures are small images with non-negative coordinates"
    )]
    pub(crate) fn scatter(size: Size2us, scatter: Scatter, bg: BgField) -> Self {
        let Scatter {
            count,
            flux,
            placement,
            seed,
        } = scatter;
        let flux_dist = LogUniformFlux::new(flux);
        let mut rng = TestRng::new(seed);
        let sources = match placement {
            Placement::Uniform { margin } => (0..count)
                .map(|_| {
                    let x = margin + rng.next_f64() * (size.width as f64 - 2.0 * margin);
                    let y = margin + rng.next_f64() * (size.height as f64 - 2.0 * margin);
                    TrueSource {
                        pos: DVec2::new(x, y),
                        flux: flux_dist.sample(&mut rng),
                    }
                })
                .collect(),
            Placement::Cluster => {
                let (cx, cy) = (size.width as f64 / 2.0, size.height as f64 / 2.0);
                let core_sigma = size.width.min(size.height) as f64 * 0.12;
                let core_count = (count as f64 * 0.8) as usize;
                let margin = 4.0;

                let mut sources = Vec::with_capacity(count);
                for i in 0..count {
                    let pos = if i < core_count {
                        // Rejection-sample a central Gaussian back into bounds.
                        loop {
                            let x = cx + f64::from(rng.next_gaussian_f32()) * core_sigma;
                            let y = cy + f64::from(rng.next_gaussian_f32()) * core_sigma;
                            if x >= margin
                                && x < size.width as f64 - margin
                                && y >= margin
                                && y < size.height as f64 - margin
                            {
                                break DVec2::new(x, y);
                            }
                        }
                    } else {
                        DVec2::new(
                            margin + rng.next_f64() * (size.width as f64 - 2.0 * margin),
                            margin + rng.next_f64() * (size.height as f64 - 2.0 * margin),
                        )
                    };
                    sources.push(TrueSource {
                        pos,
                        flux: flux_dist.sample(&mut rng),
                    });
                }
                sources
            }
        };
        Self { size, sources, bg }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use crate::internals::synthetic::metrics::pixel_stats;
    use crate::internals::synthetic::scene::*;

    /// The scene renders each background through its generator, so a uniform level is that
    /// level exactly and a gradient reaches both end levels exactly.
    #[test]
    fn bgs_render_their_levels() {
        let buf = BgField::Uniform { level: 0.25 }.render(Size2us::new(32, 16));
        assert_eq!(buf.len(), 32 * 16);
        assert!(buf.iter().all(|&p| p == 0.25));

        let bg = BgField::Gradient(Gradient {
            start: 0.0,
            end: 1.0,
            angle: 0.0,
        });
        let buf = bg.render(Size2us::new(64, 4));
        assert_eq!((buf[0], buf[63], buf[64], buf[127]), (0.0, 1.0, 0.0, 1.0));
    }

    #[test]
    fn uniform_scatter_count_bounds_and_reproducibility() {
        let bg = BgField::Uniform { level: 0.05 };
        let a = Scene::scatter(
            Size2us::new(200, 200),
            Scatter {
                count: 50,
                flux: (1.0, 100.0),
                placement: Placement::Uniform { margin: 10.0 },
                seed: 42,
            },
            bg.clone(),
        );
        assert_eq!(a.sources.len(), 50);
        for s in &a.sources {
            assert!(s.pos.x >= 10.0 && s.pos.x <= 190.0);
            assert!(s.pos.y >= 10.0 && s.pos.y <= 190.0);
            assert!(s.flux >= 1.0 && s.flux <= 100.0);
        }
        // Same seed → identical scene.
        let b = Scene::scatter(
            Size2us::new(200, 200),
            Scatter {
                count: 50,
                flux: (1.0, 100.0),
                placement: Placement::Uniform { margin: 10.0 },
                seed: 42,
            },
            bg,
        );
        for (sa, sb) in a.sources.iter().zip(&b.sources) {
            assert_eq!(sa.pos, sb.pos);
            assert_eq!(sa.flux, sb.flux);
        }
    }

    #[test]
    fn single_source_scene() {
        let scene = Scene::single(
            Size2us::new(64, 64),
            TrueSource {
                pos: DVec2::new(32.0, 32.0),
                flux: 5.0,
            },
            BgField::Uniform { level: 0.0 },
        );
        assert_eq!(scene.sources.len(), 1);
        assert_eq!(scene.sources[0].pos, DVec2::new(32.0, 32.0));
        // Empty-sky background really is empty.
        assert_eq!(
            pixel_stats(&scene.bg.render(Size2us::new(64, 64))).mean,
            0.0
        );
    }

    #[test]
    fn cluster_is_centrally_concentrated() {
        let size = Size2us::new(400usize, 400usize);
        let scene = Scene::scatter(
            size,
            Scatter {
                count: 500,
                flux: (1.0, 10.0),
                placement: Placement::Cluster,
                seed: 7,
            },
            BgField::Uniform { level: 0.0 },
        );
        assert_eq!(scene.sources.len(), 500);

        // Count sources within a central disk of radius = 12% of the field (the core σ).
        let (cx, cy) = (size.width as f64 / 2.0, size.height as f64 / 2.0);
        let r = size.width as f64 * 0.12;
        let central = scene
            .sources
            .iter()
            .filter(|s| (s.pos.x - cx).hypot(s.pos.y - cy) < r)
            .count();
        // A uniform field would put only ~π r² / (size.width·size.height) ≈ 4.5% in that disk;
        // clustering must pack far more (the 80% core inside ~1σ ⇒ well over a third).
        let uniform_expectation = 500.0 * PI * r * r / (size.pixel_count()) as f64;
        assert!(
            central as f64 > uniform_expectation * 4.0 && central > 150,
            "central {central} vs uniform expectation {uniform_expectation:.1}"
        );
    }
}
