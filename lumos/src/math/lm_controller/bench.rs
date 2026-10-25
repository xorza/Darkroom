//! One Levenberg–Marquardt fit of a star's stamp, from a start a pixel and a third of its width
//! off.

use std::hint::black_box;

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture;
use crate::math::lm_controller::{LmController, LmProblem, NormalEquations};

const STAMP: usize = 15;
/// Amplitude, centre x and y, σ and background.
const TRUTH: [f64; 5] = [1.0, 7.3, 6.8, 1.6, 0.1];
const START: [f64; 5] = [0.8, 7.0, 7.4, 2.1, 0.0];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/lm_controller", Scale::Micros);
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("fit/gaussian", STAMP * STAMP), |b| {
        let stamp = Stamp::new();
        assert!(
            LmController::STANDARD.fit(&stamp, START).is_some(),
            "the bench times a fit that converges"
        );
        b.iter(|| LmController::STANDARD.fit(black_box(&stamp), START));
    });
    group.finish();
}

/// A round Gaussian on a flat background, sampled at pixel centres with noise of σ 0.01.
#[derive(Debug)]
struct Stamp {
    pixels: Vec<f64>,
}

impl Stamp {
    fn new() -> Self {
        let noise = fixture::gaussian(STAMP * STAMP, 13);
        let pixels = (0..STAMP * STAMP)
            .map(|idx| {
                let terms = Self::terms(&TRUTH, idx);
                terms.model + 0.01 * f64::from(noise[idx])
            })
            .collect();
        Self { pixels }
    }

    /// The model at pixel `idx` and its derivatives by each parameter.
    fn terms(params: &[f64; 5], idx: usize) -> Terms {
        let [amplitude, cx, cy, sigma, bg] = *params;
        let (dx, dy) = ((idx % STAMP) as f64 - cx, (idx / STAMP) as f64 - cy);
        let r2 = dx * dx + dy * dy;
        let s2 = sigma * sigma;
        let profile = (-r2 / (2.0 * s2)).exp();
        let peak = amplitude * profile;
        Terms {
            model: peak + bg,
            jacobian: [
                profile,
                peak * dx / s2,
                peak * dy / s2,
                peak * r2 / (s2 * sigma),
                1.0,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Terms {
    model: f64,
    jacobian: [f64; 5],
}

impl LmProblem<5> for Stamp {
    fn normal_equations(&self, params: &[f64; 5]) -> NormalEquations<5> {
        let mut equations = NormalEquations {
            hessian: [[0.0; 5]; 5],
            gradient: [0.0; 5],
            chi2: 0.0,
        };
        for (idx, &pixel) in self.pixels.iter().enumerate() {
            let Terms { model, jacobian } = Self::terms(params, idx);
            let residual = pixel - model;
            for i in 0..5 {
                equations.gradient[i] += jacobian[i] * residual;
                for k in i..5 {
                    equations.hessian[i][k] += jacobian[i] * jacobian[k];
                }
            }
            equations.chi2 += residual * residual;
        }
        equations.mirror_lower_triangle();
        equations
    }

    fn chi2(&self, params: &[f64; 5]) -> f64 {
        self.pixels
            .iter()
            .enumerate()
            .map(|(idx, &pixel)| (pixel - Self::terms(params, idx).model).powi(2))
            .sum()
    }

    fn constrain(&self, params: &mut [f64; 5]) {
        params[3] = params[3].max(0.1);
    }
}
