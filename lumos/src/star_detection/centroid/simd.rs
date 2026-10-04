//! The batch kernels every profile fit shares: the L-M normal equations and χ² over a stamp, four
//! samples a vector, weighted or not.
//!
//! A model takes part by [`BatchModel`]: at fixed parameters it gives each Isa a [`LaneProfile`],
//! which evaluates the residual and the Jacobian row at four samples. Both kernels take the
//! residual from that one evaluation, so the χ² the step test reads and the χ² the normal
//! equations carry agree; the χ² kernel leaves the Jacobian unused, and the compiler drops it.
//!
//! An unweighted fit is a fit at unit weight, without the multiplies: `fma(1·j, j, h)` and
//! `fma(j, j, h)` are one value. The last, partial vector pads its samples with zeros and its
//! weights with zeros, so the padding lanes add exact zeros wherever the model is finite there.

use crate::math::lm_controller::NormalEquations;
use crate::simd::{F64_LANES, F64x4, Isa, Kernel};
use crate::star_detection::centroid::lm_optimizer::FitData;

/// One vector of a stamp's samples: positions and values.
#[derive(Debug, Clone, Copy)]
pub(super) struct Lanes<V> {
    pub(super) x: V,
    pub(super) y: V,
    pub(super) z: V,
}

/// The model at one vector of samples: the residual `z − f`, and the whole Jacobian row `∂f/∂p`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sample<V, const N: usize> {
    pub(super) residual: V,
    pub(super) jacobian: [V; N],
}

/// A model of `N` parameters at fixed values, as the batch kernels evaluate it.
pub(super) trait BatchModel<const N: usize>: Copy {
    /// The model's per-step constants, splat across one Isa's lanes.
    type Profile<S: Isa>: LaneProfile<S, N>;

    fn profile<S: Isa>(self, isa: S) -> Self::Profile<S>;
}

/// A model evaluated lane by lane on one Isa.
pub(super) trait LaneProfile<S: Isa, const N: usize>: Copy {
    /// The residual and the Jacobian row at four samples. Every implementation is
    /// `#[inline(always)]`.
    fn sample(self, isa: S, lanes: Lanes<S::F64>) -> Sample<S::F64, N>;
}

/// The normal equations for one Levenberg-Marquardt step over the whole stamp.
#[derive(Debug)]
pub(super) struct NormalEquationsKernel<'a, M, const N: usize> {
    pub(super) model: M,
    pub(super) data: FitData<'a>,
}

impl<M: BatchModel<N>, const N: usize> Kernel for NormalEquationsKernel<'_, M, N> {
    type Output = NormalEquations<N>;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> NormalEquations<N> {
        let zero = isa.splat_f64(0.0);
        let mut sums = NormalSums {
            profile: self.model.profile(isa),
            chi2: zero,
            gradient: [zero; N],
            hessian: [[zero; N]; N],
        };
        sums.feed(isa, self.data);
        sums.finish()
    }
}

/// The (weighted) residual sum of squares over the whole stamp.
#[derive(Debug)]
pub(super) struct Chi2Kernel<'a, M, const N: usize> {
    pub(super) model: M,
    pub(super) data: FitData<'a>,
}

impl<M: BatchModel<N>, const N: usize> Kernel for Chi2Kernel<'_, M, N> {
    type Output = f64;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> f64 {
        let mut sum = Chi2Sum::<_, _, N> {
            profile: self.model.profile(isa),
            chi2: isa.splat_f64(0.0),
        };
        sum.feed(isa, self.data);
        sum.chi2.reduce_sum()
    }
}

/// A kernel's per-vector step, fed every sample of a stamp by [`LaneSink::feed`].
trait LaneSink<S: Isa> {
    /// Take one vector of samples at `weight` per lane; `None` is unit weight.
    fn sample(&mut self, isa: S, lanes: Lanes<S::F64>, weight: Option<S::F64>);

    /// Every sample of `data`, [`F64_LANES`] at a time, the last, partial vector zero-padded.
    #[inline(always)]
    fn feed(&mut self, isa: S, data: FitData<'_>) {
        let (x, x_tail) = data.x.as_chunks::<F64_LANES>();
        let (y, y_tail) = data.y.as_chunks::<F64_LANES>();
        let (z, z_tail) = data.z.as_chunks::<F64_LANES>();
        let lanes = x.iter().zip(y).zip(z);
        match data.weights {
            None => {
                for ((x, y), z) in lanes {
                    self.sample(isa, load(isa, x, y, z), None);
                }
            }
            Some(weights) => {
                for (((x, y), z), w) in lanes.zip(weights.as_chunks::<F64_LANES>().0) {
                    self.sample(isa, load(isa, x, y, z), Some(isa.load_f64(w)));
                }
            }
        }
        if x_tail.is_empty() {
            return;
        }
        let lanes = Lanes {
            x: isa.load_f64_partial(x_tail),
            y: isa.load_f64_partial(y_tail),
            z: isa.load_f64_partial(z_tail),
        };
        let ones = [1.0; F64_LANES];
        let weights = match data.weights {
            None => &ones[..x_tail.len()],
            Some(weights) => weights.as_chunks::<F64_LANES>().1,
        };
        self.sample(isa, lanes, Some(isa.load_f64_partial(weights)));
    }
}

#[inline(always)]
fn load<S: Isa>(
    isa: S,
    x: &[f64; F64_LANES],
    y: &[f64; F64_LANES],
    z: &[f64; F64_LANES],
) -> Lanes<S::F64> {
    Lanes {
        x: isa.load_f64(x),
        y: isa.load_f64(y),
        z: isa.load_f64(z),
    }
}

