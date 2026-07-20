//! RCD (Ratio Corrected Demosaicing) algorithm.
//!
//! Based on: Luis Sanz Rodriguez, "Ratio Corrected Demosaicing" v2.3 (2017).
//! Reference: <https://github.com/LuisSR/RCD-Demosaicing>
//!
//! The algorithm uses directional discrimination and ratio-corrected
//! interpolation in a low-pass filter domain to reduce color artifacts,
//! particularly beneficial for astrophotography (star morphology). Near a signed
//! low-pass denominator cancellation, the ratio estimate blends continuously into
//! an additive midpoint estimate.

use common::CancelToken;
use rayon::prelude::*;

use crate::concurrency::UnsafeSendPtr;
use crate::io::raw::demosaic::Cancelled;
use crate::io::raw::{
    alloc_uninit_vec,
    demosaic::bayer::{BayerImage, CfaPattern},
};

const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;
// Limit the ratio's relative condition number to four before blending.
const MIN_SIGNED_DENOMINATOR_RATIO: f32 = 0.25;
/// Border size required by the algorithm (pixels on each side).
const BORDER: usize = 4;

/// Pre-computed row stride multiples used throughout the algorithm.
#[derive(Debug, Clone, Copy)]
struct Strides {
    rw: usize,
    rh: usize,
    w1: usize,
    w2: usize,
    w3: usize,
}

impl Strides {
    fn new(rw: usize, rh: usize) -> Self {
        Self {
            rw,
            rh,
            w1: rw,
            w2: 2 * rw,
            w3: 3 * rw,
        }
    }
}

/// Linear interpolation: `(1 - a) * b + a * c`.
#[inline(always)]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    b + a * (c - b)
}

#[inline(always)]
fn estimate_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
    let numerator = neighbor_green * (center_lpf + center_lpf);
    let denominator = EPS + center_lpf + same_color_lpf;
    if center_lpf >= 0.0 && same_color_lpf >= 0.0 {
        return numerator / denominator;
    }

    let scale = EPS + center_lpf.abs() + same_color_lpf.abs();
    let transition = MIN_SIGNED_DENOMINATOR_RATIO * scale;
    if denominator.abs() >= transition {
        return numerator / denominator;
    }

    // Same-color LPFs are two pixels apart; midpoint correction halves their 4× gain.
    let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
    let t = denominator.abs() / transition;
    let curve = t * (3.0 - 2.0 * t);
    let ratio_weight = t * curve;
    // Fold the reciprocal into smoothstep so exact cancellation cannot form 0/0.
    let weighted_ratio = numerator * denominator.signum() * curve / transition;
    additive * (1.0 - ratio_weight) + weighted_ratio
}

/// Mean of the four diagonal neighbours of `idx` (stride `w1`): the direction-discriminator's
/// local average, used to pull a pixel's V/H (or P/Q) direction estimate toward its neighbourhood.
#[inline(always)]
fn avg4_diag(buf: &[f32], idx: usize, w1: usize) -> f32 {
    0.25 * (buf[idx - w1 - 1] + buf[idx - w1 + 1] + buf[idx + w1 - 1] + buf[idx + w1 + 1])
}

