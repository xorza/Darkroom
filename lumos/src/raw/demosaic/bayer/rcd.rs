//! RCD (Ratio Corrected Demosaicing) algorithm.
//!
//! Based on: Luis Sanz Rodriguez, "Ratio Corrected Demosaicing" v2.3 (2017).
//! Reference: <https://github.com/LuisSR/RCD-Demosaicing>
//!
//! The algorithm uses directional discrimination and ratio-corrected
//! interpolation in a low-pass filter domain to reduce color artifacts,
//! particularly beneficial for astrophotography (star morphology).

use super::{BayerImage, CfaPattern};

const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;
/// Border size required by the algorithm (pixels on each side).
const BORDER: usize = 4;

/// Linear interpolation: `(1 - a) * b + a * c`.
#[inline(always)]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    b + a * (c - b)
}

/// RCD demosaic implementation.
///
/// Input: BayerImage with normalized [0,1] CFA data and margin info.
/// Output: Interleaved RGB f32 pixels `[R0, G0, B0, R1, G1, B1, ...]`
/// for the active area (width × height).
pub(super) fn rcd_demosaic(bayer: &BayerImage) -> Vec<f32> {
    let width = bayer.width;
    let height = bayer.height;

    // We work in raw coordinates. The active area starts at (top_margin, left_margin).
    // The algorithm needs 4 pixels of border on each side. We clamp the processing
    // area to what's available in the raw buffer.
    let rw = bayer.raw_width;
    let tm = bayer.top_margin;
    let lm = bayer.left_margin;

    // CFA accessor: returns the color at raw coordinates (ry, rx).
    // The CFA pattern is defined relative to raw (0,0).
    let cfa_color = |ry: usize, rx: usize| bayer.cfa.color_at(ry, rx);

    // Raw pixel accessor (flat index).
    let cfa = bayer.data;

    // Allocate working buffers in raw coordinate space.
    let npix = rw * bayer.raw_height;

    // VH_Dir: direction map [0,1] where 0=vertical, 1=horizontal
    let mut vh_dir = vec![0.0f32; npix];

    // LPF: low-pass filter at R/B positions only (half the pixels)
    let mut lpf = vec![0.0f32; npix];

    // RGB output in raw coordinate space (planar: rgb[channel][index])
    let mut rgb = [vec![0.0f32; npix], vec![0.0f32; npix], vec![0.0f32; npix]];

    // Copy CFA values to the appropriate RGB channel.
    for ry in 0..bayer.raw_height {
        for rx in 0..rw {
            let idx = ry * rw + rx;
            let c = cfa_color(ry, rx);
            rgb[c][idx] = cfa[idx];
        }
    }

    // ── Step 1: V/H Direction Detection ──────────────────────────────────

    // Step 1.1: Compute high-pass filter responses.
    // The HPF is: (cfa[-3w] - cfa[-w] - cfa[+w] + cfa[+3w]) - 3*(cfa[-2w] + cfa[+2w]) + 6*cfa[0]
    // Squared, then summed over 3 consecutive rows/cols for smoothing.

    // We compute V and H HPF values and store VH_Dir directly.
    // V needs rows [row-3..row+3] for the HPF, then summed over 3 consecutive values.
    // So total vertical reach is row-1-3=row-4 to row+1+3=row+4, meaning we need 4-pixel border.

    // Vertical HPF buffer: we need 3 consecutive rows of HPF values.
    // Process row by row, keeping a sliding window of 3 HPF rows.
    let rh = bayer.raw_height;
    let mut v_hpf = vec![vec![0.0f32; rw]; 3]; // circular buffer of 3 rows

    // Pre-fill the first 2 rows of vertical HPF (rows 3 and 4)
    for (buf_row, hpf_row) in v_hpf.iter_mut().enumerate().take(2) {
        let ry = 3 + buf_row; // raw row 3, 4
        if ry + 3 >= rh {
            continue;
        }
        let w1 = rw;
        let w2 = 2 * rw;
        let w3 = 3 * rw;
        for (rx, hpf_val) in hpf_row.iter_mut().enumerate().take(rw) {
            let idx = ry * rw + rx;
            let val = (cfa[idx - w3] - cfa[idx - w1] - cfa[idx + w1] + cfa[idx + w3])
                - 3.0 * (cfa[idx - w2] + cfa[idx + w2])
                + 6.0 * cfa[idx];
            *hpf_val = val * val;
        }
    }

    // Process rows 4..rh-4 for VH_Dir
    for ry in 4..rh.saturating_sub(4) {
        // Compute vertical HPF for row ry+1 (we need rows ry-1, ry, ry+1)
        let new_row = ry + 1;
        let buf_idx = (new_row - 3) % 3;
        if new_row + 3 < rh {
            let w1 = rw;
            let w2 = 2 * rw;
            let w3 = 3 * rw;
            for (rx, hpf_val) in v_hpf[buf_idx].iter_mut().enumerate().take(rw) {
                let idx = new_row * rw + rx;
                let val = (cfa[idx - w3] - cfa[idx - w1] - cfa[idx + w1] + cfa[idx + w3])
                    - 3.0 * (cfa[idx - w2] + cfa[idx + w2])
                    + 6.0 * cfa[idx];
                *hpf_val = val * val;
            }
        }

        // For VH_Dir at (ry, rx), V_Stat = sum of 3 vertical HPF values at
        // rows (ry-1, ry, ry+1) at column rx.
        // H_Stat = sum of 3 horizontal HPF values at columns (rx-1, rx, rx+1) at row ry.

        // Compute horizontal HPF for this row
        let mut h_hpf = vec![0.0f32; rw];
        for (rx, hpf_val) in h_hpf
            .iter_mut()
            .enumerate()
            .take(rw.saturating_sub(3))
            .skip(3)
        {
            let idx = ry * rw + rx;
            let val = (cfa[idx - 3] - cfa[idx - 1] - cfa[idx + 1] + cfa[idx + 3])
                - 3.0 * (cfa[idx - 2] + cfa[idx + 2])
                + 6.0 * cfa[idx];
            *hpf_val = val * val;
        }

        // VH_Dir
        let v0_idx = (ry - 1 - 3) % 3;
        let v1_idx = (ry - 3) % 3;
        let v2_idx = (ry + 1 - 3) % 3;
        for rx in 4..rw.saturating_sub(4) {
            let v_stat = (v_hpf[v0_idx][rx] + v_hpf[v1_idx][rx] + v_hpf[v2_idx][rx]).max(EPSSQ);
            let h_stat = (h_hpf[rx - 1] + h_hpf[rx] + h_hpf[rx + 1]).max(EPSSQ);
            vh_dir[ry * rw + rx] = v_stat / (v_stat + h_stat);
        }
    }

    // ── Step 2: Low-Pass Filter ──────────────────────────────────────────

    // LPF at R/B positions (every other pixel, col stride 2):
    // lpf = cfa + 0.5*(N+S+W+E) + 0.25*(NW+NE+SW+SE)
    // where N,S,W,E are the 4 cardinal neighbors (same-color pixels at distance 2,
    // but we use distance 1 since neighbors are green).
    // Actually from reference: lpf uses cfa at distance 1 (the green neighbors).
    for ry in 2..rh.saturating_sub(2) {
        // Start col: R/B positions have color_at != 1 (green)
        let col_start = 2 + (cfa_color(ry, 0) & 1);
        let mut rx = col_start;
        while rx < rw.saturating_sub(2) {
            let idx = ry * rw + rx;
            lpf[idx] = cfa[idx]
                + 0.5 * (cfa[idx - rw] + cfa[idx + rw] + cfa[idx - 1] + cfa[idx + 1])
                + 0.25
                    * (cfa[idx - rw - 1]
                        + cfa[idx - rw + 1]
                        + cfa[idx + rw - 1]
                        + cfa[idx + rw + 1]);
            rx += 2;
        }
    }

    // ── Step 3: Green Channel Interpolation ──────────────────────────────

    let w1 = rw;
    let w2 = 2 * rw;
    let w3 = 3 * rw;
    let w4 = 4 * rw;

    for ry in BORDER..rh.saturating_sub(BORDER) {
        // Process only R/B positions (non-green)
        let col_start = BORDER + (cfa_color(ry, 0) & 1);
        let mut rx = col_start;
        while rx < rw.saturating_sub(BORDER) {
            let idx = ry * rw + rx;

            // Cardinal gradients
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

            // Ratio-corrected cardinal estimations
            let lpfi = lpf[idx];
            let two_lpfi = lpfi + lpfi;
            let n_est = cfa[idx - w1] * two_lpfi / (EPS + lpfi + lpf[idx - w2]);
            let s_est = cfa[idx + w1] * two_lpfi / (EPS + lpfi + lpf[idx + w2]);
            let w_est = cfa[idx - 1] * two_lpfi / (EPS + lpfi + lpf[idx - 2]);
            let e_est = cfa[idx + 1] * two_lpfi / (EPS + lpfi + lpf[idx + 2]);

            // Vertical and horizontal estimates
            let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
            let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

            // VH discrimination refinement
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

            // Green = blend of V and H estimates using direction weight
            rgb[1][idx] = intp(vh_disc, v_est, h_est).clamp(0.0, 1.0);

            rx += 2;
        }
    }

    // ── Step 4: Red and Blue Channel Interpolation ───────────────────────

    // Step 4.0-4.1: P/Q diagonal direction detection
    // Similar HPF structure but on diagonals.
    let mut pq_dir = vec![0.0f32; npix];

    {
        // Compute P (NW-SE) and Q (NE-SW) high-pass filter values.
        // P: offset pattern (-w3-3, -w1-1, +w1+1, +w3+3) along NW-SE diagonal
        // Q: offset pattern (-w3+3, -w1+1, +w1-1, +w3-3) along NE-SW diagonal
        // Both are squared, then summed over 3 consecutive diagonal positions.

        // Half-resolution indexing for P/Q (every other pixel)
        let half_w = rw.div_ceil(2);
        let half_size = half_w * rh;
        let mut p_hpf = vec![0.0f32; half_size];
        let mut q_hpf = vec![0.0f32; half_size];

        // Compute P and Q HPF at all positions with sufficient border
        for ry in 3..rh.saturating_sub(3) {
            let col_start = 3 + ((ry + 1) & 1); // offset for checkerboard pattern
            let mut rx = col_start;
            while rx < rw.saturating_sub(3) {
                let idx = ry * rw + rx;
                let hidx = ry * half_w + rx / 2;

                // P diagonal: NW-SE
                let p_val = (cfa[idx - w3 - 3] - cfa[idx - w1 - 1] - cfa[idx + w1 + 1]
                    + cfa[idx + w3 + 3])
                    - 3.0 * (cfa[idx - w2 - 2] + cfa[idx + w2 + 2])
                    + 6.0 * cfa[idx];
                p_hpf[hidx] = p_val * p_val;

                // Q diagonal: NE-SW
                let q_val = (cfa[idx - w3 + 3] - cfa[idx - w1 + 1] - cfa[idx + w1 - 1]
                    + cfa[idx + w3 - 3])
                    - 3.0 * (cfa[idx - w2 + 2] + cfa[idx + w2 - 2])
                    + 6.0 * cfa[idx];
                q_hpf[hidx] = q_val * q_val;

                rx += 2;
            }
        }

        // Compute PQ_Dir from HPF sums at R/B positions
        for ry in BORDER..rh.saturating_sub(BORDER) {
            let col_start = BORDER + (cfa_color(ry, 0) & 1);
            let mut rx = col_start;
            while rx < rw.saturating_sub(BORDER) {
                let idx = ry * rw + rx;

                // Sum 3 neighboring HPF values along the diagonal
                let h_center = ry * half_w + rx / 2;
                let h_nw = (ry - 1) * half_w + (rx - 1) / 2;
                let h_se = (ry + 1) * half_w + rx.div_ceil(2);
                let h_ne = (ry - 1) * half_w + rx.div_ceil(2);
                let h_sw = (ry + 1) * half_w + (rx - 1) / 2;

                let p_stat = (p_hpf[h_nw] + p_hpf[h_center] + p_hpf[h_se]).max(EPSSQ);
                let q_stat = (q_hpf[h_ne] + q_hpf[h_center] + q_hpf[h_sw]).max(EPSSQ);

                pq_dir[idx] = p_stat / (p_stat + q_stat);

                rx += 2;
            }
        }
    }

    // Step 4.2: Red/Blue at opposing CFA positions
    // At R positions: interpolate B. At B positions: interpolate R.
    for ry in BORDER..rh.saturating_sub(BORDER) {
        let col_start = BORDER + (cfa_color(ry, 0) & 1);
        let mut rx = col_start;
        while rx < rw.saturating_sub(BORDER) {
            let idx = ry * rw + rx;
            // c = the missing color: if this is R (0), we need B (2); if B (2), we need R (0)
            let c = 2 - cfa_color(ry, rx);

            // PQ discrimination refinement
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

            // Diagonal gradients
            let nw_grad = EPS
                + (rgb[c][idx - w1 - 1] - rgb[c][idx + w1 + 1]).abs()
                + (rgb[c][idx - w1 - 1] - rgb[c][idx - w3 - 3]).abs()
                + (rgb[1][idx] - rgb[1][idx - w2 - 2]).abs();
            let ne_grad = EPS
                + (rgb[c][idx - w1 + 1] - rgb[c][idx + w1 - 1]).abs()
                + (rgb[c][idx - w1 + 1] - rgb[c][idx - w3 + 3]).abs()
                + (rgb[1][idx] - rgb[1][idx - w2 + 2]).abs();
            let sw_grad = EPS
                + (rgb[c][idx - w1 + 1] - rgb[c][idx + w1 - 1]).abs()
                + (rgb[c][idx + w1 - 1] - rgb[c][idx + w3 - 3]).abs()
                + (rgb[1][idx] - rgb[1][idx + w2 - 2]).abs();
            let se_grad = EPS
                + (rgb[c][idx - w1 - 1] - rgb[c][idx + w1 + 1]).abs()
                + (rgb[c][idx + w1 + 1] - rgb[c][idx + w3 + 3]).abs()
                + (rgb[1][idx] - rgb[1][idx + w2 + 2]).abs();

            // Diagonal color difference estimates
            let nw_est = rgb[c][idx - w1 - 1] - rgb[1][idx - w1 - 1];
            let ne_est = rgb[c][idx - w1 + 1] - rgb[1][idx - w1 + 1];
            let sw_est = rgb[c][idx + w1 - 1] - rgb[1][idx + w1 - 1];
            let se_est = rgb[c][idx + w1 + 1] - rgb[1][idx + w1 + 1];

            // P (NW-SE) and Q (NE-SW) estimates
            let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
            let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

            // Interpolate missing color
            rgb[c][idx] = (rgb[1][idx] + intp(pq_disc, p_est, q_est)).clamp(0.0, 1.0);

            rx += 2;
        }
    }

    // Step 4.3: Red/Blue at Green CFA positions
    for ry in BORDER..rh.saturating_sub(BORDER) {
        // Green positions: where cfa_color is 1
        let col_start = BORDER + (cfa_color(ry, 1) & 1);
        let mut rx = col_start;
        while rx < rw.saturating_sub(BORDER) {
            let idx = ry * rw + rx;

            // VH discrimination
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

            // Precompute green gradient components shared by R and B
            let n1 = EPS + (rgb[1][idx] - rgb[1][idx - w2]).abs();
            let s1 = EPS + (rgb[1][idx] - rgb[1][idx + w2]).abs();
            let w1_val = EPS + (rgb[1][idx] - rgb[1][idx - 2]).abs();
            let e1_val = EPS + (rgb[1][idx] - rgb[1][idx + 2]).abs();

            // Interpolate both R (c=0) and B (c=2)
            for c in [0, 2] {
                let sn_abs = (rgb[c][idx - w1] - rgb[c][idx + w1]).abs();
                let ew_abs = (rgb[c][idx - 1] - rgb[c][idx + 1]).abs();

                let n_grad = n1 + sn_abs + (rgb[c][idx - w1] - rgb[c][idx - w3]).abs();
                let s_grad = s1 + sn_abs + (rgb[c][idx + w1] - rgb[c][idx + w3]).abs();
                let w_grad = w1_val + ew_abs + (rgb[c][idx - 1] - rgb[c][idx - 3]).abs();
                let e_grad = e1_val + ew_abs + (rgb[c][idx + 1] - rgb[c][idx + 3]).abs();

                // Color difference estimates
                let n_est = rgb[c][idx - w1] - rgb[1][idx - w1];
                let s_est = rgb[c][idx + w1] - rgb[1][idx + w1];
                let w_est = rgb[c][idx - 1] - rgb[1][idx - 1];
                let e_est = rgb[c][idx + 1] - rgb[1][idx + 1];

                let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                rgb[c][idx] = (rgb[1][idx] + intp(vh_disc, v_est, h_est)).clamp(0.0, 1.0);
            }

            rx += 2;
        }
    }

    // ── Step 5: Border Handling ──────────────────────────────────────────

    // Simple bilinear interpolation for the border region.
    border_interpolate(&mut rgb, cfa, bayer.raw_height, rw, &bayer.cfa, BORDER);

    // ── Extract active area to interleaved RGB output ────────────────────

    let out_size = width * height * 3;
    let mut output = vec![0.0f32; out_size];

    for y in 0..height {
        let ry = tm + y;
        for x in 0..width {
            let rx = lm + x;
            let raw_idx = ry * rw + rx;
            let out_idx = (y * width + x) * 3;
            output[out_idx] = rgb[0][raw_idx];
            output[out_idx + 1] = rgb[1][raw_idx];
            output[out_idx + 2] = rgb[2][raw_idx];
        }
    }

    output
}

/// Simple bilinear border interpolation for pixels within `border` pixels of the edge.
fn border_interpolate(
    rgb: &mut [Vec<f32>; 3],
    cfa: &[f32],
    height: usize,
    width: usize,
    pattern: &CfaPattern,
    border: usize,
) {
    for ry in 0..height {
        for rx in 0..width {
            // Only process border pixels
            if ry >= border && ry < height - border && rx >= border && rx < width - border {
                continue;
            }

            let idx = ry * width + rx;
            let c = pattern.color_at(ry, rx);

            // The CFA color is already set; interpolate the missing channels
            for (ic, rgb_ch) in rgb.iter_mut().enumerate() {
                if ic == c {
                    // Already have this channel from CFA
                    rgb_ch[idx] = cfa[idx];
                    continue;
                }

                // Average all same-color neighbors within bounds
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
                    // Expand search to 5x5 if no neighbors found
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
            }
        }
    }
}
