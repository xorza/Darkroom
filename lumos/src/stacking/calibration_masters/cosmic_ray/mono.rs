//! Textbook L.A.Cosmic on one dense plane.
//!
//! Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)`
//! → fine structure `F` → flag → grow → in-paint → iterate. Also serves each deinterleaved Bayer
//! phase, whose dense neighbours are same-colour in the mosaic.

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_mut};
use crate::math::vec2us::Vec2us;

use crate::stacking::calibration_masters::cosmic_ray::FINE_STRUCTURE_FLOOR;
use crate::stacking::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
use crate::stacking::calibration_masters::cosmic_ray::masks::CrMasks;

/// The mono detector's frame-sized `f32` working set, allocated on the first iteration and reused
/// by every one after it.
///
/// Each buffer is written in full before it is read, so what the previous iteration — or the
/// previous Bayer plane — left in it never matters; only the capacity does. The four Bayer phase
/// planes differ by at most a row and a column, so one scratch serves all four: the `resize` in
/// each producer keeps the largest allocation.
///
/// Five planes, not the eight the stages name: `significance` and `fine` are rewritten in place by
/// the elementwise step that consumes them, and `median` and `frame` are each handed from one
/// stage to the next. On a 6144² mono frame that is 720 MB of working set instead of 1.1 GB —
/// and, the point of the struct, no allocation at all after the first iteration.
#[derive(Debug, Default)]
struct MonoScratch {
    /// `L⁺`, then the significance `S = L⁺/(2N)`, then `S' = S − median₅(S)`, each in place.
    significance: Vec<f32>,
    /// `median₃(I)`, then the object fine structure `F = median₃ − median₇(median₃)` in place.
    fine: Vec<f32>,
    /// Per-pixel noise `N`.
    noise: Vec<f32>,
    /// The window medians, one at a time: `median₇(median₃(I))`, then `median₅(I)`, then
    /// `median₅(S)`. Each is consumed by the step immediately after it, so the three never overlap.
    median: Vec<f32>,
    /// Whole-frame scratch: the copy the empirical background median and MAD consume, then the
    /// read-only snapshot [`replace_flagged`] gathers from — again never both at once.
    frame: Vec<f32>,
}

/// Monochrome L.A.Cosmic on one plane (also each deinterleaved Bayer plane). Subsample ×2 → clipped
/// Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)` → fine structure `F`
/// → flag → grow → in-paint → iterate. Returns the CR pixel count.
/// The mono cosmic-ray detector: its configuration, and the working set it reuses.
///
/// Owning both is what lets one detector clean every Bayer phase plane with a single allocation —
/// `(0, 0)` is the largest phase and runs first, so no later one grows the buffers — without the
/// caller having to thread a scratch through by hand and know that rule.
#[derive(Debug)]
pub(super) struct MonoDetector<'a> {
    config: &'a CosmicRayConfig,
    scratch: MonoScratch,
}

impl<'a> MonoDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig) -> Self {
        Self {
            config,
            scratch: MonoScratch::default(),
        }
    }

    /// Detect and in-paint cosmic rays in one dense plane, in place, returning the CR pixel count.
    ///
    /// Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` →
    /// `S' = S − median₅(S)` → fine structure `F` → flag → grow → in-paint → iterate.
    pub(super) fn reject(&mut self, data: &mut [f32], size: Size2us) -> usize {
        debug_assert_eq!(data.len(), size.pixel_count());
        if size.width < 3 || size.height < 3 {
            return 0;
        }
        let mut masks = CrMasks::new(size);

        for _ in 0..self.config.niter {
            let MonoScratch {
                significance,
                fine,
                noise,
                median,
                frame,
            } = &mut self.scratch;
            let pix = &*data;

            // L⁺: clipped Laplacian of the ×2-subsampled frame, averaged back to native resolution.
            laplacian_plus_into(pix, size, significance);

            // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources, ~0 at
            // a CR (median₃ already erased the spike). The difference is elementwise, so it lands back
            // over median₃ in `fine` rather than in a buffer of its own.
            median_window_into(pix, size, 1, fine);
            median_window_into(fine, size, 3, median);
            for (a, &b) in fine.iter_mut().zip(&*median) {
                *a = (*a - b).max(FINE_STRUCTURE_FLOOR);
            }

            // Significance S = L⁺/(2N), then S' = S − median₅(S) to strip smooth large-scale structure.
            // Both steps are elementwise over the same extent, so they run in place down the Laplacian
            // buffer instead of allocating a frame each.
            median_window_into(pix, size, 2, median);
            noise_map_into(pix, median, &self.config.noise, noise, frame);
            for (l, &nz) in significance.iter_mut().zip(&*noise) {
                *l /= 2.0 * nz;
            }
            median_window_into(significance, size, 2, median);
            for (v, &m) in significance.iter_mut().zip(&*median) {
                *v -= m;
            }

            if masks.detect_and_grow(significance, fine, noise, self.config) == 0 {
                break;
            }
            replace_flagged(data, size, &masks.accumulated, frame);
        }

        masks.accumulated.count_ones()
    }
}

