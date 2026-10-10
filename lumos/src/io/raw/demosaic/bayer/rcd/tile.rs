//! [`Tile`]: one tile of the RCD demosaic and the buffers it works in.

use crate::io::raw::demosaic::bayer::rcd::{
    BORDER, EPS, EPSSQ, INTERPOLATED_BORDER, TILE, estimate_green, intp, pq_neighbourhood,
    vh_neighbourhood,
};
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern};
use crate::io::raw::demosaic::tiled::{OutputPlanes, TilePlace};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The working buffers of one tile, kept from tile to tile by the worker that holds it.
///
/// Each holds the tile's crop of the frame at the crop's own width, so the stages run on the crop
/// as on a frame of that size. A cell no stage writes for this tile keeps what the last tile left:
/// only pixels within [`INTERPOLATED_BORDER`] of the crop's edge read such cells, and the tile
/// does not write those.
#[derive(Debug)]
pub(super) struct Tile {
    cfa: Vec<f32>,
    vh_dir: Vec<f32>,
    /// The vertical high-pass filter, then the low-pass filter, then the diagonal direction map.
    scratch: Vec<f32>,
    p_hpf: Vec<f32>,
    q_hpf: Vec<f32>,
    rgb: [Vec<f32>; 3],
}

impl Tile {
    pub(super) fn new() -> Self {
        let plane = || vec![0.0f32; TILE * TILE];
        let half = || vec![0.0f32; TILE.div_ceil(2) * TILE];
        Self {
            cfa: plane(),
            vh_dir: plane(),
            scratch: plane(),
            p_hpf: half(),
            q_hpf: half(),
            rgb: [plane(), plane(), plane()],
        }
    }

    /// The bytes one tile's buffers hold.
    pub(super) const fn bytes() -> usize {
        (7 * TILE * TILE + 2 * TILE.div_ceil(2) * TILE) * size_of::<f32>()
    }

    /// Demosaic the crop of `bayer` at `place`, balanced by its gains, and write the part
    /// [`INTERPOLATED_BORDER`] or more inside the crop's edges into `out` in the frame's own
    /// balance.
    ///
    /// # Safety
    ///
    /// `out` covers the whole frame, and no other tile writes the pixels this one owns, which tile
    /// the frame without overlap at the stride of `TILE − 2·INTERPOLATED_BORDER`.
    pub(super) unsafe fn demosaic(
        &mut self,
        bayer: &BayerImage<'_>,
        place: TilePlace,
        out: OutputPlanes,
    ) {
        let TilePlace { top, left, size } = place;
        let Size2us { width, height } = size;
        let (frame_width, pattern, gains) = (bayer.size.width, bayer.pattern, bayer.gains);
        debug_assert!(
            top % 2 == 0 && left % 2 == 0,
            "a tile keeps the frame's phase"
        );
        debug_assert!(width <= TILE && height <= TILE, "{size:?} past a tile");
        for (row, line) in self.cfa[..width * height]
            .chunks_exact_mut(width)
            .enumerate()
        {
            let start = (top + row) * frame_width + left;
            let input = &bayer.data[start..start + width];
            // A tile starts on the frame's phase, so a row's colours alternate from its own first.
            let row_gains = [
                gains[pattern.color_at(Vec2us::new(0, row))],
                gains[pattern.color_at(Vec2us::new(1, row))],
            ];
            if row_gains == [1.0; 2] {
                line.copy_from_slice(input);
            } else {
                for (col, (balanced, &sample)) in line.iter_mut().zip(input).enumerate() {
                    *balanced = sample * row_gains[col & 1];
                }
            }
        }
        self.seed(size, pattern);
        self.directions(size);
        self.green(size, pattern);
        self.diagonal_directions(size, pattern);
        self.opposite_colours(size, pattern);
        self.colours_at_green(size, pattern);
        let border = INTERPOLATED_BORDER;
        for row in border..height - border {
            let at = row * width;
            let index = (top + row) * frame_width + left;
            for (channel, (plane, &gain)) in self.rgb.iter().zip(&gains).enumerate() {
                // A channel at unit gain holds its native samples as seeded from the input, exact,
                // and its interpolated ones in the frame's own balance: the row goes out whole.
                if gain == 1.0 {
                    // SAFETY: the caller hands over the frame's planes and the region this tile
                    // alone owns, which this row's part lies in.
                    unsafe {
                        out.write_row(
                            channel,
                            index + border,
                            &plane[at + border..at + width - border],
                        );
                    }
                    continue;
                }
                for col in border..width - border {
                    // SAFETY: as above, for this pixel.
                    unsafe {
                        out.write(
                            channel,
                            index + col,
                            pattern.color_at(Vec2us::new(col, row)),
                            bayer.data[index + col],
                            plane[at + col],
                            gain,
                        );
                    }
                }
            }
        }
    }

