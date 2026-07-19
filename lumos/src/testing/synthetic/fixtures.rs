//! Forward-model field fixtures for benchmarks and integration tests.
//!
//! Populated star fields rendered through a realistic [`Camera`] and returned as a
//! [`SimFrame`] (sensor image + ground truth): benches take `frame.image` (or
//! `frame.image.channel(0)`); tests grade against `frame.truth`.

use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::observe::{Observation, SimFrame, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

/// A uniform-random field of `num_stars` bright, cleanly-detected stars over a modest sky —
/// the general-purpose populated field.
pub fn star_field(width: usize, height: usize, num_stars: usize, seed: u64) -> SimFrame {
    let scene = Scene::random_field(
        width,
        height,
        num_stars,
        (6.0, 16.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        seed,
    );
    render(
        &scene,
        &Camera::realistic(4.0),
        &Observation::reference(seed),
    )
}

/// A crowded central cluster of `num_stars` with heavy blending over a dark sky — for
/// deblend, labeling, and crowded-detection stress.
pub fn cluster_field(width: usize, height: usize, num_stars: usize, seed: u64) -> SimFrame {
    let scene = Scene::cluster(
        width,
        height,
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
    use crate::testing::synthetic::fixtures::*;
    use crate::testing::synthetic::metrics::pixel_stats;
    use imaginarium::Buffer2;

    fn region_sum(px: &Buffer2<f32>, x0: usize, y0: usize, size: usize) -> f64 {
        let mut s = 0.0;
        for y in y0..y0 + size {
            for x in x0..x0 + size {
                s += px[(x, y)] as f64;
            }
        }
        s
    }

    #[test]
    fn star_field_has_requested_sources_and_signal() {
        let frame = star_field(128, 128, 30, 1);
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
    }

    #[test]
    fn cluster_field_is_denser_at_center() {
        let frame = cluster_field(200, 200, 400, 2);
        assert_eq!(frame.truth.sources.len(), 400);
        let px = frame.image.channel(0);
        // Central 40×40 carries much more flux than a corner 40×40.
        let center = region_sum(px, 80, 80, 40);
        let corner = region_sum(px, 2, 2, 40);
        assert!(center > corner * 3.0, "center {center} corner {corner}");
    }
}