/// RCD demosaic implementation.
///
/// Input: Bayer data and margin info. Calibrated samples may be outside `[0, 1]`.
/// Output: planar RGB f32 channels for the active area (width × height).
pub(crate) fn rcd_demosaic(
    bayer: &BayerImage,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let width = bayer.width;
    let height = bayer.height;
    let rw = bayer.raw_width;
    let rh = bayer.raw_height;
    let tm = bayer.top_margin;
    let lm = bayer.left_margin;
    let cfa = bayer.data;
    let pattern = bayer.raw_cfa_pattern;
    let npix = rw * rh;

    // Cooperative cancel: each stage below is a full-image parallel pass. A
    // check between stages lets a cancelled run bail within one stage (~tens of
    // ms) instead of finishing the whole demosaic; the partial buffers are
    // dropped and `Cancelled` propagates to the caller.

    let s = Strides::new(rw, rh);
    let w1 = s.w1;
    let w2 = s.w2;
    let w3 = s.w3;
    let w4 = 4 * rw;

    // Allocate working buffers in raw coordinate space.
    // RGB channels are zero-initialized because CFA copy only writes matching positions.
    let mut vh_dir = vec![0.0f32; npix];
    let mut rgb_r = vec![0.0f32; npix];
    let mut rgb_g = vec![0.0f32; npix];
    let mut rgb_b = vec![0.0f32; npix];

    // Single parallel dispatch for all 3 channels (avoids 3 rayon syncs).
    {
        let ptr_r = UnsafeSendPtr::new(rgb_r.as_mut_ptr());
        let ptr_g = UnsafeSendPtr::new(rgb_g.as_mut_ptr());
        let ptr_b = UnsafeSendPtr::new(rgb_b.as_mut_ptr());
        (0..rh).into_par_iter().for_each(|ry| {
            let ptrs = [ptr_r.get(), ptr_g.get(), ptr_b.get()];
            for rx in 0..rw {
                let c = pattern.color_at(ry, rx);
                // SAFETY: Each (ry, rx) maps to a unique index ry*rw+rx.
                // Only one channel is written per pixel, so no data races.
                unsafe {
                    *ptrs[c].add(ry * rw + rx) = cfa[ry * rw + rx];
                }
            }
        });
    }

    // Computes V-HPF², H-HPF², and VH_Dir in a single parallel pass per row.
    // Each row computes its own H-HPF² inline and reads V-HPF² from ±1 neighbor
    // rows via a full V-HPF buffer. This eliminates two extra full-image passes.

    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let mut v_hpf = vec![0.0f32; npix];

    // Step 1a: Compute V-HPF² for all rows (needed by Step 1b for ±1 row reads).
    v_hpf.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
        if ry < 3 || ry + 3 >= rh {
            return;
        }
        for (rx, val) in row.iter_mut().enumerate() {
            let idx = ry * rw + rx;
            let v = (cfa[idx - w3] - cfa[idx - w1] - cfa[idx + w1] + cfa[idx + w3])
                - 3.0 * (cfa[idx - w2] + cfa[idx + w2])
                + 6.0 * cfa[idx];
            *val = v * v;
        }
    });

    // Step 1b: Fused H-HPF² + VH_Dir computation. H-HPF² is computed per-row
    // inline (no extra buffer), combined with V-HPF² from v_hpf to produce VH_Dir.
    vh_dir
        .par_chunks_mut(rw)
        .enumerate()
        .for_each(|(ry, vh_row)| {
            if ry < BORDER || ry + BORDER >= rh {
                return;
            }
            // Inline H-HPF² computation (no per-row allocation).
            let h_hpf_sq = |rx: usize| -> f32 {
                let idx = ry * rw + rx;
                let v = (cfa[idx - 3] - cfa[idx - 1] - cfa[idx + 1] + cfa[idx + 3])
                    - 3.0 * (cfa[idx - 2] + cfa[idx + 2])
                    + 6.0 * cfa[idx];
                v * v
            };
            // Sliding window of 3 H-HPF² values to avoid per-row buffer.
            let mut h_prev = h_hpf_sq(BORDER - 1);
            let mut h_curr = h_hpf_sq(BORDER);
            for rx in BORDER..rw.saturating_sub(BORDER) {
                let h_next = h_hpf_sq(rx + 1);
                let v_stat =
                    (v_hpf[(ry - 1) * rw + rx] + v_hpf[ry * rw + rx] + v_hpf[(ry + 1) * rw + rx])
                        .max(EPSSQ);
                let h_stat = (h_prev + h_curr + h_next).max(EPSSQ);
                vh_row[rx] = v_stat / (v_stat + h_stat);
                h_prev = h_curr;
                h_curr = h_next;
            }
        });

    // v_hpf is dead — reuse buffer for LPF (Step 2–3), then PQ-Dir (Step 4).
    let mut scratch = v_hpf;

    // 3×3 weighted average used by Step 3 for ratio-corrected estimation.
    // Reuses scratch buffer (was v_hpf, same npix size).

    // No fill needed: Step 2 writes all positions that Step 3 reads
    // (rows 1..rh-1, cols 1..rw-1; Step 3 reads within BORDER=4 margin).
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let lpf = &mut scratch;
    lpf.par_chunks_mut(rw)
        .enumerate()
        .for_each(|(ry, lpf_row)| {
            if ry < 1 || ry + 1 >= rh {
                return;
            }
            for (rx, val) in lpf_row
                .iter_mut()
                .enumerate()
                .skip(1)
                .take(rw.saturating_sub(2))
            {
                let i = ry * rw + rx;
                *val = cfa[i]
                    + 0.5 * (cfa[i - rw] + cfa[i + rw] + cfa[i - 1] + cfa[i + 1])
                    + 0.25
                        * (cfa[i - rw - 1] + cfa[i - rw + 1] + cfa[i + rw - 1] + cfa[i + rw + 1]);
            }
        });

    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    rgb_g
        .par_chunks_mut(rw)
        .enumerate()
        .for_each(|(ry, green_row)| {
            if ry < BORDER || ry + BORDER >= rh {
                return;
            }

            let col_start = BORDER + (pattern.color_at(ry, 0) & 1);
            let mut rx = col_start;
            while rx < rw.saturating_sub(BORDER) {
                let idx = ry * rw + rx;

                let cfai = cfa[idx];
                let n_grad = EPS
                    + (cfa[idx - w1] - cfa[idx + w1]).abs()
                    + (cfai - cfa[idx - w2]).abs()
                    + (cfa[idx - w1] - cfa[idx - w3]).abs()
                    + (cfa[idx - w2] - cfa[idx - w4]).abs();
                let s_grad = EPS
                    + (cfa[idx - w1] - cfa[idx + w1]).abs()
                    + (cfai - cfa[idx + w2]).abs()
                    + (cfa[idx + w1] - cfa[idx + w3]).abs()
                    + (cfa[idx + w2] - cfa[idx + w4]).abs();
                let w_grad = EPS
                    + (cfa[idx - 1] - cfa[idx + 1]).abs()
                    + (cfai - cfa[idx - 2]).abs()
                    + (cfa[idx - 1] - cfa[idx - 3]).abs()
                    + (cfa[idx - 2] - cfa[idx - 4]).abs();
                let e_grad = EPS
                    + (cfa[idx - 1] - cfa[idx + 1]).abs()
                    + (cfai - cfa[idx + 2]).abs()
                    + (cfa[idx + 1] - cfa[idx + 3]).abs()
                    + (cfa[idx + 2] - cfa[idx + 4]).abs();

                let lpfi = lpf[idx];
                let n_est = estimate_green(cfa[idx - w1], lpfi, lpf[idx - w2]);
                let s_est = estimate_green(cfa[idx + w1], lpfi, lpf[idx + w2]);
                let w_est = estimate_green(cfa[idx - 1], lpfi, lpf[idx - 2]);
                let e_est = estimate_green(cfa[idx + 1], lpfi, lpf[idx + 2]);

                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

                let vh_central = vh_dir[idx];
                let vh_neighbourhood = avg4_diag(&vh_dir, idx, w1);
                let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neighbourhood).abs() {
                    vh_neighbourhood
                } else {
                    vh_central
                };

                green_row[rx] = intp(vh_disc, v_est, h_est);

                rx += 2;
            }
        });

    // End LPF borrow so scratch is reusable.
    let _ = lpf;

    // Step 4.0-4.1: P/Q diagonal direction detection.
    // Reuse scratch buffer for pq_dir.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    scratch.fill(0.0);
    let mut pq_dir = scratch;

    {
        let half_w = rw.div_ceil(2);
        let half_size = half_w * rh;
        let mut p_hpf = vec![0.0f32; half_size];
        let mut q_hpf = vec![0.0f32; half_size];

        // Compute P and Q HPF in parallel by row.
        p_hpf
            .par_chunks_mut(half_w)
            .zip(q_hpf.par_chunks_mut(half_w))
            .enumerate()
            .for_each(|(ry, (p_row, q_row))| {
                if ry < 3 || ry + 3 >= rh {
                    return;
                }
                let col_start = 3 + ((ry + 1) & 1);
                let mut rx = col_start;
                while rx < rw.saturating_sub(3) {
                    let idx = ry * rw + rx;
                    let hx = rx / 2;

                    let p_val = (cfa[idx - w3 - 3] - cfa[idx - w1 - 1] - cfa[idx + w1 + 1]
                        + cfa[idx + w3 + 3])
                        - 3.0 * (cfa[idx - w2 - 2] + cfa[idx + w2 + 2])
                        + 6.0 * cfa[idx];
                    p_row[hx] = p_val * p_val;

                    let q_val = (cfa[idx - w3 + 3] - cfa[idx - w1 + 1] - cfa[idx + w1 - 1]
                        + cfa[idx + w3 - 3])
                        - 3.0 * (cfa[idx - w2 + 2] + cfa[idx + w2 - 2])
                        + 6.0 * cfa[idx];
                    q_row[hx] = q_val * q_val;

                    rx += 2;
                }
            });

        // Compute PQ_Dir in parallel by row.
        pq_dir
            .par_chunks_mut(rw)
            .enumerate()
            .for_each(|(ry, pq_row)| {
                if ry < BORDER || ry + BORDER >= rh {
                    return;
                }
                let col_start = BORDER + (pattern.color_at(ry, 0) & 1);
                let mut rx = col_start;
                while rx < rw.saturating_sub(BORDER) {
                    let h_center = ry * half_w + rx / 2;
                    let h_nw = (ry - 1) * half_w + (rx - 1) / 2;
                    let h_se = (ry + 1) * half_w + rx.div_ceil(2);
                    let h_ne = (ry - 1) * half_w + rx.div_ceil(2);
                    let h_sw = (ry + 1) * half_w + (rx - 1) / 2;

                    let p_stat = (p_hpf[h_nw] + p_hpf[h_center] + p_hpf[h_se]).max(EPSSQ);
                    let q_stat = (q_hpf[h_ne] + q_hpf[h_center] + q_hpf[h_sw]).max(EPSSQ);

                    pq_row[rx] = p_stat / (p_stat + q_stat);

                    rx += 2;
                }
            });
    }

    // Step 4.2: Red/Blue at opposing CFA positions.
    // At R positions: interpolate B. At B positions: interpolate R.
    // Each row only reads from rgb[c] at neighboring rows (which hold the original
    // CFA values, not values written in this step) and rgb_g, so rows are independent.
    //
    // We need mutable access to rgb_r and rgb_b while reading rgb_g.
    // Since Step 4.2 only writes to one of rgb_r/rgb_b at R/B positions, and we
    // need to read the other, we process R-rows and B-rows with the appropriate channel.
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    step4_2_rb_at_opposing(&mut rgb_r, &mut rgb_b, &rgb_g, &pq_dir, &pattern, s);

    // Step 4.3: Red/Blue at Green CFA positions.
    // Reads rgb_r, rgb_b, rgb_g, vh_dir. Writes rgb_r and rgb_b at green positions.
    // Each row's green positions only read from neighboring rows' R/B values
    // (set in CFA copy or Step 4.2), so rows are independent.
    step4_3_rb_at_green(&mut rgb_r, &mut rgb_b, &rgb_g, &vh_dir, &pattern, s);

    drop(vh_dir);
    drop(pq_dir);

    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    border_interpolate(&mut rgb_r, &mut rgb_b, &mut rgb_g, cfa, &pattern, s);

    // Per-row contiguous copy (margins cropped); cheaper than the interleaved
    // scatter and lets the caller take the buffers zero-copy.

    let active = width * height;
    // SAFETY: the per-row copy_from_slice below writes every element of each buffer.
    let mut out_r = unsafe { alloc_uninit_vec::<f32>(active) };
    let mut out_g = unsafe { alloc_uninit_vec::<f32>(active) };
    let mut out_b = unsafe { alloc_uninit_vec::<f32>(active) };

    out_r
        .par_chunks_mut(width)
        .zip(out_g.par_chunks_mut(width))
        .zip(out_b.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, ((r_row, g_row), b_row))| {
            let base = (tm + y) * rw + lm;
            r_row.copy_from_slice(&rgb_r[base..base + width]);
            g_row.copy_from_slice(&rgb_g[base..base + width]);
            b_row.copy_from_slice(&rgb_b[base..base + width]);
        });

    Ok([out_r, out_g, out_b])
}

