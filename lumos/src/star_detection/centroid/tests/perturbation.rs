//! [`Perturbation`]: what a fit test adds to its rendered stamp.

use imaginarium::Buffer2;

use crate::internals::synthetic::patterns;

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
    /// The perturbation's root-mean-square size per pixel. The sawtooth cycles through
    /// `(k − 3)/3` for k = 0..7, whose mean square is 4/9.
    pub(crate) fn rms(&self) -> f32 {
        match *self {
            Perturbation::None => 0.0,
            Perturbation::Sawtooth { amplitude } => amplitude * 2.0 / 3.0,
            Perturbation::Gaussian { sigma, .. } => sigma,
        }
    }

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
