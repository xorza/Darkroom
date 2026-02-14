//! RCD (Ratio Corrected Demosaicing) algorithm.
//!
//! Based on: Luis Sanz Rodriguez, "Ratio Corrected Demosaicing" v2.3 (2017).
//! Reference: <https://github.com/LuisSR/RCD-Demosaicing>
//!
//! The algorithm uses directional discrimination and ratio-corrected
//! interpolation in a low-pass filter domain to reduce color artifacts,
//! particularly beneficial for astrophotography (star morphology).

use rayon::prelude::*;

use super::{BayerImage, CfaPattern};

const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;
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

// SAFETY: Caller must ensure no data races (each thread writes to unique indices).
#[derive(Clone, Copy)]
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

impl SendPtr {
    fn get(self) -> *mut f32 {
        self.0
    }
}

/// RCD demosaic implementation.
///
/// Input: BayerImage with normalized [0,1] CFA data and margin info.
/// Output: Interleaved RGB f32 pixels `[R0, G0, B0, R1, G1, B1, ...]`
/// for the active area (width × height).
pub(super) fn rcd_demosaic(bayer: &BayerImage) -> Vec<f32> {
    let width = bayer.width;
    let height = bayer.height;
    let rw = bayer.raw_width;
    let rh = bayer.raw_height;
    let tm = bayer.top_margin;
    let lm = bayer.left_margin;
    let cfa = bayer.data;
    let pattern = bayer.cfa;
    let npix = rw * rh;

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

    // ── Copy CFA values to the appropriate RGB channel ───────────────────

    let rgb_slices: [&mut [f32]; 3] = [&mut rgb_r, &mut rgb_g, &mut rgb_b];
    rgb_slices.into_iter().enumerate().for_each(|(ch, rgb_ch)| {
        rgb_ch.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
            for (rx, pixel) in row.iter_mut().enumerate() {
                if pattern.color_at(ry, rx) == ch {
                    *pixel = cfa[ry * rw + rx];
                }
            }
        });
    });

    // ── Step 1: Fused V/H Direction Detection ───────────────────────────
    // Computes V-HPF², H-HPF², and VH_Dir in a single parallel pass per row.
    // Each row computes its own H-HPF² inline and reads V-HPF² from ±1 neighbor
    // rows via a full V-HPF buffer. This eliminates two extra full-image passes.

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

    // v_hpf is dead — reuse as scratch for PQ-Dir later.
    let mut scratch = v_hpf;
    let mut lpf = vec![0.0f32; npix];

    // ── Step 2: Low-Pass Filter ──────────────────────────────────────────

    lpf.par_chunks_mut(rw)
        .enumerate()
        .for_each(|(ry, lpf_row)| {
            if ry < 2 || ry + 2 >= rh {
                return;
            }
            let col_start = 2 + (pattern.color_at(ry, 0) & 1);
            let mut rx = col_start;
            while rx < rw.saturating_sub(2) {
                let idx = ry * rw + rx;
                lpf_row[rx] = cfa[idx]
                    + 0.5 * (cfa[idx - rw] + cfa[idx + rw] + cfa[idx - 1] + cfa[idx + 1])
                    + 0.25
                        * (cfa[idx - rw - 1]
                            + cfa[idx - rw + 1]
                            + cfa[idx + rw - 1]
                            + cfa[idx + rw + 1]);
                rx += 2;
            }
        });

    // ── Step 3: Green Channel Interpolation ──────────────────────────────

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
                let two_lpfi = lpfi + lpfi;
                let n_est = cfa[idx - w1] * two_lpfi / (EPS + lpfi + lpf[idx - w2]);
                let s_est = cfa[idx + w1] * two_lpfi / (EPS + lpfi + lpf[idx + w2]);
                let w_est = cfa[idx - 1] * two_lpfi / (EPS + lpfi + lpf[idx - 2]);
                let e_est = cfa[idx + 1] * two_lpfi / (EPS + lpfi + lpf[idx + 2]);

                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

                let vh_central = vh_dir[idx];
                let vh_neighbourhood = 0.25
                    * (vh_dir[idx - w1 - 1]
                        + vh_dir[idx - w1 + 1]
                        + vh_dir[idx + w1 - 1]
                        + vh_dir[idx + w1 + 1]);
                let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neighbourhood).abs() {
                    vh_neighbourhood
                } else {
                    vh_central
                };

                green_row[rx] = intp(vh_disc, v_est, h_est).clamp(0.0, 1.0);

                rx += 2;
            }
        });

    drop(lpf);

    // ── Step 4: Red and Blue Channel Interpolation ───────────────────────

    // Step 4.0-4.1: P/Q diagonal direction detection.
    // Reuse scratch buffer (was V-HPF, now dead) for pq_dir.
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
    step4_2_rb_at_opposing(&mut rgb_r, &mut rgb_b, &rgb_g, &pq_dir, &pattern, s);

    // Step 4.3: Red/Blue at Green CFA positions.
    // Reads rgb_r, rgb_b, rgb_g, vh_dir. Writes rgb_r and rgb_b at green positions.
    // Each row's green positions only read from neighboring rows' R/B values
    // (set in CFA copy or Step 4.2), so rows are independent.
    step4_3_rb_at_green(&mut rgb_r, &mut rgb_b, &rgb_g, &vh_dir, &pattern, s);

    drop(vh_dir);
    drop(pq_dir);

    // ── Step 5: Border Handling ──────────────────────────────────────────

    border_interpolate(&mut rgb_r, &mut rgb_b, &mut rgb_g, cfa, &pattern, s);

    // ── Extract active area to interleaved RGB output ────────────────────

    let out_size = width * height * 3;
    let mut output = vec![0.0f32; out_size];

    output
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, out_row)| {
            let ry = tm + y;
            for x in 0..width {
                let rx = lm + x;
                let raw_idx = ry * rw + rx;
                let ox = x * 3;
                out_row[ox] = rgb_r[raw_idx];
                out_row[ox + 1] = rgb_g[raw_idx];
                out_row[ox + 2] = rgb_b[raw_idx];
            }
        });

    output
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
    let Strides { rw, rh, .. } = s;

    // On R-rows we write rgb_b (reading rgb_b for diagonal neighbors which are CFA originals).
    // On B-rows we write rgb_r (reading rgb_r for diagonal neighbors which are CFA originals).
    // Each pass: the destination channel's values at neighbor positions were set in CFA copy
    // and are never written in this step, so reads from other rows are safe.

    // Pass 1: R-rows → write B. The dst buffer (rgb_b) is read for neighbors and
    // written at current-row positions only. Each row writes to non-overlapping indices.
    let dst_b = SendPtr(rgb_b.as_mut_ptr());
    (BORDER..rh.saturating_sub(BORDER))
        .into_par_iter()
        .for_each(|ry| {
            let col_start = BORDER + (pattern.color_at(ry, 0) & 1);
            if pattern.color_at(ry, col_start) != 0 {
                return; // Not an R-row
            }
            // SAFETY: Each row writes only to its own indices (ry*rw+col_start, ry*rw+col_start+2, ...).
            // No two parallel iterations share the same ry, so writes don't overlap.
            // Reads from other rows access CFA-original values that are never written in this loop.
            let dst = unsafe { std::slice::from_raw_parts_mut(dst_b.get(), rw * rh) };
            process_step4_2_row(dst, rgb_g, pq_dir, ry, col_start, s);
        });

    // Pass 2: B-rows → write R.
    let dst_r = SendPtr(rgb_r.as_mut_ptr());
    (BORDER..rh.saturating_sub(BORDER))
        .into_par_iter()
        .for_each(|ry| {
            let col_start = BORDER + (pattern.color_at(ry, 0) & 1);
            if pattern.color_at(ry, col_start) != 2 {
                return; // Not a B-row
            }
            // SAFETY: Same reasoning as Pass 1.
            let dst = unsafe { std::slice::from_raw_parts_mut(dst_r.get(), rw * rh) };
            process_step4_2_row(dst, rgb_g, pq_dir, ry, col_start, s);
        });
}