/// Step 4.2: Interpolate the missing color at R/B CFA positions.
///
/// At R positions we interpolate B, and at B positions we interpolate R,
/// using diagonal color differences guided by PQ direction.
fn step4_2_rb_at_opposing(
    rgb_r: &mut [f32],
    rgb_b: &mut [f32],
    rgb_g: &[f32],
    pq_dir: &[f32],
    pattern: &CfaPattern,
    s: Strides,
) {
    let rh = s.rh;

    // On R-rows we write rgb_b (reading rgb_b for diagonal neighbors which are CFA originals).
    // On B-rows we write rgb_r (reading rgb_r for diagonal neighbors which are CFA originals).
    // Each pass: the destination channel's values at neighbor positions were set in CFA copy
    // and are never written in this step, so reads from other rows are safe.

    // Single dispatch: R-rows write B, B-rows write R. Rows are independent
    // (each writes only to its own indices in the destination buffer).
    let dst_r = UnsafeSendPtr::new(rgb_r.as_mut_ptr());
    let dst_b = UnsafeSendPtr::new(rgb_b.as_mut_ptr());
    (BORDER..rh.saturating_sub(BORDER))
        .into_par_iter()
        .for_each(|ry| {
            let col_start = BORDER + (pattern.color_at(ry, 0) & 1);
            let color = pattern.color_at(ry, col_start);
            let ptr = match color {
                0 => dst_b.get(), // R-row → write B
                2 => dst_r.get(), // B-row → write R
                _ => return,      // G-row → skip
            };
            // SAFETY: Rows write disjoint indices. Diagonal reads land only on native CFA samples,
            // which this phase never writes.
            unsafe {
                process_step4_2_row(ptr, rgb_g, pq_dir, ry, col_start, s);
            }
        });
}

