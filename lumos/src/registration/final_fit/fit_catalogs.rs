//! [`FitCatalogs`]: the catalogs as the final fit reads them, and the passes that fit a model
//! over them.

use common::Libm;
use glam::DVec2;

use crate::math::statistics;
use crate::math::statistics::CHI2_99_2DOF;
use crate::registration::final_fit::weighted_pairs::WeightedPairs;
use crate::registration::final_fit::{FinalFit, FinalFitFailure, FitModel};
use crate::registration::kd_tree::KdTree;
use crate::registration::match_idxs::MatchIdxs;
use crate::registration::registration_result::{RegistrationResult, StarMatch};
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::star_detection::star::Star;

/// The median of a Rayleigh distribution of unit σ, `√(2 ln 2)`: what the median of 2-D residual
/// lengths reads, in units of their per-axis σ, when the noise is what the position σ claims.
const RAYLEIGH_MEDIAN: f64 = 1.177_410_022_515_474_7;

/// Holland and Welsch's (1977) tuning constant for the Cauchy loss, 95% efficient at the normal.
pub(super) const CAUCHY_TUNING: f64 = 2.3849;

/// The passes the fit may take. Each refit moves the weights and the gate by less than the last;
/// the cap only stops a fit whose matched set keeps trading one marginal pair.
const MAX_PASSES: usize = 50;

/// Two passes whose predictions differ by less than this, in pixels, are the same fit: a centroid
/// is good to a thousandth of a pixel at best.
const CONVERGED_PX: f64 = 1e-9;

/// The halvings the excess variance's bisection takes: past 64 its bracket is narrower than the
/// rounding of any variance it bounds.
const EXCESS_BISECTIONS: usize = 64;

/// The catalogs as the final fit reads them: each star's position and positional variance, and the
/// usable target stars in a tree.
#[derive(Debug)]
pub(crate) struct FitCatalogs {
    reference: Vec<DVec2>,
    reference_variance: Vec<f64>,
    /// The reference stars that take part: the unsaturated ones.
    reference_usable: Vec<bool>,
    target: Vec<DVec2>,
    target_variance: Vec<f64>,
    /// The unsaturated target stars.
    target_tree: KdTree,
    /// For each point of `target_tree`, its index in the target catalog.
    target_idx: Vec<usize>,
}

/// Where the pass accepts a pair.
#[derive(Debug, Clone, Copy)]
enum Gate {
    /// Within this many pixels of the prediction: the first pass, from the hypothesis.
    Radius(f64),
    /// Within `√χ²₀.₉₉(2)` of the pair's σ, its variance raised by this excess: every later pass.
    Excess(f64),
}

/// One candidate pair of a pass.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    idxs: MatchIdxs,
    /// The residual's length, in pixels.
    distance: f64,
    /// The pair's formal variance, its stars' position σ carried through the warp.
    variance: f64,
}

impl FitCatalogs {
    /// The catalogs of `reference` and `target`, the target holding at least one unsaturated star.
    pub(crate) fn new(reference: &[Star], target: &[Star]) -> Self {
        let variance = |star: &Star| star.pos_sigma * star.pos_sigma;
        let (target_points, target_idx): (Vec<DVec2>, Vec<usize>) = target
            .iter()
            .enumerate()
            .filter(|(_, star)| !star.saturated)
            .map(|(idx, star)| (star.pos, idx))
            .unzip();
        Self {
            reference: reference.iter().map(|star| star.pos).collect(),
            reference_variance: reference.iter().map(variance).collect(),
            reference_usable: reference.iter().map(|star| !star.saturated).collect(),
            target: target.iter().map(|star| star.pos).collect(),
            target_variance: target.iter().map(variance).collect(),
            target_tree: KdTree::build(target_points)
                .expect("registration gates the unsaturated targets before the fit"),
            target_idx,
        }
    }

    /// The residual of `pair` under `warp`, over the pair's σ, its variance raised by `excess`.
    pub(crate) fn normalized_residual(
        &self,
        warp: &WarpTransform,
        pair: MatchIdxs,
        excess: f64,
    ) -> f64 {
        let pos = self.reference[pair.reference];
        (warp.apply(pos) - self.target[pair.target]).length()
            / (self.pair_variance(warp, pair) + excess).sqrt()
    }

    /// The variance of `pair`'s residual under `warp`, in target pixels: the reference star's
    /// position variance carried through the warp's local area scale, and the target's own.
    fn pair_variance(&self, warp: &WarpTransform, pair: MatchIdxs) -> f64 {
        warp.jacobian(self.reference[pair.reference])
            .determinant()
            .abs()
            * self.reference_variance[pair.reference]
            + self.target_variance[pair.target]
    }

    /// Every usable reference star's nearest usable target under `warp`, that `gate` admits, each
    /// target taken by the reference star nearest it in units of σ.
    fn candidates(&self, warp: &WarpTransform, gate: Gate, out: &mut Vec<Candidate>) {
        out.clear();
        for (reference, &pos) in self.reference.iter().enumerate() {
            if !self.reference_usable[reference] {
                continue;
            }
            let predicted = warp.apply(pos);
            let Some(nearest) = self.target_tree.nearest_one(predicted) else {
                continue;
            };
            let idxs = MatchIdxs {
                reference,
                target: self.target_idx[nearest.idx],
            };
            let variance = self.pair_variance(warp, idxs);
            debug_assert!(
                variance > 0.0,
                "a positive position σ leaves every pair a variance"
            );
            let admitted = match gate {
                Gate::Radius(radius) => nearest.dist_sq <= radius * radius,
                Gate::Excess(excess) => nearest.dist_sq <= CHI2_99_2DOF * (variance + excess),
            };
            if admitted {
                out.push(Candidate {
                    idxs,
                    distance: nearest.dist_sq.sqrt(),
                    variance,
                });
            }
        }
        out.sort_unstable_by(|a, b| {
            a.idxs
                .target
                .cmp(&b.idxs.target)
                .then((a.distance / a.variance.sqrt()).total_cmp(&(b.distance / b.variance.sqrt())))
                .then(a.idxs.reference.cmp(&b.idxs.reference))
        });
        out.dedup_by_key(|candidate| candidate.idxs.target);
        out.sort_unstable_by_key(|candidate| (candidate.idxs.reference, candidate.idxs.target));
    }