    /// Each sample into its own colour's plane.
    fn seed(&mut self, Size2us { width, height }: Size2us, pattern: CfaPattern) {
        for row in 0..height {
            for col in 0..width {
                let index = row * width + col;
                self.rgb[pattern.color_at(Vec2us::new(col, row))][index] = self.cfa[index];
            }
        }
    }

    /// Step 1: the vertical-against-horizontal direction map, from the squared high-pass filters
    /// summed over three pixels along each axis.
    fn directions(&mut self, Size2us { width, height }: Size2us) {
        let cfa = &self.cfa;
        let (w1, w2, w3) = (width, 2 * width, 3 * width);
        let v_hpf = &mut self.scratch;
        for row in 3..height - 3 {
            for col in 0..width {
                let idx = row * width + col;
                let v = (cfa[idx - w3] - cfa[idx - w1] - cfa[idx + w1] + cfa[idx + w3])
                    - 3.0 * (cfa[idx - w2] + cfa[idx + w2])
                    + 6.0 * cfa[idx];
                v_hpf[idx] = v * v;
            }
        }
        let h_hpf_sq = |idx: usize| {
            let v = (cfa[idx - 3] - cfa[idx - 1] - cfa[idx + 1] + cfa[idx + 3])
                - 3.0 * (cfa[idx - 2] + cfa[idx + 2])
                + 6.0 * cfa[idx];
            v * v
        };
        for row in BORDER..height - BORDER {
            let base = row * width;
            let mut h_prev = h_hpf_sq(base + BORDER - 1);
            let mut h_curr = h_hpf_sq(base + BORDER);
            for col in BORDER..width - BORDER {
                let idx = base + col;
                let h_next = h_hpf_sq(idx + 1);
                let v_stat = (v_hpf[idx - w1] + v_hpf[idx] + v_hpf[idx + w1]).max(EPSSQ);
                let h_stat = (h_prev + h_curr + h_next).max(EPSSQ);
                self.vh_dir[idx] = v_stat / (v_stat + h_stat);
                h_prev = h_curr;
                h_curr = h_next;
            }
        }
    }