/// Process one row of Step 4.2.
/// Values at diagonal neighbors were set during CFA copy and are not modified by this step.
unsafe fn process_step4_2_row(
    dst: *mut f32,
    rgb_g: &[f32],
    pq_dir: &[f32],
    ry: usize,
    col_start: usize,
    s: Strides,
) {
    let Strides { rw, w1, w2, w3, .. } = s;
    let read = |index: usize| {
        // SAFETY: The caller guarantees that `dst` covers the complete `rw * rh` channel.
        unsafe { dst.add(index).read() }
    };
    let mut rx = col_start;
    while rx < rw.saturating_sub(BORDER) {
        let idx = ry * rw + rx;

        let pq_central = pq_dir[idx];
        let pq_neighbourhood = avg4_diag(pq_dir, idx, w1);
        let pq_disc = if (0.5 - pq_central).abs() < (0.5 - pq_neighbourhood).abs() {
            pq_neighbourhood
        } else {
            pq_central
        };

        let nw_grad = EPS
            + (read(idx - w1 - 1) - read(idx + w1 + 1)).abs()
            + (read(idx - w1 - 1) - read(idx - w3 - 3)).abs()
            + (rgb_g[idx] - rgb_g[idx - w2 - 2]).abs();
        let ne_grad = EPS
            + (read(idx - w1 + 1) - read(idx + w1 - 1)).abs()
            + (read(idx - w1 + 1) - read(idx - w3 + 3)).abs()
            + (rgb_g[idx] - rgb_g[idx - w2 + 2]).abs();
        let sw_grad = EPS
            + (read(idx - w1 + 1) - read(idx + w1 - 1)).abs()
            + (read(idx + w1 - 1) - read(idx + w3 - 3)).abs()
            + (rgb_g[idx] - rgb_g[idx + w2 - 2]).abs();
        let se_grad = EPS
            + (read(idx - w1 - 1) - read(idx + w1 + 1)).abs()
            + (read(idx + w1 + 1) - read(idx + w3 + 3)).abs()
            + (rgb_g[idx] - rgb_g[idx + w2 + 2]).abs();

        let nw_est = read(idx - w1 - 1) - rgb_g[idx - w1 - 1];
        let ne_est = read(idx - w1 + 1) - rgb_g[idx - w1 + 1];
        let sw_est = read(idx + w1 - 1) - rgb_g[idx + w1 - 1];
        let se_est = read(idx + w1 + 1) - rgb_g[idx + w1 + 1];

        let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
        let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

        let value = rgb_g[idx] + intp(pq_disc, p_est, q_est);
        // SAFETY: This row owns `idx`; no other phase worker reads or writes missing-color sites.
        unsafe { dst.add(idx).write(value) };

        rx += 2;
    }
}

