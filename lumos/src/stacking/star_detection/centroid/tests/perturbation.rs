//! [`Perturbation`]: what a fit test adds to its rendered stamp.

use imaginarium::Buffer2;

use crate::testing::synthetic::patterns;

/// What gets added to the rendered stamp before fitting.
#[derive(Debug)]
pub(crate) enum Perturbation {
    None,
    /// Index-based sawtooth — deterministic without an RNG, and correlated with pixel order
    /// rather than random, which is a different stress than [`Perturbation::Gaussian`].
    Sawtooth {
        amplitude: f32,
    },
    Gaussian {
        sigma: f32,
        seed: u64,
    },
}

impl Perturbation {
    pub(crate) fn apply(&self, pixels: &mut Buffer2<f32>) {
        match *self {
            Perturbation::None => {}
            Perturbation::Sawtooth { amplitude } => {
                for (i, p) in pixels.iter_mut().enumerate() {
                    *p += amplitude * ((i % 7) as f32 - 3.0) / 3.0;
                }
            }
            Perturbation::Gaussian { sigma, seed } => {
                patterns::add_gaussian_noise(pixels, sigma, seed);
            }
        }
    }
}
