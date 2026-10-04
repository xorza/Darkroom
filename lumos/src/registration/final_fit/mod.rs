//! The final registration fit: from a hypothesis transform, every usable pair of the full catalogs,
//! weighted by its positional variance and robustly, under a gate that shrinks with the fit.
//!
//! RANSAC's consensus is the brightest stars a sample agreed on, judged at a noise scale of half
//! the FWHM. The fit the frame is warped by deserves every star and every star's precision: each
//! pass matches the full catalogs through the current transform, weights each pair by the inverse
//! of its variance `|det J|·σ_ref² + σ_target²` from the stars' position σ, takes a Cauchy weight
//! on its normalized residual, and refits; the gate starts at the recovery radius and then
//! follows the residuals' own scale. Saturated stars, whose flat tops centre poorly, take no part.
//! A SIP correction is fitted together with the transform in every pass, never after it.

mod homography_refinement;
mod weighted_pairs;

use glam::DVec2;

use crate::math::statistics::{CHI2_99_2DOF, median_mut};
use crate::registration::distortion::sip::SipPolynomial;
use crate::registration::final_fit::weighted_pairs::WeightedPairs;
use crate::registration::result::StarMatch;
use crate::registration::spatial::KdTree;
use crate::registration::transform::{Transform, TransformType, WarpTransform};
use crate::registration::triangle::voting::MatchIndices;
use crate::star_detection::star::Star;

/// The median of a Rayleigh distribution of unit σ, `√(2 ln 2)`: what the median of 2-D residual
/// lengths reads, in units of their per-axis σ, when the noise is what the position σ claims.
const RAYLEIGH_MEDIAN: f64 = 1.177_410_022_515_474_7;

/// Holland and Welsch's (1977) tuning constant for the Cauchy loss, 95% efficient at the normal.
const CAUCHY_TUNING: f64 = 2.3849;

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

/// What the final fit fits: a transform, and optionally a SIP correction applied before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FitModel {
    pub(crate) transform: TransformType,
    pub(crate) sip: Option<SipModel>,
}

/// A SIP correction's order and the origin its polynomial is taken about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SipModel {
    pub(crate) order: usize,
    pub(crate) origin: DVec2,
}

impl FitModel {
    /// The model's free parameters: the transform's, and the SIP correction's two per term.
    pub(crate) fn parameter_count(self) -> usize {
        self.transform.parameter_count()
            + self
                .sip
                .map_or(0, |sip| 2 * SipPolynomial::required_points(sip.order) / 3)
    }

    /// The pairs the model needs: the transform's minimal sample, or the SIP fit's floor.
    fn required_pairs(self) -> usize {
        self.sip.map_or(self.transform.min_points(), |sip| {
            SipPolynomial::required_points(sip.order).max(self.transform.min_points())
        })
    }
}

/// The final fit of one model.
#[derive(Debug)]
pub(crate) struct FinalFit {
    pub(crate) warp: WarpTransform,
    /// The pairs the warp was fitted to, with their residuals in target pixels.
    pub(crate) matches: Vec<StarMatch>,
    /// The robust scale of the normalized residuals, at least 1: how far the residuals spread
    /// beyond what the stars' position σ claims.
    pub(crate) scale: f64,
}

/// Why a final fit produced no transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalFitFailure {
    /// Fewer pairs passed the gate than the transform needs.
    TooFewPairs { found: usize, required: usize },
    /// Fewer pairs passed the gate than the SIP correction needs.
    TooFewSipPairs { found: usize, required: usize },
    /// The pairs do not determine the model.
    Degenerate,
}

impl FitCatalogs {
    /// `None` when no target star is unsaturated.
    pub(crate) fn new(reference: &[Star], target: &[Star]) -> Option<Self> {
        let variance = |star: &Star| star.position_sigma * star.position_sigma;
        let (target_points, target_index): (Vec<DVec2>, Vec<usize>) = target
            .iter()
            .enumerate()
            .filter(|(_, star)| !star.saturated)
            .map(|(index, star)| (star.pos, index))
            .unzip();
        Some(Self {
            reference: reference.iter().map(|star| star.pos).collect(),
            reference_variance: reference.iter().map(variance).collect(),
            reference_usable: reference.iter().map(|star| !star.saturated).collect(),
            target: target.iter().map(|star| star.pos).collect(),
            target_variance: target.iter().map(variance).collect(),
            target_tree: KdTree::build(target_points)?,
            target_index,
        })
    }

    /// The residual of `pair` under `warp`, over the pair's σ.
    pub(crate) fn normalized_residual(&self, warp: &WarpTransform, pair: MatchIndices) -> f64 {
        let position = self.reference[pair.reference];
        let variance = warp.jacobian(position).determinant().abs()
            * self.reference_variance[pair.reference]
            + self.target_variance[pair.target];
        (warp.apply(position) - self.target[pair.target]).length() / variance.sqrt()
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
            let target = self.target_index[nearest.index];
            let variance = warp.jacobian(position).determinant().abs()
                * self.reference_variance[reference]
                + self.target_variance[target];
            let normalized = nearest.dist_sq.sqrt() / variance.sqrt();
            let admitted = match gate {
                Gate::Radius(radius) => nearest.dist_sq <= radius * radius,
                Gate::Scale(scale) => normalized <= CHI2_99_2DOF.sqrt() * scale,
            };
            if admitted && normalized.is_finite() {
                out.push(Candidate {
                    indices: MatchIndices { reference, target },
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
}

impl FinalFit {
    /// Fit `model` from `hypothesis`, the first pass gated at `radius` pixels.
    ///
    /// A pass matches, measures the robust scale `ŝ` of the normalized residuals — their median
    /// over the Rayleigh median, at least 1, as the position σ is the noise floor and the scale
    /// only widens it — weights each pair by `1/(1 + (z/(c·ŝ))²)` over its variance, and refits.
    /// The next pass gates at `√χ²₀.₉₉(2)·ŝ`. The fit has converged when a pass keeps the same
    /// pairs and moves no prediction by [`CONVERGED_PX`].
    pub(crate) fn run(
        catalogs: &FitCatalogs,
        hypothesis: Transform,
        model: FitModel,
        radius: f64,
    ) -> Result<Self, FinalFitFailure> {
        let mut warp = WarpTransform::new(hypothesis);
        let mut gate = Gate::Radius(radius);
        let mut candidates = Vec::new();
        let mut previous: Vec<MatchIndices> = Vec::new();
        let mut normalized = Vec::new();
        let mut pairs = WeightedPairs::default();
        let mut scale = 1.0;
        for _ in 0..MAX_PASSES {
            catalogs.candidates(&warp, gate, &mut candidates);
            let found = candidates.len();
            if found < model.transform.min_points() {
                return Err(FinalFitFailure::TooFewPairs {
                    found,
                    required: model.transform.min_points(),
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
            scale = (median_mut(&mut normalized) / RAYLEIGH_MEDIAN).max(1.0);

            pairs.clear();
            for candidate in &candidates {
                let robust = 1.0 / (1.0 + (candidate.normalized / (CAUCHY_TUNING * scale)).powi(2));
                pairs.push(
                    catalogs.reference[candidate.indices.reference],
                    catalogs.target[candidate.indices.target],
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

        let matches = candidates
            .iter()
            .map(|candidate| StarMatch {
                indices: candidate.indices,
                residual: (warp.apply(catalogs.reference[candidate.indices.reference])
                    - catalogs.target[candidate.indices.target])
                    .length(),
            })
            .collect();
        Ok(Self {
            warp,
            matches,
            scale,
        })
    }
}

#[cfg(test)]
mod tests;