/// Step 4.3: Interpolate R and B at green CFA positions.
fn step4_3_rb_at_green(
    rgb_r: &mut [f32],
    rgb_b: &mut [f32],
    rgb_g: &[f32],
    vh_dir: &[f32],
    pattern: &CfaPattern,
    s: Strides,
) {
    let Strides { rw, rh, w1, w2, w3 } = s;
    // At green positions, we need to interpolate both R and B.
    // The R and B values at cardinal neighbors are either CFA originals or
    // were written in Step 4.2. Since we only write at green positions and
    // read from R/B positions, there are no write-read conflicts between rows.
    //
    // Neighbor reads span rows, so raw pointers avoid creating overlapping mutable slices.
    let ptr_r = UnsafeSendPtr::new(rgb_r.as_mut_ptr());
    let ptr_b = UnsafeSendPtr::new(rgb_b.as_mut_ptr());

    (BORDER..rh.saturating_sub(BORDER))
        .into_par_iter()
        .for_each(|ry| {
            let r_ptr = ptr_r.get();
            let b_ptr = ptr_b.get();
            let read_r = |index: usize| {
                // SAFETY: Cardinal reads land only on native or Step 4.2 R/B sites.
                unsafe { r_ptr.add(index).read() }
            };
            let read_b = |index: usize| {
                // SAFETY: Cardinal reads land only on native or Step 4.2 R/B sites.
                unsafe { b_ptr.add(index).read() }
            };

            let col_start = BORDER + (pattern.color_at(ry, 1) & 1);
            let mut rx = col_start;
            while rx < rw.saturating_sub(BORDER) {
                let idx = ry * rw + rx;

                let vh_central = vh_dir[idx];
                let vh_neighbourhood = avg4_diag(vh_dir, idx, w1);
                let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neighbourhood).abs() {
                    vh_neighbourhood
                } else {
                    vh_central
                };

                let g_center = rgb_g[idx];
                let n1 = EPS + (g_center - rgb_g[idx - w2]).abs();
                let s1 = EPS + (g_center - rgb_g[idx + w2]).abs();
                let w1_val = EPS + (g_center - rgb_g[idx - 2]).abs();
                let e1_val = EPS + (g_center - rgb_g[idx + 2]).abs();

                // Interpolate R
                {
                    let sn_abs = (read_r(idx - w1) - read_r(idx + w1)).abs();
                    let ew_abs = (read_r(idx - 1) - read_r(idx + 1)).abs();
                    let n_grad = n1 + sn_abs + (read_r(idx - w1) - read_r(idx - w3)).abs();
                    let s_grad = s1 + sn_abs + (read_r(idx + w1) - read_r(idx + w3)).abs();
                    let w_grad = w1_val + ew_abs + (read_r(idx - 1) - read_r(idx - 3)).abs();
                    let e_grad = e1_val + ew_abs + (read_r(idx + 1) - read_r(idx + 3)).abs();

                    let n_est = read_r(idx - w1) - rgb_g[idx - w1];
                    let s_est = read_r(idx + w1) - rgb_g[idx + w1];
                    let w_est = read_r(idx - 1) - rgb_g[idx - 1];
                    let e_est = read_r(idx + 1) - rgb_g[idx + 1];

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                    let value = g_center + intp(vh_disc, v_est, h_est);
                    // SAFETY: Each worker owns the green sites in its row.
                    unsafe { r_ptr.add(idx).write(value) };
                }

                // Interpolate B
                {
                    let sn_abs = (read_b(idx - w1) - read_b(idx + w1)).abs();
                    let ew_abs = (read_b(idx - 1) - read_b(idx + 1)).abs();
                    let n_grad = n1 + sn_abs + (read_b(idx - w1) - read_b(idx - w3)).abs();
                    let s_grad = s1 + sn_abs + (read_b(idx + w1) - read_b(idx + w3)).abs();
                    let w_grad = w1_val + ew_abs + (read_b(idx - 1) - read_b(idx - 3)).abs();
                    let e_grad = e1_val + ew_abs + (read_b(idx + 1) - read_b(idx + 3)).abs();

                    let n_est = read_b(idx - w1) - rgb_g[idx - w1];
                    let s_est = read_b(idx + w1) - rgb_g[idx + w1];
                    let w_est = read_b(idx - 1) - rgb_g[idx - 1];
                    let e_est = read_b(idx + 1) - rgb_g[idx + 1];

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                    let value = g_center + intp(vh_disc, v_est, h_est);
                    // SAFETY: Each worker owns the green sites in its row.
                    unsafe { b_ptr.add(idx).write(value) };
                }

                rx += 2;
            }
        });
}