/// Clipped Laplacian of the ×2-subsampled frame, block-averaged back down to `size`.
///
/// Convolves the ×2 image with `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clips negatives to 0 (keeping only
/// sharp positive peaks), then averages each 2×2 block. Edge-clamped on the ×2 grid.
///
/// The ×2 image is never materialized. Subsampling here is a block replication — every `sub`
/// sample is `data[y2 / 2][x2 / 2]` — so it is read through that index instead of being written to
/// a buffer four times pixel count, and the clipped Laplacian is averaged as it is produced rather
/// than stored in a second buffer of the same size. That is 8n floats of allocation and traffic
/// removed from every iteration; what remains is the `n`-length result, written into the caller's
/// buffer.
fn laplacian_plus_into(data: &[f32], size: Size2us, out: &mut Vec<f32>) {
    let (w2, h2) = (size.width * 2, size.height * 2);
    // The ×2 sample at (x2, y2), which is the native pixel under it.
    let at = |y2: usize, x2: usize| data[(y2 / 2) * size.width + (x2 / 2)];

    // Every element is written below, so only the length matters.
    out.resize(size.pixel_count(), 0.0);
    out.par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for dy in 0..2 {
                    let y2 = 2 * y + dy;
                    let yu = y2.saturating_sub(1);
                    let yd = (y2 + 1).min(h2 - 1);
                    for dx in 0..2 {
                        let x2 = 2 * x + dx;
                        let xl = x2.saturating_sub(1);
                        let xr = (x2 + 1).min(w2 - 1);
                        let v =
                            4.0 * at(y2, x2) - at(yu, x2) - at(yd, x2) - at(y2, xl) - at(y2, xr);
                        sum += v.max(0.0);
                    }
                }
                *o = 0.25 * sum;
            }
        });
}

/// Median over a `(2r+1)²` window, replicating the border pixel for out-of-bounds coordinates so
/// every output sees a full window. Scalar, row-parallel.
///
/// Deliberately not [`median_filter_3x3`](crate::stacking::star_detection::median_filter), even
/// though `r == 1` describes the same 3×3 median: that one *shrinks* its window at the border to
/// the 4 or 6 in-bounds samples where this one replicates. L.A.Cosmic differences two of these
/// windows against each other — the fine structure is `median₃ − median₇(median₃)` — so every
/// radius here has to share one border convention. Feeding a shrunk `median₃` into that
/// difference while `median₇` stayed replicated would corrupt the border in a way neither
/// convention does alone. Replication is also the usual choice for astronomical median filtering.
///
/// The two *could* still share an interior kernel: `median_filter`'s `median_filter_row_simd`
/// takes three rows and fills the interior, knowing nothing about edges. That is worth doing
/// behind a cosmic-ray benchmark rather than before one — it couples two subsystems to accelerate
/// one of the four windows below (areas 9, 49, 25, 25), and an `r == 1` fast path would have to
/// be proven bit-identical to this general one or the detection changes.
fn median_window_into(data: &[f32], size: Size2us, r: usize, out: &mut Vec<f32>) {
    let ri = r as isize;
    let (wi, hi) = (size.width as isize, size.height as isize);
    // Every element is written below, so only the length matters.
    out.resize(size.pixel_count(), 0.0);
    out.par_chunks_mut(size.width).enumerate().for_each_init(
        || Vec::<f32>::with_capacity((2 * r + 1) * (2 * r + 1)),
        |buf, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                buf.clear();
                for dy in -ri..=ri {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -ri..=ri {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        buf.push(data[size.index_of(Vec2us::new(xx, yy))]);
                    }
                }
                *o = median_f32_mut(buf);
            }
        },
    );
}

/// Per-pixel noise `N` from the median-filtered (CR-free) signal estimate `m5`, into `out`.
///
/// `scratch` is a frame-sized buffer the empirical background statistics consume; what it holds on
/// entry means nothing, and what it holds on return means nothing either.
fn noise_map_into(
    data: &[f32],
    m5: &[f32],
    noise: &NoiseEstimation,
    out: &mut Vec<f32>,
    scratch: &mut Vec<f32>,
) {
    match *noise {
        NoiseEstimation::Empirical => {
            scratch.clear();
            scratch.extend_from_slice(data);
            let bg = median_f32_mut(scratch);
            let sigma_bg = mad_to_sigma(mad_f32_fast(data, bg, scratch)).max(1e-9);
            out.clear();
            out.extend(m5.iter().map(|&s| empirical_noise(s, bg, sigma_bg)));
        }
        NoiseEstimation::Parametric {
            gain,
            read_noise,
            full_scale,
        } => parametric_noise_into(m5, gain, read_noise, full_scale, out),
    }
}