    /// The fit of `model` from `hypothesis`, the first pass gated at `radius` pixels.
    ///
    /// A pass matches, measures the excess variance `σₑ²` its residuals hold past their stars'
    /// position σ ([`excess_variance`]), weights each pair by `1/(1 + (z/c)²)` over its variance
    /// raised by it, `z` the residual over that σ, and refits. The next pass gates at
    /// `√χ²₀.₉₉(2)` of it. The fit has converged when a pass keeps the same pairs and moves no
    /// prediction by [`CONVERGED_PX`].
    ///
    /// The excess adds to each pair's variance, where one scale would multiply it: a floor every
    /// star shares — the seeing's jitter between frames, a distortion the model leaves — does not
    /// shrink with a star's SNR, so a scale fitted to the faint stars gates out the best-measured
    /// ones, three in ten under a 0.03 px floor at σ 0.01 px.
    pub(crate) fn fit(
        &self,
        hypothesis: Transform,
        model: FitModel,
        radius: f64,
    ) -> Result<FinalFit, FinalFitFailure> {
        let mut warp = WarpTransform::from(hypothesis);
        let mut gate = Gate::Radius(radius);
        let mut candidates = Vec::new();
        let mut prev: Vec<MatchIdxs> = Vec::new();
        let mut normalized = Vec::new();
        let mut pairs = WeightedPairs::default();
        let mut excess = 0.0;
        for _ in 0..MAX_PASSES {
            self.candidates(&warp, gate, &mut candidates);
            let found = candidates.len();
            if found < model.transform_type().min_points() {
                return Err(FinalFitFailure::TooFewPairs {
                    found,
                    required: model.transform_type().min_points(),
                });
            }
            if found < model.required_pairs() {
                return Err(FinalFitFailure::TooFewSipPairs {
                    found,
                    required: model.required_pairs(),
                });
            }
            excess = excess_variance(&candidates, &mut normalized);

            pairs.clear();
            for candidate in &candidates {
                let variance = candidate.variance + excess;
                let robust = 1.0
                    / (1.0 + (candidate.distance / (CAUCHY_TUNING * variance.sqrt())).libm_powi(2));
                pairs.push(
                    self.reference[candidate.idxs.reference],
                    self.target[candidate.idxs.target],
                    robust / variance,
                );
            }
            let refit = pairs.fit_warp(model).ok_or(FinalFitFailure::Degenerate)?;

            let same_pairs = candidates
                .iter()
                .map(|candidate| candidate.idxs)
                .eq(prev.iter().copied());
            let moved = pairs
                .reference
                .iter()
                .map(|&r| refit.apply(r).distance(warp.apply(r)))
                .fold(0.0, f64::max);
            warp = refit;
            if same_pairs && moved <= CONVERGED_PX {
                break;
            }
            prev.clear();
            prev.extend(candidates.iter().map(|candidate| candidate.idxs));
            gate = Gate::Excess(excess);
        }

        let matches: Vec<StarMatch> = candidates
            .iter()
            .map(|candidate| StarMatch {
                idxs: candidate.idxs,
                residual: (warp.apply(self.reference[candidate.idxs.reference])
                    - self.target[candidate.idxs.target])
                    .length(),
            })
            .collect();
        Ok(FinalFit {
            result: RegistrationResult::new(warp, matches),
            excess_variance: excess,
        })
    }
}

/// The variance, in square pixels, the residuals of `candidates` hold past their pairs' own: the
/// least `σₑ²` whose raised σ puts the median residual at the Rayleigh median, as one of unit σ
/// reads; 0 where the pairs' own σ explains it. The median of `dᵢ/√(vᵢ + σₑ²)` falls as `σₑ²`
/// grows, so a bisection over `[0, max dᵢ²]`, whose top puts every ratio at most 1, below the
/// Rayleigh median 1.18, finds it.
fn excess_variance(candidates: &[Candidate], scratch: &mut Vec<f64>) -> f64 {
    let mut median_at = |excess: f64| {
        scratch.clear();
        scratch.extend(
            candidates
                .iter()
                .map(|candidate| candidate.distance / (candidate.variance + excess).sqrt()),
        );
        statistics::median_mut(scratch)
    };
    if median_at(0.0) <= RAYLEIGH_MEDIAN {
        return 0.0;
    }
    let (mut low, mut high) = (
        0.0,
        candidates
            .iter()
            .map(|candidate| candidate.distance * candidate.distance)
            .fold(0.0, f64::max),
    );
    for _ in 0..EXCESS_BISECTIONS {
        let mid = f64::midpoint(low, high);
        if median_at(mid) > RAYLEIGH_MEDIAN {
            low = mid;
        } else {
            high = mid;
        }
    }
    high
}
