//! The true sky: a resolution-independent catalog of point sources plus an astrophysical
//! background, in reference-frame ("sky") coordinates.
//!
//! A `Scene` is ground truth. A [`Camera`](crate::testing::synthetic::camera::Camera) and
//! [`Observation`](crate::testing::synthetic::observe::Observation) render it forward into a
//! frame; a lumos stage runs backward and is graded against this truth.

use std::f64::consts::PI;

use crate::math::size2us::Size2us;
use crate::testing::TestRng;
use crate::testing::synthetic::backgrounds::{
    NebulaConfig, add_gradient_background, add_nebula_background, add_uniform_background,
    add_vignette_background,
};
use glam::DVec2;

/// A true point source (star) in sky coordinates. Its on-sensor shape comes entirely from
/// the [`Camera`](crate::testing::synthetic::camera::Camera) PSF.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TrueSource {
    /// Position in sky / reference-frame pixel coordinates (sub-pixel).
    pub(crate) pos: DVec2,
    /// Total integrated flux in normalized units (1.0 == one full-well pixel of light).
    pub(crate) flux: f32,
}

/// Astrophysical background (sky glow / light pollution / nebulosity), additive in
/// normalized units. Delegates to the existing `backgrounds` generators.
#[derive(Debug, Clone)]
pub(crate) enum BackgroundField {
    Uniform {
        level: f32,
    },
    Gradient {
        start: f32,
        end: f32,
        angle: f32,
    },
    Vignette {
        center: f32,
        edge: f32,
        falloff: f32,
    },
    Nebula(NebulaConfig),
}

