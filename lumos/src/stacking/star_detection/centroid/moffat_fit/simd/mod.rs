//! Vector backends for the Moffat batch kernels, and the dispatch between them.
//!
//! Both entry points return `None` rather than falling back here, because the scalar forms are
//! `lm_optimizer`'s generic ones — shared with every other model — and belong to the caller, not
//! to this module.

use crate::simd::dispatch;
use crate::stacking::star_detection::centroid::lm_optimizer::{FitData, NormalEquations};
use crate::stacking::star_detection::centroid::moffat_fit::MoffatFixedBeta;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

/// Accumulate the normal equations for one Levenberg-Marquardt step over the whole stamp.
///
/// `None` when the target has no backend, or when the fit is weighted — the kernels are
/// unweighted-only.
pub(super) fn batch_build_normal_equations(
    model: &MoffatFixedBeta,
    data: FitData,
    params: &[f64; 5],
) -> Option<NormalEquations<5>> {
    if data.weights.is_some() {
        return None;
    }
    dispatch! {
        x86: avx2_fma => Some(avx2::batch_build_normal_equations_avx2(
            model, data.x, data.y, data.z, params,
        )),
        aarch64 => Some(neon::batch_build_normal_equations_neon(
            model, data.x, data.y, data.z, params,
        )),
        scalar => None,
    }
}

/// Residual sum of squares over the whole stamp. `None` on the same two conditions as
/// [`batch_build_normal_equations`].
pub(super) fn batch_compute_chi2(
    model: &MoffatFixedBeta,
    data: FitData,
    params: &[f64; 5],
) -> Option<f64> {
    if data.weights.is_some() {
        return None;
    }
    dispatch! {
        x86: avx2_fma => Some(avx2::batch_compute_chi2_avx2(
            model, data.x, data.y, data.z, params,
        )),
        aarch64 => Some(neon::batch_compute_chi2_neon(
            model, data.x, data.y, data.z, params,
        )),
        scalar => None,
    }
}