/// χ² alone: `Σ w·r²`.
#[derive(Debug)]
struct Chi2Sum<P, V, const N: usize> {
    profile: P,
    chi2: V,
}

impl<S: Isa, P: LaneProfile<S, N>, const N: usize> LaneSink<S> for Chi2Sum<P, S::F64, N> {
    #[inline(always)]
    fn sample(&mut self, isa: S, lanes: Lanes<S::F64>, weight: Option<S::F64>) {
        let residual = self.profile.sample(isa, lanes).residual;
        let weighted = match weight {
            Some(w) => w * residual,
            None => residual,
        };
        self.chi2 = weighted.mul_add(residual, self.chi2);
    }
}

/// The normal equations' running sums, lane by lane: χ², the gradient `Σ w·j·r`, and the upper
/// triangle of the Hessian `Σ w·jᵀj`.
#[derive(Debug)]
struct NormalSums<P, V, const N: usize> {
    profile: P,
    chi2: V,
    gradient: [V; N],
    hessian: [[V; N]; N],
}

impl<P, V: F64x4, const N: usize> NormalSums<P, V, N> {
    /// The lanes folded, and the Hessian's lower triangle mirrored from the upper.
    #[inline(always)]
    fn finish(self) -> NormalEquations<N> {
        let mut equations = NormalEquations {
            hessian: [[0.0; N]; N],
            gradient: [0.0; N],
            chi2: self.chi2.reduce_sum(),
        };
        for i in 0..N {
            equations.gradient[i] = self.gradient[i].reduce_sum();
            for k in i..N {
                equations.hessian[i][k] = self.hessian[i][k].reduce_sum();
            }
        }
        equations.mirror_lower_triangle();
        equations
    }
}

impl<S: Isa, P: LaneProfile<S, N>, const N: usize> LaneSink<S> for NormalSums<P, S::F64, N> {
    #[inline(always)]
    fn sample(&mut self, isa: S, lanes: Lanes<S::F64>, weight: Option<S::F64>) {
        let Sample { residual, jacobian } = self.profile.sample(isa, lanes);
        let mut weighted = jacobian;
        let mut weighted_residual = residual;
        if let Some(w) = weight {
            for j in &mut weighted {
                *j = w * *j;
            }
            weighted_residual = w * residual;
        }
        self.chi2 = weighted_residual.mul_add(residual, self.chi2);
        for (gradient, &wj) in self.gradient.iter_mut().zip(&weighted) {
            *gradient = wj.mul_add(residual, *gradient);
        }
        for (i, (row, &wj)) in self.hessian.iter_mut().zip(&weighted).enumerate() {
            for (entry, &j) in row[i..].iter_mut().zip(&jacobian[i..]) {
                *entry = wj.mul_add(j, *entry);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::lm_controller::NormalEquations;
    use crate::simd::tier::Tier;
    use crate::star_detection::centroid::lm_optimizer::FitData;
    use crate::star_detection::centroid::simd::{BatchModel, Chi2Kernel, NormalEquationsKernel};

    /// Every entry's bits, Hessian then gradient then χ².
    fn bits<const N: usize>(equations: &NormalEquations<N>) -> Vec<u64> {
        equations
            .hessian
            .as_flattened()
            .iter()
            .chain(&equations.gradient)
            .chain([&equations.chi2])
            .map(|value| value.to_bits())
            .collect()
    }

    /// `model` over the first 9 to 16 samples of `stamp`, which leave every remainder of a
    /// vector, unweighted and at varied weights: every tier builds `Portable`'s normal equations
    /// and χ² bit for bit, the χ² kernel equals the equations' own, and unit weights reproduce the
    /// unweighted equations bit for bit.
    pub(crate) fn assert_every_tier_matches_portable<M: BatchModel<N>, const N: usize>(
        model: M,
        stamp: FitData<'_>,
    ) {
        assert!(stamp.x.len() >= 16, "the sweep reads 16 samples");
        let ones = [1.0; 16];
        let weights: Vec<f64> = (0..16).map(|i| 0.5 + f64::from(i % 5) * 0.37).collect();
        for n in 9..=16 {
            let portable = |data| Tier::portable().run(NormalEquationsKernel { model, data });
            assert_eq!(
                bits(&portable(prefix(stamp, n, Some(&ones)))),
                bits(&portable(prefix(stamp, n, None))),
                "unit weights, n={n}"
            );
            for data in [prefix(stamp, n, None), prefix(stamp, n, Some(&weights))] {
                let equations = portable(data);
                let chi2 = Tier::portable().run(Chi2Kernel { model, data });
                assert_eq!(chi2.to_bits(), equations.chi2.to_bits(), "n={n}");
                for tier in Tier::supported() {
                    assert_eq!(
                        bits(&tier.run(NormalEquationsKernel { model, data })),
                        bits(&equations),
                        "{tier} n={n} weighted={}",
                        data.weights.is_some()
                    );
                    assert_eq!(
                        tier.run(Chi2Kernel { model, data }).to_bits(),
                        chi2.to_bits(),
                        "{tier} n={n} weighted={}",
                        data.weights.is_some()
                    );
                }
            }
        }
    }

    /// The first `n` samples of `stamp`, at the first `n` of `weights`.
    fn prefix<'a>(stamp: FitData<'a>, n: usize, weights: Option<&'a [f64]>) -> FitData<'a> {
        FitData::new(
            &stamp.x[..n],
            &stamp.y[..n],
            &stamp.z[..n],
            weights.map(|w| &w[..n]),
        )
    }
}
