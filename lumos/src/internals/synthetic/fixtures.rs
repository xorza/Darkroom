//! Forward-model field fixtures for benchmarks and integration tests.
//!
//! Populated star fields rendered through a realistic [`Camera`] and returned as a
//! [`SimFrame`] (sensor image + ground truth): benches take `frame.image` (or
//! `frame.image.channel(0)`); tests grade against `frame.truth`.

use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::observe::{Observation, SimFrame, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::math::size2us::Size2us;

/// The total-flux range of [`star_field`]'s stars, log-uniform: bright, and clear of saturation on
/// the 0.1 sky at the 4-px FWHM.
pub(crate) const STAR_FIELD_FLUX: (f32, f32) = (5.0, 14.0);

/// [`star_field`]'s flat sky.
pub(crate) const STAR_FIELD_SKY: f32 = 0.1;

/// How far [`star_field`] keeps its stars from the frame's edge, in pixels.
pub(crate) const STAR_FIELD_MARGIN: f64 = 16.0;

/// [`star_field`]'s PSF FWHM, in pixels.
pub(crate) const STAR_FIELD_FWHM: f32 = 4.0;

/// A uniform-random field of `num_stars` bright, cleanly-detected stars over a modest sky —
/// the general-purpose populated field.
pub(crate) fn star_field(size: Size2us, num_stars: usize, seed: u64) -> SimFrame {
    let scene = Scene::random_field(
        size,
        num_stars,
        STAR_FIELD_FLUX,
        BackgroundField::Uniform {
            level: STAR_FIELD_SKY,
        },
        STAR_FIELD_MARGIN,
        seed,
    );
    render(
        &scene,
        &Camera::realistic(STAR_FIELD_FWHM),
        &Observation::reference(seed),
    )
}

/// A crowded central cluster of `num_stars` with heavy blending over a dark sky — for
/// deblend, labeling, and crowded-detection stress.
pub(crate) fn cluster_field(size: Size2us, num_stars: usize, seed: u64) -> SimFrame {
    let scene = Scene::cluster(
        size,
        num_stars,
        (5.0, 20.0),
        BackgroundField::Uniform { level: 0.05 },
        seed,
    );
    render(
        &scene,
        &Camera::realistic(3.5),
        &Observation::reference(seed),
    )
}

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::fixtures::*;
    use crate::internals::synthetic::metrics::pixel_stats;
    use imaginarium::Buffer2;

    fn region_sum(px: &Buffer2<f32>, x0: usize, y0: usize, size: usize) -> f64 {
        let mut s = 0.0;
        for y in y0..y0 + size {
            for x in x0..x0 + size {
                s += f64::from(px[(x, y)]);
            }
        }
        s
    }

    #[test]
    fn star_field_has_requested_sources_and_signal() {
        let frame = star_field(Size2us::new(128, 128), 30, 1);
        assert_eq!(frame.truth.sources.len(), 30);
        assert_eq!(frame.image.channel(0).pixels().len(), 128 * 128);
        // Bright stars on a 0.1 sky: mean above background, a clear peak.
        let s = pixel_stats(frame.image.channel(0).pixels());
        assert!(s.mean > 0.1, "mean {}", s.mean);
        let peak = frame
            .image
            .channel(0)
            .pixels()
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        assert!(peak > 0.3, "peak {peak}");
        assert!(peak < 0.95, "the brightest star saturates: {peak}");
    }

    #[test]
    fn cluster_field_is_denser_at_center() {
        let frame = cluster_field(Size2us::new(200, 200), 400, 2);
        assert_eq!(frame.truth.sources.len(), 400);
        let px = frame.image.channel(0);
        // Central 40×40 carries much more flux than a corner 40×40.
        let center = region_sum(px, 80, 80, 40);
        let corner = region_sum(px, 2, 2, 40);
        assert!(center > corner * 3.0, "center {center} corner {corner}");
    }
}