impl BackgroundField {
    /// Render this background into a fresh `size`-sized buffer of normalized values.
    pub(crate) fn render(&self, size: Size2us) -> Vec<f32> {
        let mut pixels = vec![0.0f32; size.pixel_count()];
        match self {
            BackgroundField::Uniform { level } => add_uniform_background(&mut pixels, *level),
            BackgroundField::Gradient { start, end, angle } => {
                add_gradient_background(&mut pixels, size, *start, *end, *angle)
            }
            BackgroundField::Vignette {
                center,
                edge,
                falloff,
            } => add_vignette_background(&mut pixels, size, *center, *edge, *falloff),
            BackgroundField::Nebula(cfg) => add_nebula_background(&mut pixels, size, cfg),
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

    fn sample(&self, rng: &mut TestRng) -> f32 {
        (self.log_min + rng.next_f32() * (self.log_max - self.log_min)).exp()
    }
}

/// The true sky for a simulated observation session.
#[derive(Debug, Clone)]
pub(crate) struct Scene {
    /// Sky extent in reference-frame pixels; sources/background live within this.
    pub(crate) size: Size2us,
    pub(crate) sources: Vec<TrueSource>,
    pub(crate) background: BackgroundField,
}

impl Scene {
    /// A single source at `pos` with `flux` over `background`. Useful for precise
    /// photometric/astrometric verification.
    pub(crate) fn single(
        size: Size2us,
        pos: DVec2,
        flux: f32,
        background: BackgroundField,
    ) -> Self {
        Scene {
            size,
            sources: vec![TrueSource { pos, flux }],
            background,
        }
    }

    /// A uniformly-random field of `count` sources over `background`.
    ///
    /// Fluxes are drawn **log-uniformly** in `flux_range` (a realistic brightness spread, not a
    /// flat one); `margin` keeps sources off the edges. Deterministic in `seed`.
    pub(crate) fn random_field(
        size: Size2us,
        count: usize,
        flux_range: (f32, f32),
        background: BackgroundField,
        margin: f64,
        seed: u64,
    ) -> Self {
        let flux_dist = LogUniformFlux::new(flux_range);
        let mut rng = TestRng::new(seed);
        let mut sources = Vec::with_capacity(count);
        for _ in 0..count {
            let x = margin + rng.next_f64() * (size.width as f64 - 2.0 * margin);
            let y = margin + rng.next_f64() * (size.height as f64 - 2.0 * margin);
            sources.push(TrueSource {
                pos: DVec2::new(x, y),
                flux: flux_dist.sample(&mut rng),
            });
        }
        Scene {
            size,
            sources,
            background,
        }
    }

    /// A centrally-concentrated field: most sources packed into a Gaussian core, the rest a
    /// sparse halo — a crowded cluster for deblend/labeling stress. Fluxes are log-uniform in
    /// `flux_range`; deterministic in `seed`.
    pub(crate) fn cluster(
        size: Size2us,
        count: usize,
        flux_range: (f32, f32),
        background: BackgroundField,
        seed: u64,
    ) -> Self {
        let flux_dist = LogUniformFlux::new(flux_range);
        let mut rng = TestRng::new(seed);
        let (cx, cy) = (size.width as f64 / 2.0, size.height as f64 / 2.0);
        let core_sigma = size.width.min(size.height) as f64 * 0.12;
        let core_count = (count as f64 * 0.8) as usize;
        let margin = 4.0;

        let mut sources = Vec::with_capacity(count);
        for i in 0..count {
            let pos = if i < core_count {
                // Rejection-sample a central Gaussian back into bounds.
                loop {
                    let x = cx + rng.next_gaussian_f32() as f64 * core_sigma;
                    let y = cy + rng.next_gaussian_f32() as f64 * core_sigma;
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
        Scene {
            size,
            sources,
            background,
        }
    }

    /// True source positions as a catalog (for matching against detector output).
    fn positions(&self) -> Vec<DVec2> {
        self.sources.iter().map(|s| s.pos).collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::metrics::pixel_stats;
    use crate::testing::synthetic::scene::*;

    #[test]
    fn uniform_background_renders_constant() {
        let bg = BackgroundField::Uniform { level: 0.1 };
        let buf = bg.render(Size2us::new(32, 16));
        assert_eq!(buf.len(), 32 * 16);
        assert!(buf.iter().all(|&p| (p - 0.1).abs() < 1e-6));
    }

    #[test]
    fn gradient_background_spans_endpoints() {
        // Horizontal 0→1 gradient: left edge ≈ 0, right edge ≈ 1.
        let bg = BackgroundField::Gradient {
            start: 0.0,
            end: 1.0,
            angle: 0.0,
        };
        let buf = bg.render(Size2us::new(64, 4));
        assert!(buf[0] < 0.05, "left {}", buf[0]);
        assert!(buf[63] > 0.95, "right {}", buf[63]);
    }

    #[test]
    fn random_field_count_bounds_and_reproducibility() {
        let bg = BackgroundField::Uniform { level: 0.05 };
        let a = Scene::random_field(
            Size2us::new(200, 200),
            50,
            (1.0, 100.0),
            bg.clone(),
            10.0,
            42,
        );
        assert_eq!(a.sources.len(), 50);
        for s in &a.sources {
            assert!(s.pos.x >= 10.0 && s.pos.x <= 190.0);
            assert!(s.pos.y >= 10.0 && s.pos.y <= 190.0);
            assert!(s.flux >= 1.0 && s.flux <= 100.0);
        }
        // Same seed → identical scene.
        let b = Scene::random_field(Size2us::new(200, 200), 50, (1.0, 100.0), bg, 10.0, 42);
        for (sa, sb) in a.sources.iter().zip(&b.sources) {
            assert_eq!(sa.pos, sb.pos);
            assert_eq!(sa.flux, sb.flux);
        }
    }

    #[test]
    fn single_source_scene() {
        let scene = Scene::single(
            Size2us::new(64, 64),
            DVec2::new(32.0, 32.0),
            5.0,
            BackgroundField::Uniform { level: 0.0 },
        );
        assert_eq!(scene.sources.len(), 1);
        assert_eq!(scene.positions(), vec![DVec2::new(32.0, 32.0)]);
        // Empty-sky background really is empty.
        assert_eq!(
            pixel_stats(&scene.background.render(Size2us::new(64, 64))).mean,
            0.0
        );
    }

    #[test]
    fn cluster_is_centrally_concentrated() {
        let size = Size2us::new(400usize, 400usize);
        let scene = Scene::cluster(
            size,
            500,
            (1.0, 10.0),
            BackgroundField::Uniform { level: 0.0 },
            7,
        );
        assert_eq!(scene.sources.len(), 500);

        // Count sources within a central disk of radius = 12% of the field (the core σ).
        let (cx, cy) = (size.width as f64 / 2.0, size.height as f64 / 2.0);
        let r = size.width as f64 * 0.12;
        let central = scene
            .sources
            .iter()
            .filter(|s| ((s.pos.x - cx).powi(2) + (s.pos.y - cy).powi(2)).sqrt() < r)
            .count();
        // A uniform field would put only ~π r² / (size.width·size.height) ≈ 4.5% in that disk; clustering must
        // pack far more (the 80% core inside ~1σ ⇒ well over a third).
        let uniform_expectation = 500.0 * PI * r * r / (size.pixel_count()) as f64;
        assert!(
            central as f64 > uniform_expectation * 4.0 && central > 150,
            "central {central} vs uniform expectation {uniform_expectation:.1}"
        );
    }
}
