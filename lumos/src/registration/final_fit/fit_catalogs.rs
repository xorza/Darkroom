//! [`FitCatalogs`]: the catalogs as the final fit reads them, and the passes that fit a model
//! over them.

use glam::DVec2;

use crate::math::statistics;
use crate::math::statistics::CHI2_99_2DOF;
use crate::registration::final_fit::weighted_pairs::WeightedPairs;
use crate::registration::final_fit::{FinalFit, FinalFitFailure, FitModel};
use crate::registration::kd_tree::KdTree;
use crate::registration::match_indices::MatchIndices;
use crate::registration::registration_result::{self, StarMatch};
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
    target_index: Vec<usize>,
}

/// Where the pass accepts a pair.
#[derive(Debug, Clone, Copy)]
enum Gate {
    /// Within this many pixels of the prediction: the first pass, from the hypothesis.
    Radius(f64),
    /// Within `√χ²₀.₉₉(2)` of this scale, in units of the pair's σ: every later pass.
    Scale(f64),
}

/// One candidate pair of a pass.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    indices: MatchIndices,
    /// The residual over the pair's σ.
    normalized: f64,
    variance: f64,
}

impl FitCatalogs {
    /// The catalogs of `reference` and `target`, the target holding at least one unsaturated star.
    pub(crate) fn new(reference: &[Star], target: &[Star]) -> Self {
        let variance = |star: &Star| star.position_sigma * star.position_sigma;
        let (target_points, target_index): (Vec<DVec2>, Vec<usize>) = target
            .iter()
            .enumerate()
            .filter(|(_, star)| !star.saturated)
            .map(|(index, star)| (star.pos, index))
            .unzip();
        Self {
            reference: reference.iter().map(|star| star.pos).collect(),
            reference_variance: reference.iter().map(variance).collect(),
            reference_usable: reference.iter().map(|star| !star.saturated).collect(),
            target: target.iter().map(|star| star.pos).collect(),
            target_variance: target.iter().map(variance).collect(),
            target_tree: KdTree::build(target_points)
                .expect("registration gates the unsaturated targets before the fit"),
            target_index,
        }
    }

    /// The residual of `pair` under `warp`, over the pair's σ.
    pub(crate) fn normalized_residual(&self, warp: &WarpTransform, pair: MatchIndices) -> f64 {
        let position = self.reference[pair.reference];
        (warp.apply(position) - self.target[pair.target]).length()
            / self.pair_variance(warp, pair).sqrt()
    }

    /// The variance of `pair`'s residual under `warp`, in target pixels: the reference star's
    /// position variance carried through the warp's local area scale, and the target's own.
    fn pair_variance(&self, warp: &WarpTransform, pair: MatchIndices) -> f64 {
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
        for (reference, &position) in self.reference.iter().enumerate() {
            if !self.reference_usable[reference] {
                continue;
            }
            let predicted = warp.apply(position);
            let Some(nearest) = self.target_tree.nearest_one(predicted) else {
                continue;
            };
            let indices = MatchIndices {
                reference,
                target: self.target_index[nearest.index],
            };
            let variance = self.pair_variance(warp, indices);
            let normalized = nearest.dist_sq.sqrt() / variance.sqrt();
            let admitted = match gate {
                Gate::Radius(radius) => nearest.dist_sq <= radius * radius,
                Gate::Scale(scale) => normalized <= CHI2_99_2DOF.sqrt() * scale,
            };
            debug_assert!(
                normalized.is_finite(),
                "a positive position σ leaves every residual finite"
            );
            if admitted {
                out.push(Candidate {
                    indices,
                    normalized,
                    variance,
                });
            }
        }
        out.sort_unstable_by(|a, b| {
            a.indices
                .target
                .cmp(&b.indices.target)
                .then(a.normalized.total_cmp(&b.normalized))
                .then(a.indices.reference.cmp(&b.indices.reference))
        });
        out.dedup_by_key(|candidate| candidate.indices.target);
        out.sort_unstable_by_key(|candidate| {
            (candidate.indices.reference, candidate.indices.target)
        });
    }

    /// The fit of `model` from `hypothesis`, the first pass gated at `radius` pixels.
    ///
    /// A pass matches, measures the robust scale `ŝ` of the normalized residuals — their median
    /// over the Rayleigh median, at least 1, as the position σ is the noise floor and the scale
    /// only widens it — weights each pair by `1/(1 + (z/(c·ŝ))²)` over its variance, and refits.
    /// The next pass gates at `√χ²₀.₉₉(2)·ŝ`. The fit has converged when a pass keeps the same
    /// pairs and moves no prediction by [`CONVERGED_PX`].
    pub(crate) fn fit(
        &self,
        hypothesis: Transform,
        model: FitModel,
        radius: f64,
    ) -> Result<FinalFit, FinalFitFailure> {
        let mut warp = WarpTransform::new(hypothesis);
        let mut gate = Gate::Radius(radius);
        let mut candidates = Vec::new();
        let mut previous: Vec<MatchIndices> = Vec::new();
        let mut normalized = Vec::new();
        let mut pairs = WeightedPairs::default();
        let mut scale = 1.0;
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
            normalized.clear();
            normalized.extend(candidates.iter().map(|candidate| candidate.normalized));
            scale = (statistics::median_mut(&mut normalized) / RAYLEIGH_MEDIAN).max(1.0);

            pairs.clear();
            for candidate in &candidates {
                let robust = 1.0 / (1.0 + (candidate.normalized / (CAUCHY_TUNING * scale)).powi(2));
                pairs.push(
                    self.reference[candidate.indices.reference],
                    self.target[candidate.indices.target],
                    robust / candidate.variance,
                );
            }
            let refit = pairs.fit_warp(model).ok_or(FinalFitFailure::Degenerate)?;

            let same_pairs = candidates
                .iter()
                .map(|candidate| candidate.indices)
                .eq(previous.iter().copied());
            let moved = pairs
                .reference
                .iter()
                .map(|&r| refit.apply(r).distance(warp.apply(r)))
                .fold(0.0, f64::max);
            warp = refit;
            if same_pairs && moved <= CONVERGED_PX {
                break;
            }
            previous.clear();
            previous.extend(candidates.iter().map(|candidate| candidate.indices));
            gate = Gate::Scale(scale);
        }

        let matches: Vec<StarMatch> = candidates
            .iter()
            .map(|candidate| StarMatch {
                indices: candidate.indices,
                residual: (warp.apply(self.reference[candidate.indices.reference])
                    - self.target[candidate.indices.target])
                    .length(),
            })
            .collect();
        Ok(FinalFit {
            warp,
            rms_error: registration_result::rms_error(&matches),
            matches,
            scale,
        })
    }
}
