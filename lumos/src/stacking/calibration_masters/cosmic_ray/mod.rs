//! Single-frame cosmic-ray rejection via Laplacian edge detection (L.A.Cosmic, van Dokkum 2001).
//!
//! Cosmic rays and satellite/airplane streaks are sharp, single-frame events that stack-time
//! sigma/winsor rejection can't out-vote on short sequences. L.A.Cosmic flags them in a *single*
//! calibrated frame: a CR has sharper edges than a (PSF-broadened) star, so a Laplacian highlights
//! it, and a fine-structure test separates CRs from real point sources. Flagged pixels are
//! in-painted with the median of their unflagged neighbors, then the detect→replace loop repeats so
//! multi-pixel hits are fully removed.
//!
//! Runs on the calibrated, linear `CfaImage` before demosaic/registration (warping or demosaic
//! would smear a hit across pixels).
//!
//! Dispatches per CFA type: **Mono** = textbook subsampled L.A.Cosmic; **Bayer** = deinterleave the
//! four 2×2 phases and reuse the mono detector per dense same-color plane; **X-Trans** = same-color
//! stencils on the mosaic via `color_at` (no dense same-color sub-lattice exists there).
//!
//! **CFA caveat:** L.A.Cosmic assumes a PSF-sampled image, but a Bayer phase plane is half-resolution
//! — a tight star (FWHM ≲ 2–3 px in the mosaic) becomes ~1 px there, where the CR-vs-star
//! fine-structure test weakens. This per-frame rejection is therefore best for **short, un-dithered**
//! sequences; for dithered sets prefer dither + stack-time σ/winsor rejection, which out-votes CRs
//! without a per-frame discriminator. (`xtrans_removes_cosmic_ray...` / `bayer_tight_star...` tests
//! pin the tight-star behavior.)

pub(crate) mod config;
pub(crate) mod masks;
pub(crate) mod mono;
mod xtrans;

use rayon::prelude::*;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::size2us::Size2us;

use crate::stacking::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::stacking::calibration_masters::cosmic_ray::mono::MonoDetector;
use crate::stacking::calibration_masters::cosmic_ray::xtrans::reject_xtrans;

/// `F` is floored to this (in normalized pixel units) so it stays non-negative where the object fine
/// structure is ~0 (i.e. at a CR).
const FINE_STRUCTURE_FLOOR: f32 = 1e-6;

/// Floor for the **noise-normalized** fine structure `F/noise` in the contrast test (in σ units).
/// Matches astroscrappy's `f.clip(min=0.01)` — bounds the `S'/(F/noise)` ratio where fine structure
/// is ~0 so a CR (F→0) doesn't divide by zero.
const FINE_STRUCTURE_SIGMA_FLOOR: f32 = 0.01;

/// Detect and in-paint cosmic rays in a single calibrated frame, in place, dispatching on its CFA
/// type (mono / Bayer / X-Trans). Returns the number of CR pixels corrected.
pub(crate) fn reject_cosmic_rays(image: &mut CfaImage, config: &CosmicRayConfig) -> usize {
    let size = Size2us::new(image.data.width(), image.data.height());
    // Disjoint fields: the pixels go in by `&mut`, the CFA type is read from the metadata beside it.
    let pixels = image.data.pixels_mut();
    match &image.metadata.cfa_type {
        // Bayer is 2×2-periodic → four dense same-color planes; reuse the mono detector per plane.
        Some(CfaType::Bayer(_)) => reject_bayer(pixels, size, config),
        // X-Trans has no dense same-color sub-lattice → same-color stencils on the mosaic.
        Some(c @ CfaType::XTrans(_)) => reject_xtrans(pixels, size, c, config),
        // Mono (or an unlabeled frame): the dense Laplacian path.
        _ => MonoDetector::new(config).reject(pixels, size),
    }
}

/// Bayer: the mosaic is 2×2-periodic, so pixels sharing a `(x%2, y%2)` phase are the same color and
/// form a dense plane. Deinterleave the four phases, run [`MonoDetector::reject`] on each (its dense
/// neighbors are same-color in the mosaic), and write the cleaned planes back. Pattern-independent —
/// phase alone determines color, so no `CfaPattern` is needed.
///
/// One [`MonoDetector`] and one `plane` serve all four phases.
///
/// Deinterleave and re-interleave are row-parallel like the detector between them. They are only a
/// few percent of a frame today, but they are the whole of its *serial* fraction — the one part
/// that would not shrink as thread count rises.
fn reject_bayer(data: &mut [f32], size: Size2us, config: &CosmicRayConfig) -> usize {
    let (w, h) = (size.width, size.height);
    let mut detector = MonoDetector::new(config);
    let mut plane: Vec<f32> = Vec::new();
    let mut total = 0;
    for b in 0..2 {
        for a in 0..2 {
            let pw = if a == 0 { w.div_ceil(2) } else { w / 2 };
            let ph = if b == 0 { h.div_ceil(2) } else { h / 2 };
            if pw < 3 || ph < 3 {
                continue;
            }

            // Deinterleave: plane row j is mosaic row 2j+b, every second pixel from column a. Every
            // element is written, so `resize` only has to get the length right.
            plane.resize(pw * ph, 0.0);
            let mosaic = &*data;
            plane.par_chunks_mut(pw).enumerate().for_each(|(j, row)| {
                let src = &mosaic[(j * 2 + b) * w..][..w];
                for (i, o) in row.iter_mut().enumerate() {
                    *o = src[i * 2 + a];
                }
            });

            total += detector.reject(&mut plane, Size2us::new(pw, ph));

            // Re-interleave the cleaned plane. Chunking the mosaic by row keeps each thread's
            // writes to one row, so the phase's rows can be picked out of the full sweep.
            let cleaned = &plane[..];
            data.par_chunks_mut(w)
                .enumerate()
                .filter(|(y, _)| y % 2 == b)
                .for_each(|(y, row)| {
                    for (i, &v) in cleaned[(y / 2) * pw..][..pw].iter().enumerate() {
                        row[i * 2 + a] = v;
                    }
                });
        }
    }
    total
}

#[cfg(test)]
mod tests;