/// Empirical per-pixel noise: a read-noise floor `σ` plus a sky-anchored Poisson term that rises as
/// `σ²·(signal−bg)/max(bg,σ)` above the background. Shared by the mono (whole-image `bg,σ`) and
/// X-Trans (per-color `bg,σ`) paths so the model can't drift between them.
#[inline]
pub(super) fn empirical_noise(signal: f32, bg: f32, sigma: f32) -> f32 {
    let sigma2 = sigma * sigma;
    let slope = sigma2 / bg.max(sigma);
    (sigma2 + (signal - bg).max(0.0) * slope).sqrt()
}

/// Poisson + read noise per pixel from a CR-free signal estimate, in normalized units:
/// `N_e = √(gain·I_ADU + read_noise²)` mapped back through `full_scale`.
pub(super) fn parametric_noise_into(
    signal: &[f32],
    gain: f32,
    read_noise: f32,
    full_scale: f32,
    out: &mut Vec<f32>,
) {
    let denom = gain * full_scale;
    out.clear();
    out.extend(signal.iter().map(|&s| {
        let adu = s.max(0.0) * full_scale;
        ((gain * adu + read_noise * read_noise).sqrt() / denom).max(1e-9)
    }));
}

/// Replace masked pixels with the median of their unmasked 5×5 neighbors (edge-clamped);
/// fully-masked neighborhoods (huge CRs) are left for the next iteration to shrink.
///
/// The frame copy is not what makes replacements independent of each other — writes land only on
/// masked pixels and reads only on unmasked ones, so no replacement can consult a replaced
/// neighbour whatever order the rows run in. It is here because `pixels_mut()` is held across the
/// reads, and it pays for itself besides: reading a separate, read-only array keeps one row's
/// writes off the cache lines the rows above and below are reading. Aliasing the two through a raw
/// pointer is sound (the sets are disjoint) and was measured *slower* on every run of
/// `bench_cosmic_ray_reject_mono` — the false sharing costs more than the copy. The copy lands in
/// the caller's `snapshot` buffer, so it is a memcpy per iteration and not an allocation.
pub(super) fn replace_flagged(
    data: &mut [f32],
    size: Size2us,
    mask: &BitBuffer2,
    snapshot: &mut Vec<f32>,
) {
    snapshot.clear();
    snapshot.extend_from_slice(data);
    let src: &[f32] = snapshot;
    let (wi, hi) = (size.width as isize, size.height as isize);
    data.par_chunks_mut(size.width).enumerate().for_each_init(
        || Vec::<f32>::with_capacity(25),
        |buf, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask.get_at(Vec2us::new(x, y)) {
                    continue;
                }
                buf.clear();
                for dy in -2..=2 {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -2..=2 {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        let j = size.index_of(Vec2us::new(xx, yy));
                        if !mask.get(j) {
                            buf.push(src[j]);
                        }
                    }
                }
                if !buf.is_empty() {
                    *o = median_f32_mut(buf);
                }
            }
        },
    );
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::stacking::calibration_masters::cosmic_ray::config::CosmicRayConfig;
    use crate::stacking::calibration_masters::cosmic_ray::mono::{MonoDetector, MonoScratch};

    /// Frame-sized `f32` planes the mono detector holds, however many iterations it runs.
    pub(crate) const MONO_SCRATCH_PLANES: usize = 5;

    /// Total capacity, in floats, of the mono detector's working set after a run on `data` — what
    /// `mem_budget` weighs against [`MONO_SCRATCH_PLANES`].
    ///
    /// Destructured rather than summed through a helper, so a plane added to or dropped from
    /// [`MonoScratch`] fails to compile here instead of silently drifting from the constant.
    pub(crate) fn mono_scratch_floats(
        data: &mut [f32],
        size: Size2us,
        config: &CosmicRayConfig,
    ) -> usize {
        let mut detector = MonoDetector::new(config);
        detector.reject(data, size);
        let MonoScratch {
            significance,
            fine,
            noise,
            median,
            frame,
        } = &detector.scratch;
        significance.capacity()
            + fine.capacity()
            + noise.capacity()
            + median.capacity()
            + frame.capacity()
    }
}