/// Process one row of Step 4.2.
/// `dst` is the full channel buffer being written (the missing color channel).
/// Values at diagonal neighbors were set during CFA copy and are not modified by this step.
fn process_step4_2_row(
    dst: &mut [f32],
    rgb_g: &[f32],
    pq_dir: &[f32],
    ry: usize,
    col_start: usize,
    s: Strides,
) {
    let Strides { rw, w1, w2, w3, .. } = s;
    let mut rx = col_start;
    while rx < rw.saturating_sub(BORDER) {
        let idx = ry * rw + rx;

        let pq_central = pq_dir[idx];
        let pq_neighbourhood = 0.25
            * (pq_dir[idx - w1 - 1]
                + pq_dir[idx - w1 + 1]
                + pq_dir[idx + w1 - 1]
                + pq_dir[idx + w1 + 1]);
        let pq_disc = if (0.5 - pq_central).abs() < (0.5 - pq_neighbourhood).abs() {
            pq_neighbourhood
        } else {
            pq_central
        };

        let nw_grad = EPS
            + (dst[idx - w1 - 1] - dst[idx + w1 + 1]).abs()
            + (dst[idx - w1 - 1] - dst[idx - w3 - 3]).abs()
            + (rgb_g[idx] - rgb_g[idx - w2 - 2]).abs();
        let ne_grad = EPS
            + (dst[idx - w1 + 1] - dst[idx + w1 - 1]).abs()
            + (dst[idx - w1 + 1] - dst[idx - w3 + 3]).abs()
            + (rgb_g[idx] - rgb_g[idx - w2 + 2]).abs();
        let sw_grad = EPS
            + (dst[idx - w1 + 1] - dst[idx + w1 - 1]).abs()
            + (dst[idx + w1 - 1] - dst[idx + w3 - 3]).abs()
            + (rgb_g[idx] - rgb_g[idx + w2 - 2]).abs();
        let se_grad = EPS
            + (dst[idx - w1 - 1] - dst[idx + w1 + 1]).abs()
            + (dst[idx + w1 + 1] - dst[idx + w3 + 3]).abs()
            + (rgb_g[idx] - rgb_g[idx + w2 + 2]).abs();

        let nw_est = dst[idx - w1 - 1] - rgb_g[idx - w1 - 1];
        let ne_est = dst[idx - w1 + 1] - rgb_g[idx - w1 + 1];
        let sw_est = dst[idx + w1 - 1] - rgb_g[idx + w1 - 1];
        let se_est = dst[idx + w1 + 1] - rgb_g[idx + w1 + 1];

        let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
        let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

        dst[idx] = (rgb_g[idx] + intp(pq_disc, p_est, q_est)).clamp(0.0, 1.0);

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
    // We need full-buffer access for neighbor rows (±w1, ±w3), so use SendPtr.
    let ptr_r = SendPtr(rgb_r.as_mut_ptr());
    let ptr_b = SendPtr(rgb_b.as_mut_ptr());
    let buf_len = rw * rh;

    (BORDER..rh.saturating_sub(BORDER))
        .into_par_iter()
        .for_each(|ry| {
            // SAFETY: Each row writes only to its own green positions (ry*rw+col_start, +2, ...).
            // No two parallel iterations share the same ry. Reads from other rows access
            // values set in CFA copy or Step 4.2, which are not modified here.
            let r_buf = unsafe { std::slice::from_raw_parts_mut(ptr_r.get(), buf_len) };
            let b_buf = unsafe { std::slice::from_raw_parts_mut(ptr_b.get(), buf_len) };

            let col_start = BORDER + (pattern.color_at(ry, 1) & 1);
            let mut rx = col_start;
            while rx < rw.saturating_sub(BORDER) {
                let idx = ry * rw + rx;

                let vh_central = vh_dir[idx];
                let vh_neighbourhood = 0.25
                    * (vh_dir[idx - w1 - 1]
                        + vh_dir[idx - w1 + 1]
                        + vh_dir[idx + w1 - 1]
                        + vh_dir[idx + w1 + 1]);
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
                    let sn_abs = (r_buf[idx - w1] - r_buf[idx + w1]).abs();
                    let ew_abs = (r_buf[idx - 1] - r_buf[idx + 1]).abs();
                    let n_grad = n1 + sn_abs + (r_buf[idx - w1] - r_buf[idx - w3]).abs();
                    let s_grad = s1 + sn_abs + (r_buf[idx + w1] - r_buf[idx + w3]).abs();
                    let w_grad = w1_val + ew_abs + (r_buf[idx - 1] - r_buf[idx - 3]).abs();
                    let e_grad = e1_val + ew_abs + (r_buf[idx + 1] - r_buf[idx + 3]).abs();

                    let n_est = r_buf[idx - w1] - rgb_g[idx - w1];
                    let s_est = r_buf[idx + w1] - rgb_g[idx + w1];
                    let w_est = r_buf[idx - 1] - rgb_g[idx - 1];
                    let e_est = r_buf[idx + 1] - rgb_g[idx + 1];

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                    r_buf[idx] = (g_center + intp(vh_disc, v_est, h_est)).clamp(0.0, 1.0);
                }

                // Interpolate B
                {
                    let sn_abs = (b_buf[idx - w1] - b_buf[idx + w1]).abs();
                    let ew_abs = (b_buf[idx - 1] - b_buf[idx + 1]).abs();
                    let n_grad = n1 + sn_abs + (b_buf[idx - w1] - b_buf[idx - w3]).abs();
                    let s_grad = s1 + sn_abs + (b_buf[idx + w1] - b_buf[idx + w3]).abs();
                    let w_grad = w1_val + ew_abs + (b_buf[idx - 1] - b_buf[idx - 3]).abs();
                    let e_grad = e1_val + ew_abs + (b_buf[idx + 1] - b_buf[idx + 3]).abs();

                    let n_est = b_buf[idx - w1] - rgb_g[idx - w1];
                    let s_est = b_buf[idx + w1] - rgb_g[idx + w1];
                    let w_est = b_buf[idx - 1] - rgb_g[idx - 1];
                    let e_est = b_buf[idx + 1] - rgb_g[idx + 1];

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                    b_buf[idx] = (g_center + intp(vh_disc, v_est, h_est)).clamp(0.0, 1.0);
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