/// Simple bilinear border interpolation for pixels within `border` pixels of the edge.
fn border_interpolate(
    rgb_r: &mut [f32],
    rgb_b: &mut [f32],
    rgb_g: &mut [f32],
    cfa: &[f32],
    pattern: &CfaPattern,
    s: Strides,
) {
    let Strides {
        rw: width,
        rh: height,
        ..
    } = s;
    let border = BORDER;
    let rgb_channels: [&mut [f32]; 3] = [rgb_r, rgb_g, rgb_b];

    for (ic, rgb_ch) in rgb_channels.into_iter().enumerate() {
        // Only iterate actual border pixels: top/bottom bands + left/right edges.
        // This avoids walking the entire image just to skip interior pixels.
        let mut border_pixel = |ry: usize, rx: usize| {
            let idx = ry * width + rx;
            let c = pattern.color_at(ry, rx);
            if ic == c {
                rgb_ch[idx] = cfa[idx];
                return;
            }
            let mut sum = 0.0f32;
            let mut count = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dy == 0 && dx == 0 {
                        continue;
                    }
                    let ny = ry as i32 + dy;
                    let nx = rx as i32 + dx;
                    if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                        let nidx = ny as usize * width + nx as usize;
                        if pattern.color_at(ny as usize, nx as usize) == ic {
                            sum += cfa[nidx];
                            count += 1;
                        }
                    }
                }
            }
            if count > 0 {
                rgb_ch[idx] = sum / count as f32;
            } else {
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let ny = ry as i32 + dy;
                        let nx = rx as i32 + dx;
                        if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                            let nidx = ny as usize * width + nx as usize;
                            if pattern.color_at(ny as usize, nx as usize) == ic {
                                sum += cfa[nidx];
                                count += 1;
                            }
                        }
                    }
                }
                rgb_ch[idx] = if count > 0 { sum / count as f32 } else { 0.0 };
            }
        };

        // Top border band (full rows).
        for ry in 0..border {
            for rx in 0..width {
                border_pixel(ry, rx);
            }
        }
        // Bottom border band (full rows).
        for ry in height.saturating_sub(border)..height {
            for rx in 0..width {
                border_pixel(ry, rx);
            }
        }
        // Left and right edges on interior rows.
        for ry in border..height.saturating_sub(border) {
            for rx in 0..border {
                border_pixel(ry, rx);
            }
            for rx in width.saturating_sub(border)..width {
                border_pixel(ry, rx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::io::raw::demosaic::bayer::rcd::{EPS, MIN_SIGNED_DENOMINATOR_RATIO, estimate_green};

    fn canonical_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
        neighbor_green * (center_lpf + center_lpf) / (EPS + center_lpf + same_color_lpf)
    }

    #[test]
    fn well_conditioned_green_estimate_matches_canonical_ratio() {
        for neighbor_green in [-0.75, 0.0, 1.5] {
            for center_lpf in [0.0, EPS * 0.5, EPS, 4.0] {
                for same_color_lpf in [0.0, EPS * 0.5, EPS, 8.0] {
                    assert_eq!(
                        estimate_green(neighbor_green, center_lpf, same_color_lpf),
                        canonical_green(neighbor_green, center_lpf, same_color_lpf)
                    );
                }
            }
        }

        for (neighbor_green, center_lpf, same_color_lpf) in [(0.25, 1.0, -0.5), (-0.75, -4.0, -2.0)]
        {
            assert_eq!(
                estimate_green(neighbor_green, center_lpf, same_color_lpf),
                canonical_green(neighbor_green, center_lpf, same_color_lpf)
            );
        }
    }

    #[test]
    fn cancelling_green_estimate_has_the_additive_limit() {
        let center_lpf = 1.0;
        let same_color_lpf = -(EPS + center_lpf);
        let actual = estimate_green(0.25, center_lpf, same_color_lpf);
        let expected = 0.25 + (1.0 - same_color_lpf) / 8.0;
        assert_eq!(actual, expected);
    }

    #[test]
    fn cancelling_green_estimate_blends_halfway_at_half_the_condition_limit() {
        let neighbor_green = 0.25;
        let center_lpf = 1.0;
        let condition = 0.5 * MIN_SIGNED_DENOMINATOR_RATIO;
        let same_color_lpf = -(EPS + center_lpf) * (1.0 - condition) / (1.0 + condition);
        let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
        let canonical = canonical_green(neighbor_green, center_lpf, same_color_lpf);
        let expected = 0.5 * (additive + canonical);
        let actual = estimate_green(neighbor_green, center_lpf, same_color_lpf);

        assert!((actual - expected).abs() < 1e-6);
    }

    #[derive(Debug, Clone, Copy)]
    enum Neighborhood {
        Edge,
        Impulse,
        ChromaticStar,
        Noise,
    }

    fn neighborhood_value(neighborhood: Neighborhood, channel: usize, x: usize, y: usize) -> f32 {
        match neighborhood {
            Neighborhood::Edge => {
                if x < 4 {
                    [0.7, 0.4, 0.2][channel]
                } else {
                    [0.1, 0.3, 0.8][channel]
                }
            }
            Neighborhood::Impulse => {
                let impulse = if x == 4 && y == 2 { 0.9 } else { 0.0 };
                [0.05 + impulse, 0.08, 0.03][channel]
            }
            Neighborhood::ChromaticStar => {
                let dx = x as f32 - 3.5;
                let dy = y as f32 - 2.0;
                let profile = (-0.5 * (dx * dx + dy * dy)).exp();
                0.02 + [0.9, 0.5, 0.2][channel] * profile
            }
            Neighborhood::Noise => {
                let sample = (x * 17 + y * 29 + channel * 11) % 31;
                0.02 + sample as f32 / 62.0
            }
        }
    }

    fn neighborhood_lpf(cfa: &[f32], width: usize, y: usize, x: usize) -> f32 {
        let index = y * width + x;
        cfa[index]
            + 0.5 * (cfa[index - width] + cfa[index + width] + cfa[index - 1] + cfa[index + 1])
            + 0.25
                * (cfa[index - width - 1]
                    + cfa[index - width + 1]
                    + cfa[index + width - 1]
                    + cfa[index + width + 1])
    }

    fn reference_signed_green(neighbor_green: f32, center_lpf: f32, same_color_lpf: f32) -> f32 {
        let neighbor_green = f64::from(neighbor_green);
        let center_lpf = f64::from(center_lpf);
        let same_color_lpf = f64::from(same_color_lpf);
        let epsilon = f64::from(EPS);
        let numerator = neighbor_green * (center_lpf + center_lpf);
        let denominator = epsilon + center_lpf + same_color_lpf;
        if center_lpf >= 0.0 && same_color_lpf >= 0.0 {
            return (numerator / denominator) as f32;
        }

        let scale = epsilon + center_lpf.abs() + same_color_lpf.abs();
        let transition = f64::from(MIN_SIGNED_DENOMINATOR_RATIO) * scale;
        if denominator.abs() >= transition {
            return (numerator / denominator) as f32;
        }

        let additive = neighbor_green + (center_lpf - same_color_lpf) * 0.125;
        let t = denominator.abs() / transition;
        let curve = t * (3.0 - 2.0 * t);
        let ratio_weight = t * curve;
        let weighted_ratio = numerator * denominator.signum() * curve / transition;
        (additive * (1.0 - ratio_weight) + weighted_ratio) as f32
    }

    #[test]
    fn signed_scene_neighborhoods_match_f64_reference_across_zero() {
        const WIDTH: usize = 7;
        const HEIGHT: usize = 5;
        const CENTER_X: usize = 4;
        const SAME_COLOR_X: usize = 2;
        const Y: usize = 2;

        let pattern = CfaPattern::Rggb;
        for neighborhood in [
            Neighborhood::Edge,
            Neighborhood::Impulse,
            Neighborhood::ChromaticStar,
            Neighborhood::Noise,
        ] {
            let cfa: Vec<f32> = (0..HEIGHT)
                .flat_map(|y| {
                    (0..WIDTH).map(move |x| {
                        neighborhood_value(neighborhood, pattern.color_at(y, x), x, y)
                    })
                })
                .collect();
            let center_lpf = neighborhood_lpf(&cfa, WIDTH, Y, CENTER_X);
            let same_color_lpf = neighborhood_lpf(&cfa, WIDTH, Y, SAME_COLOR_X);
            let neighbor_green = cfa[Y * WIDTH + 3];

            for (label, anchor) in [("center", center_lpf), ("same-color", same_color_lpf)] {
                for target in [-2.0 * EPS, -EPS, -0.5 * EPS, 0.0, 0.5 * EPS, EPS, 2.0 * EPS] {
                    let pedestal = (target - anchor) * 0.25;
                    let shifted_neighbor = neighbor_green + pedestal;
                    let shifted_center = center_lpf + 4.0 * pedestal;
                    let shifted_same_color = same_color_lpf + 4.0 * pedestal;
                    let actual =
                        estimate_green(shifted_neighbor, shifted_center, shifted_same_color);
                    let expected = reference_signed_green(
                        shifted_neighbor,
                        shifted_center,
                        shifted_same_color,
                    );
                    let tolerance = 2e-6 * expected.abs().max(1.0);
                    assert!(
                        (actual - expected).abs() <= tolerance,
                        "{neighborhood:?} {label} LPF={target}: expected {expected}, got {actual}"
                    );
                }
            }
        }
    }
}