    /// Steps 2 and 3: the low-pass filter, then green at each red and blue site from the ratio of
    /// its neighbours' green to the filter, blended along the direction map.
    fn green(&mut self, Size2us { width, height }: Size2us, pattern: CfaPattern) {
        let cfa = &self.cfa;
        let lpf = &mut self.scratch;
        for row in 1..height - 1 {
            for col in 1..width - 1 {
                let i = row * width + col;
                lpf[i] = cfa[i]
                    + 0.5 * (cfa[i - width] + cfa[i + width] + cfa[i - 1] + cfa[i + 1])
                    + 0.25
                        * (cfa[i - width - 1]
                            + cfa[i - width + 1]
                            + cfa[i + width - 1]
                            + cfa[i + width + 1]);
            }
        }
        let lpf = &self.scratch;
        let (w1, w2, w3, w4) = (width, 2 * width, 3 * width, 4 * width);
        let green = &mut self.rgb[1];
        for row in BORDER..height - BORDER {
            let mut col = BORDER + (pattern.color_at(Vec2us::new(0, row)) & 1);
            while col < width - BORDER {
                let idx = row * width + col;
                let cfai = cfa[idx];
                // In pairs, as librtprocess sums them.
                let n_grad = EPS
                    + ((cfa[idx - w1] - cfa[idx + w1]).abs() + (cfai - cfa[idx - w2]).abs())
                    + ((cfa[idx - w1] - cfa[idx - w3]).abs()
                        + (cfa[idx - w2] - cfa[idx - w4]).abs());
                let s_grad = EPS
                    + ((cfa[idx - w1] - cfa[idx + w1]).abs() + (cfai - cfa[idx + w2]).abs())
                    + ((cfa[idx + w1] - cfa[idx + w3]).abs()
                        + (cfa[idx + w2] - cfa[idx + w4]).abs());
                let w_grad = EPS
                    + ((cfa[idx - 1] - cfa[idx + 1]).abs() + (cfai - cfa[idx - 2]).abs())
                    + ((cfa[idx - 1] - cfa[idx - 3]).abs() + (cfa[idx - 2] - cfa[idx - 4]).abs());
                let e_grad = EPS
                    + ((cfa[idx - 1] - cfa[idx + 1]).abs() + (cfai - cfa[idx + 2]).abs())
                    + ((cfa[idx + 1] - cfa[idx + 3]).abs() + (cfa[idx + 2] - cfa[idx + 4]).abs());

                let lpfi = lpf[idx];
                let n_est = estimate_green(cfa[idx - w1], lpfi, lpf[idx - w2]);
                let s_est = estimate_green(cfa[idx + w1], lpfi, lpf[idx + w2]);
                let w_est = estimate_green(cfa[idx - 1], lpfi, lpf[idx - 2]);
                let e_est = estimate_green(cfa[idx + 1], lpfi, lpf[idx + 2]);

                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

                let vh_central = self.vh_dir[idx];
                let vh_neighbourhood = vh_neighbourhood(&self.vh_dir, idx, w1);
                let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neighbourhood).abs() {
                    vh_neighbourhood
                } else {
                    vh_central
                };
                green[idx] = intp(vh_disc, v_est, h_est);
                col += 2;
            }
        }
    }

    /// Steps 4.0 and 4.1: the diagonal direction map at red and blue sites, from the squared
    /// diagonal high-pass filters summed over each site and its two neighbours along the diagonal.
    fn diagonal_directions(&mut self, Size2us { width, height }: Size2us, pattern: CfaPattern) {
        let cfa = &self.cfa;
        let (w1, w2, w3) = (width, 2 * width, 3 * width);
        let half_w = width.div_ceil(2);
        for row in 3..height - 3 {
            // The red and blue sites of this row, the three along each diagonal of which step 4.1
            // sums, as RCD 2.3's closed forms do. librtprocess keeps the filter on odd columns
            // only, so its step 4.1 reads some of the three beside the diagonal.
            let mut col = 3 + (pattern.color_at(Vec2us::new(1, row)) & 1);
            while col < width - 3 {
                let idx = row * width + col;
                let hx = row * half_w + col / 2;
                let p_val = (cfa[idx - w3 - 3] - cfa[idx - w1 - 1] - cfa[idx + w1 + 1]
                    + cfa[idx + w3 + 3])
                    - 3.0 * (cfa[idx - w2 - 2] + cfa[idx + w2 + 2])
                    + 6.0 * cfa[idx];
                self.p_hpf[hx] = p_val * p_val;
                let q_val = (cfa[idx - w3 + 3] - cfa[idx - w1 + 1] - cfa[idx + w1 - 1]
                    + cfa[idx + w3 - 3])
                    - 3.0 * (cfa[idx - w2 + 2] + cfa[idx + w2 - 2])
                    + 6.0 * cfa[idx];
                self.q_hpf[hx] = q_val * q_val;
                col += 2;
            }
        }
        let pq_dir = &mut self.scratch;
        for row in BORDER..height - BORDER {
            let mut col = BORDER + (pattern.color_at(Vec2us::new(0, row)) & 1);
            while col < width - BORDER {
                let h_center = row * half_w + col / 2;
                let h_nw = (row - 1) * half_w + (col - 1) / 2;
                let h_se = (row + 1) * half_w + col.div_ceil(2);
                let h_ne = (row - 1) * half_w + col.div_ceil(2);
                let h_sw = (row + 1) * half_w + (col - 1) / 2;
                let p_stat =
                    (self.p_hpf[h_nw] + self.p_hpf[h_center] + self.p_hpf[h_se]).max(EPSSQ);
                let q_stat =
                    (self.q_hpf[h_ne] + self.q_hpf[h_center] + self.q_hpf[h_sw]).max(EPSSQ);
                pq_dir[row * width + col] = p_stat / (p_stat + q_stat);
                col += 2;
            }
        }
    }

    /// Step 4.2: blue at red sites and red at blue ones, from the diagonal colour differences
    /// along the diagonal direction map.
    fn opposite_colours(&mut self, Size2us { width, height }: Size2us, pattern: CfaPattern) {
        let (w1, w2, w3) = (width, 2 * width, 3 * width);
        let pq_dir = &self.scratch;
        let [red, green, blue] = &mut self.rgb;
        for row in BORDER..height - BORDER {
            let col_start = BORDER + (pattern.color_at(Vec2us::new(0, row)) & 1);
            // A red row takes blue and a blue row red; the diagonal reads land on that colour's
            // own samples, which this step never writes.
            let dst = match pattern.color_at(Vec2us::new(col_start, row)) {
                0 => &mut *blue,
                2 => &mut *red,
                _ => continue,
            };
            let mut col = col_start;
            while col < width - BORDER {
                let idx = row * width + col;
                let pq_central = pq_dir[idx];
                let pq_neighbourhood = pq_neighbourhood(pq_dir, idx, w1);
                let pq_disc = if (0.5 - pq_central).abs() < (0.5 - pq_neighbourhood).abs() {
                    pq_neighbourhood
                } else {
                    pq_central
                };

                let nw_grad = EPS
                    + (dst[idx - w1 - 1] - dst[idx + w1 + 1]).abs()
                    + (dst[idx - w1 - 1] - dst[idx - w3 - 3]).abs()
                    + (green[idx] - green[idx - w2 - 2]).abs();
                let ne_grad = EPS
                    + (dst[idx - w1 + 1] - dst[idx + w1 - 1]).abs()
                    + (dst[idx - w1 + 1] - dst[idx - w3 + 3]).abs()
                    + (green[idx] - green[idx - w2 + 2]).abs();
                let sw_grad = EPS
                    + (dst[idx - w1 + 1] - dst[idx + w1 - 1]).abs()
                    + (dst[idx + w1 - 1] - dst[idx + w3 - 3]).abs()
                    + (green[idx] - green[idx + w2 - 2]).abs();
                let se_grad = EPS
                    + (dst[idx - w1 - 1] - dst[idx + w1 + 1]).abs()
                    + (dst[idx + w1 + 1] - dst[idx + w3 + 3]).abs()
                    + (green[idx] - green[idx + w2 + 2]).abs();

                let nw_est = dst[idx - w1 - 1] - green[idx - w1 - 1];
                let ne_est = dst[idx - w1 + 1] - green[idx - w1 + 1];
                let sw_est = dst[idx + w1 - 1] - green[idx + w1 - 1];
                let se_est = dst[idx + w1 + 1] - green[idx + w1 + 1];

                let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
                let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

                dst[idx] = green[idx] + intp(pq_disc, p_est, q_est);
                col += 2;
            }
        }
    }

    /// Step 4.3: red and blue at green sites, from the colour differences of their four
    /// neighbours along the direction map.
    fn colours_at_green(&mut self, Size2us { width, height }: Size2us, pattern: CfaPattern) {
        let (w1, w2, w3) = (width, 2 * width, 3 * width);
        let vh_dir = &self.vh_dir;
        let [red, green, blue] = &mut self.rgb;
        for row in BORDER..height - BORDER {
            let mut col = BORDER + (pattern.color_at(Vec2us::new(1, row)) & 1);
            while col < width - BORDER {
                let idx = row * width + col;
                let vh_central = vh_dir[idx];
                let vh_neighbourhood = vh_neighbourhood(vh_dir, idx, w1);
                let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neighbourhood).abs() {
                    vh_neighbourhood
                } else {
                    vh_central
                };

                let g_center = green[idx];
                let n1 = EPS + (g_center - green[idx - w2]).abs();
                let s1 = EPS + (g_center - green[idx + w2]).abs();
                let w1_val = EPS + (g_center - green[idx - 2]).abs();
                let e1_val = EPS + (g_center - green[idx + 2]).abs();

                for plane in [&mut *red, &mut *blue] {
                    let sn_abs = (plane[idx - w1] - plane[idx + w1]).abs();
                    let ew_abs = (plane[idx - 1] - plane[idx + 1]).abs();
                    let n_grad = n1 + sn_abs + (plane[idx - w1] - plane[idx - w3]).abs();
                    let s_grad = s1 + sn_abs + (plane[idx + w1] - plane[idx + w3]).abs();
                    let w_grad = w1_val + ew_abs + (plane[idx - 1] - plane[idx - 3]).abs();
                    let e_grad = e1_val + ew_abs + (plane[idx + 1] - plane[idx + 3]).abs();

                    let n_est = plane[idx - w1] - green[idx - w1];
                    let s_est = plane[idx + w1] - green[idx + w1];
                    let w_est = plane[idx - 1] - green[idx - 1];
                    let e_est = plane[idx + 1] - green[idx + 1];

                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                    plane[idx] = g_center + intp(vh_disc, v_est, h_est);
                }
                col += 2;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    impl Tile {
        /// Every buffer filled with `value`, as a tile finds them after other tiles.
        pub(crate) fn poison(&mut self, value: f32) {
            for buffer in [
                &mut self.cfa,
                &mut self.vh_dir,
                &mut self.scratch,
                &mut self.p_hpf,
                &mut self.q_hpf,
            ]
            .into_iter()
            .chain(&mut self.rgb)
            {
                buffer.fill(value);
            }
        }
    }
}
