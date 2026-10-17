//! [`Tile`]: one tile of the RCD demosaic and the buffers it works in.
//!
//! A tile keeps its crop as the Bayer block's four phases, each a plane of half the crop's rows
//! and columns. Every RCD stage works on the sites of one phase at a time, two columns apart in
//! the crop and adjacent in their phase's plane, so a stage runs on [`F32_LANES`] sites at once
//! over [`Isa`]. Each lane computes what a scalar stage computes for its site, op for op in the
//! same order, so the output is the same bits.

use std::ops::Range;

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::demosaic::bayer::rcd::{
    BORDER, EPS, EPSSQ, INTERPOLATED_BORDER, Neighbour, TILE, discriminate, estimate_green, intp,
    pq_neighbourhood, vh_neighbourhood,
};
use crate::image::cfa::demosaic::mosaic_view::MosaicView;
use crate::image::cfa::demosaic::tiled::{OutputPlanes, TilePlace};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// A phase plane's rows, and the columns its sites take.
const HALF: usize = TILE / 2;
/// A phase plane's row: its sites, then room for the last vector of a row to read its whole
/// width, and two columns past that, the stencils' reach.
const STRIDE: usize = HALF + F32_LANES + 2;
const PLANE: usize = HALF * STRIDE;

const _: () = assert!(TILE.is_multiple_of(2), "a tile holds whole Bayer blocks");
const _: () = assert!(
    BORDER.is_multiple_of(2),
    "a stage's rows of one parity start at BORDER plus the parity"
);

/// The working buffers of one tile, kept from tile to tile by the worker that holds it.
///
/// A set of four planes holds one value per photosite of the crop, a plane per Bayer phase
/// `2·(row parity) + column parity`, a crop row `r` at the plane's row `r/2`. The stages run on the
/// crop as on a frame of its size. A cell no stage writes for this tile keeps what the last tile
/// left: only pixels within [`INTERPOLATED_BORDER`] of the crop's edge read such cells, and the
/// tile does not write those.
#[derive(Debug)]
pub(super) struct Tile {
    /// The crop's samples, balanced.
    cfa: [Vec<f32>; 4],
    /// The vertical-against-horizontal direction map.
    vh_dir: [Vec<f32>; 4],
    /// At the red and blue sites, a plane per row parity: the low-pass filter, then the diagonal
    /// direction map.
    red_blue: [Vec<f32>; 2],
    /// Red, green and blue where the stages interpolate them, balanced. A color's own phases are
    /// empty: its samples there are `cfa`'s.
    rgb: [[Vec<f32>; 4]; 3],
    /// The squared vertical high-pass filter of the last three crop rows, each at both column
    /// phases, a row `r` at `r % 3`.
    v_hpf: [[Vec<f32>; 2]; 3],
    /// The squared horizontal high-pass filter of one crop row, at both column phases.
    h_hpf: [Vec<f32>; 2],
    /// The squared diagonal high-pass filters of the last three crop rows at their red and blue
    /// sites, a row `r` at `r % 3`.
    p_hpf: [Vec<f32>; 3],
    q_hpf: [Vec<f32>; 3],
    /// One row of one color on its way out.
    line: Vec<f32>,
}

/// The stages over one tile's crop of `size`, run on the widest Isa the CPU has.
#[derive(Debug)]
struct Stages<'a> {
    tile: &'a mut Tile,
    crop: Crop,
}

/// The crop a tile demosaics: its size, the pattern of its samples, and the column phase of the
/// red and blue sites of each row parity.
#[derive(Debug, Clone, Copy)]
struct Crop {
    size: Size2us,
    pattern: CfaPattern,
    red_blue: [usize; 2],
}

/// Color `color`'s own samples: the photosites of `cfa`, a crop of `pattern`, behind its filter.
#[derive(Debug, Clone, Copy)]
struct ColorSamples<'a> {
    cfa: [&'a [f32]; 4],
    pattern: CfaPattern,
    color: usize,
}

/// A photosite at a fixed step from each site of one crop row's column phase: its own row of a
/// phase plane, and how many plane columns from the site's it lies.
#[derive(Debug, Clone, Copy)]
struct Tap<'a> {
    row: &'a [f32],
    offset: isize,
}

/// One color's set of four planes with the phase a stage writes taken out, and the others as
/// [`Tap::at`] reads them, the color's own phases from the samples.
#[derive(Debug)]
struct Split<'a> {
    written: &'a mut [f32],
    /// The planes, the written one an empty slice.
    read: [&'a [f32]; 4],
}

/// The red or blue neighbours step 4.3 reads around the green sites of one row, in one of the two
/// colors, and the row it writes.
#[derive(Debug)]
struct Neighbours<'a> {
    n3: Tap<'a>,
    n1: Tap<'a>,
    s1: Tap<'a>,
    s3: Tap<'a>,
    w3: Tap<'a>,
    w1: Tap<'a>,
    e1: Tap<'a>,
    e3: Tap<'a>,
    out: &'a mut [f32],
}

/// The spans of [`F32_LANES`] plane columns, the last one cut short, that cover a range.
#[derive(Debug)]
struct Vectors {
    next: usize,
    end: usize,
}

impl Tile {
    /// The buffers of a tile of a frame of `pattern`.
    pub(super) fn new(pattern: CfaPattern) -> Self {
        let plane = || vec![0.0f32; PLANE];
        let row = || vec![0.0f32; STRIDE];
        let color = |color: usize| {
            [0, 1, 2, 3].map(|phase| {
                if phase_color(pattern, phase) == color {
                    Vec::new()
                } else {
                    plane()
                }
            })
        };
        Self {
            cfa: [(); 4].map(|()| plane()),
            vh_dir: [(); 4].map(|()| plane()),
            red_blue: [(); 2].map(|()| plane()),
            rgb: [color(0), color(1), color(2)],
            v_hpf: [(); 3].map(|()| [(); 2].map(|()| row())),
            h_hpf: [(); 2].map(|()| row()),
            p_hpf: [(); 3].map(|()| row()),
            q_hpf: [(); 3].map(|()| row()),
            line: vec![0.0; TILE],
        }
    }

    /// The bytes one tile's buffers hold. Each pattern has four native phases among the twelve of
    /// its three colors.
    pub(super) const fn bytes() -> usize {
        ((4 + 4 + 2 + 3 * 4 - 4) * PLANE + (3 * 2 + 2 + 3 + 3) * STRIDE + TILE) * size_of::<f32>()
    }

    /// Demosaic the crop of `bayer` at `place`, balanced as the view reads it, and write the part
    /// [`INTERPOLATED_BORDER`] or more inside the crop's edges into `out`, which takes the balance
    /// back out.
    ///
    /// # Safety
    ///
    /// `out` covers the whole frame, and no other tile writes the pixels this one owns, which tile
    /// the frame without overlap at the stride of `TILE − 2·INTERPOLATED_BORDER`.
    pub(super) unsafe fn demosaic(
        &mut self,
        bayer: &MosaicView<'_, CfaPattern>,
        place: TilePlace,
        out: OutputPlanes,
    ) {
        let TilePlace { top, left, size } = place;
        let Size2us { width, height } = size;
        let pattern = bayer.pattern();
        debug_assert!(
            top % 2 == 0 && left % 2 == 0,
            "a tile keeps the frame's phase"
        );
        debug_assert!(width <= TILE && height <= TILE, "{size:?} past a tile");
        for row in 0..height {
            let at = (row / 2) * STRIDE;
            let parity = 2 * (row % 2);
            // A tile starts on the frame's phase, so the crop's colors are the frame's.
            let [even_gain, odd_gain] =
                [0, 1].map(|column| bayer.gain_at(Vec2us::new(column, row)));
            let [even, odd] = self.cfa.get_disjoint_mut([parity, parity + 1]).unwrap();
            let (even, odd) = (&mut even[at..at + STRIDE], &mut odd[at..at + STRIDE]);
            let input = bayer.samples(top + row, left..left + width);
            let (pairs, rest) = input.as_chunks::<2>();
            if let [last] = rest {
                even[width / 2] = last * even_gain;
            }
            for ((pair, even), odd) in pairs.iter().zip(even).zip(odd) {
                *even = pair[0] * even_gain;
                *odd = pair[1] * odd_gain;
            }
        }
        // The column phase of the red and blue sites of each row parity.
        let red_blue = [
            pattern.color_at(Vec2us::new(0, 0)).index() & 1,
            pattern.color_at(Vec2us::new(0, 1)).index() & 1,
        ];
        Stages {
            tile: self,
            crop: Crop {
                size,
                pattern,
                red_blue,
            },
        }
        .dispatch();
        let border = INTERPOLATED_BORDER;
        let frame_width = bayer.frame_width();
        for row in border..height - border {
            let input = bayer.samples(top + row, left..left + width);
            let at = (row / 2) * STRIDE;
            for (channel, planes) in self.rgb.iter().enumerate() {
                for column in 0..2 {
                    let phase = 2 * (row % 2) + column;
                    let line = self.line[..width].iter_mut().skip(column).step_by(2);
                    if phase_color(pattern, phase) == channel {
                        for (out, &sample) in line.zip(input.iter().skip(column).step_by(2)) {
                            *out = sample;
                        }
                    } else {
                        for (line, &value) in line.zip(&planes[phase][at..at + STRIDE]) {
                            *line = out.unbalanced(channel, value);
                        }
                    }
                }
                let index = (top + row) * frame_width + left + border;
                unsafe { out.write_row(channel, index, &self.line[border..width - border]) };
            }
        }
    }

    /// Step 1: the vertical-against-horizontal direction map, from the squared high-pass filters
    /// summed over three pixels along each axis.
    #[inline(always)]
    fn directions<S: Isa>(&mut self, isa: S, Size2us { width, height }: Size2us) {
        let cfa = view(&self.cfa);
        let (three, six, epssq) = (isa.splat_f32(3.0), isa.splat_f32(6.0), isa.splat_f32(EPSSQ));
        // Each row's vertical filter as it comes into reach, and a row's direction map once the
        // rows on both sides have theirs.
        for row in (BORDER - 1)..=(height - BORDER) {
            for (column, v_hpf) in self.v_hpf[row % 3].iter_mut().enumerate() {
                let [m3, m2, m1, centre, p1, p2, p3] = Tap::all(
                    cfa,
                    column,
                    [
                        (row - 3, 0),
                        (row - 2, 0),
                        (row - 1, 0),
                        (row, 0),
                        (row + 1, 0),
                        (row + 2, 0),
                        (row + 3, 0),
                    ],
                );
                for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                    let j = span.start;
                    let line = [m3, m2, m1, centre, p1, p2, p3].map(|tap| tap.load(isa, j));
                    squared_high_pass(line, three, six).store_partial(&mut v_hpf[span]);
                }
            }
            if row <= BORDER {
                continue;
            }
            let row = row - 1;
            for (column, h_hpf) in self.h_hpf.iter_mut().enumerate() {
                let [m3, m2, m1, centre, p1, p2, p3] = Tap::all(
                    cfa,
                    column,
                    [
                        (row, -3),
                        (row, -2),
                        (row, -1),
                        (row, 0),
                        (row, 1),
                        (row, 2),
                        (row, 3),
                    ],
                );
                for span in Vectors::over(columns(column, BORDER - 1..width - BORDER + 1)) {
                    let j = span.start;
                    let line = [m3, m2, m1, centre, p1, p2, p3].map(|tap| tap.load(isa, j));
                    squared_high_pass(line, three, six).store_partial(&mut h_hpf[span]);
                }
            }
            let [above, here, below] = [row - 1, row, row + 1];
            let h_row = view(&self.h_hpf);
            for column in 0..2 {
                let up = Tap::new(view(&self.v_hpf[above % 3]), column, 0);
                let centre_v = Tap::new(view(&self.v_hpf[here % 3]), column, 0);
                let down = Tap::new(view(&self.v_hpf[below % 3]), column, 0);
                let left = Tap::new(h_row, column, -1);
                let centre_h = Tap::new(h_row, column, 0);
                let right = Tap::new(h_row, column, 1);
                let out = row_mut(&mut self.vh_dir[2 * (row % 2) + column], row);
                for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                    let j = span.start;
                    let v_stat = up.load(isa, j) + centre_v.load(isa, j) + down.load(isa, j);
                    let h_stat = left.load(isa, j) + centre_h.load(isa, j) + right.load(isa, j);
                    direction_ratio(v_stat, h_stat, epssq).store_partial(&mut out[span]);
                }
            }
        }
    }

    /// Steps 2 and 3: the low-pass filter at red and blue sites, then green there from the ratio
    /// of its neighbours' green to the filter, blended along the direction map.
    #[inline(always)]
    fn green<S: Isa>(&mut self, isa: S, crop: Crop) {
        let Crop {
            size: Size2us { width, height },
            red_blue,
            ..
        } = crop;
        let cfa = view(&self.cfa);
        let (half, quarter) = (isa.splat_f32(0.5), isa.splat_f32(0.25));
        for row in 2..height - 2 {
            let column = red_blue[row % 2];
            let [n, s, w, centre, e, nw, ne, sw, se] = Tap::all(
                cfa,
                column,
                [
                    (row - 1, 0),
                    (row + 1, 0),
                    (row, -1),
                    (row, 0),
                    (row, 1),
                    (row - 1, -1),
                    (row - 1, 1),
                    (row + 1, -1),
                    (row + 1, 1),
                ],
            );
            let out = row_mut(&mut self.red_blue[row % 2], row);
            for span in Vectors::over(columns(column, 2..width - 2)) {
                let j = span.start;
                let lpf = centre.load(isa, j)
                    + half * (n.load(isa, j) + s.load(isa, j) + w.load(isa, j) + e.load(isa, j))
                    + quarter
                        * (nw.load(isa, j) + ne.load(isa, j) + sw.load(isa, j) + se.load(isa, j));
                lpf.store_partial(&mut out[span]);
            }
        }

        let lpf = red_blue_view(&self.red_blue);
        let vh_dir = view(&self.vh_dir);
        let eps = isa.splat_f32(EPS);
        for row in BORDER..height - BORDER {
            let column = red_blue[row % 2];
            let vertical = Tap::all(
                cfa,
                column,
                [
                    (row - 4, 0),
                    (row - 3, 0),
                    (row - 2, 0),
                    (row - 1, 0),
                    (row + 1, 0),
                    (row + 2, 0),
                    (row + 3, 0),
                    (row + 4, 0),
                ],
            );
            let horizontal = Tap::all(
                cfa,
                column,
                [
                    (row, -4),
                    (row, -3),
                    (row, -2),
                    (row, -1),
                    (row, 0),
                    (row, 1),
                    (row, 2),
                    (row, 3),
                    (row, 4),
                ],
            );
            let [lpf_n, lpf_s, lpf_w, lpf_centre, lpf_e] = Tap::all(
                lpf,
                column,
                [(row - 2, 0), (row + 2, 0), (row, -2), (row, 0), (row, 2)],
            );
            let vh_centre = Tap::at(vh_dir, column, (row, 0));
            let vh_diagonals = diagonals(vh_dir, row, column);
            let out = row_mut(&mut self.rgb[1][2 * (row % 2) + column], row);
            for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                let j = span.start;
                let [n4, n3, n2, n1, s1, s2, s3, s4] = load_all(isa, vertical, j);
                let [w4, w3, w2, w1, cfai, e1, e2, e3, e4] = load_all(isa, horizontal, j);
                // In pairs, as librtprocess sums them.
                let n_grad = eps
                    + ((n1 - s1).abs() + (cfai - n2).abs())
                    + ((n1 - n3).abs() + (n2 - n4).abs());
                let s_grad = eps
                    + ((n1 - s1).abs() + (cfai - s2).abs())
                    + ((s1 - s3).abs() + (s2 - s4).abs());
                let w_grad = eps
                    + ((w1 - e1).abs() + (cfai - w2).abs())
                    + ((w1 - w3).abs() + (w2 - w4).abs());
                let e_grad = eps
                    + ((w1 - e1).abs() + (cfai - e2).abs())
                    + ((e1 - e3).abs() + (e2 - e4).abs());

                let lpfi = lpf_centre.load(isa, j);
                let estimate = |green, lpf: Tap<'_>| {
                    let lpf = lpf.load(isa, j);
                    estimate_green(isa, lpfi, Neighbour { green, lpf })
                };
                let n_est = estimate(n1, lpf_n);
                let s_est = estimate(s1, lpf_s);
                let w_est = estimate(w1, lpf_w);
                let e_est = estimate(e1, lpf_e);

                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

                let vh_disc = discriminate(
                    isa,
                    vh_centre.load(isa, j),
                    vh_neighbourhood(isa, load_all(isa, vh_diagonals, j)),
                );
                intp(vh_disc, v_est, h_est).store_partial(&mut out[span]);
            }
        }
    }

    /// Steps 4.0 and 4.1: the diagonal direction map at red and blue sites, from the squared
    /// diagonal high-pass filters summed over each site and its two neighbours along the diagonal,
    /// as RCD 2.3's closed forms do. librtprocess keeps the filter on odd columns only, so its
    /// step 4.1 reads some of the three beside the diagonal.
    #[inline(always)]
    fn diagonal_directions<S: Isa>(&mut self, isa: S, crop: Crop) {
        let Crop {
            size: Size2us { width, height },
            red_blue,
            ..
        } = crop;
        let cfa = view(&self.cfa);
        let (three, six, epssq) = (isa.splat_f32(3.0), isa.splat_f32(6.0), isa.splat_f32(EPSSQ));
        for row in 3..height - 3 {
            let column = red_blue[row % 2];
            let centre = Tap::at(cfa, column, (row, 0));
            // The P diagonal runs down to the right, the Q one down to the left.
            let [p_m3, p_m2, p_m1, p_p1, p_p2, p_p3] = Tap::all(
                cfa,
                column,
                [
                    (row - 3, -3),
                    (row - 2, -2),
                    (row - 1, -1),
                    (row + 1, 1),
                    (row + 2, 2),
                    (row + 3, 3),
                ],
            );
            let [q_m3, q_m2, q_m1, q_p1, q_p2, q_p3] = Tap::all(
                cfa,
                column,
                [
                    (row - 3, 3),
                    (row - 2, 2),
                    (row - 1, 1),
                    (row + 1, -1),
                    (row + 2, -2),
                    (row + 3, -3),
                ],
            );
            let p_hpf = &mut self.p_hpf[row % 3];
            let q_hpf = &mut self.q_hpf[row % 3];
            for span in Vectors::over(columns(column, 3..width - 3)) {
                let j = span.start;
                let c = centre.load(isa, j);
                let [p_m3, p_m2, p_m1, p_p1, p_p2, p_p3] =
                    [p_m3, p_m2, p_m1, p_p1, p_p2, p_p3].map(|tap| tap.load(isa, j));
                squared_high_pass([p_m3, p_m2, p_m1, c, p_p1, p_p2, p_p3], three, six)
                    .store_partial(&mut p_hpf[span.clone()]);
                let [q_m3, q_m2, q_m1, q_p1, q_p2, q_p3] =
                    [q_m3, q_m2, q_m1, q_p1, q_p2, q_p3].map(|tap| tap.load(isa, j));
                squared_high_pass([q_m3, q_m2, q_m1, c, q_p1, q_p2, q_p3], three, six)
                    .store_partial(&mut q_hpf[span]);
            }
            if row <= BORDER {
                continue;
            }
            let row = row - 1;
            let column = red_blue[row % 2];
            let p_nw = Tap::new(ring_row(&self.p_hpf, row - 1), column, -1);
            let p_centre = Tap::new(ring_row(&self.p_hpf, row), column, 0);
            let p_se = Tap::new(ring_row(&self.p_hpf, row + 1), column, 1);
            let q_ne = Tap::new(ring_row(&self.q_hpf, row - 1), column, 1);
            let q_centre = Tap::new(ring_row(&self.q_hpf, row), column, 0);
            let q_sw = Tap::new(ring_row(&self.q_hpf, row + 1), column, -1);
            let out = row_mut(&mut self.red_blue[row % 2], row);
            for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                let j = span.start;
                let p_stat = p_nw.load(isa, j) + p_centre.load(isa, j) + p_se.load(isa, j);
                let q_stat = q_ne.load(isa, j) + q_centre.load(isa, j) + q_sw.load(isa, j);
                direction_ratio(p_stat, q_stat, epssq).store_partial(&mut out[span]);
            }
        }
    }

    /// Step 4.2: blue at red sites and red at blue ones, from the diagonal color differences
    /// along the diagonal direction map.
    #[inline(always)]
    fn opposite_colors<S: Isa>(&mut self, isa: S, crop: Crop) {
        let Crop {
            size: Size2us { width, height },
            pattern,
            red_blue,
        } = crop;
        let cfa = view(&self.cfa);
        let pq_dir = red_blue_view(&self.red_blue);
        let eps = isa.splat_f32(EPS);
        for (parity, &column) in red_blue.iter().enumerate() {
            let phase = 2 * parity + column;
            // A red site takes blue and a blue one red; the diagonal reads land on that color's
            // own samples.
            let color = 2 - phase_color(pattern, phase);
            let [red, green, blue] = &mut self.rgb;
            let samples = |color| ColorSamples {
                cfa,
                pattern,
                color,
            };
            let green = samples(1).fill(view(green));
            let dst = if color == 0 { red } else { blue };
            let Split { written, read: dst } = Split::of(dst, phase, samples(color));
            for row in (BORDER + parity..height - BORDER).step_by(2) {
                let pq_centre = Tap::at(pq_dir, column, (row, 0));
                let pq_diagonals = diagonals(pq_dir, row, column);
                let [d_nw, d_ne, d_sw, d_se] = diagonals(dst, row, column);
                let [d_nw3, d_ne3, d_sw3, d_se3] = Tap::all(
                    dst,
                    column,
                    [(row - 3, -3), (row - 3, 3), (row + 3, -3), (row + 3, 3)],
                );
                let [g_centre, g_nw2, g_ne2, g_sw2, g_se2] = Tap::all(
                    green,
                    column,
                    [
                        (row, 0),
                        (row - 2, -2),
                        (row - 2, 2),
                        (row + 2, -2),
                        (row + 2, 2),
                    ],
                );
                let [g_nw, g_ne, g_sw, g_se] = diagonals(green, row, column);
                let out = row_mut(written, row);
                for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                    let j = span.start;
                    let pq_disc = discriminate(
                        isa,
                        pq_centre.load(isa, j),
                        pq_neighbourhood(isa, load_all(isa, pq_diagonals, j)),
                    );
                    let [d_nw, d_ne, d_sw, d_se] = load_all(isa, [d_nw, d_ne, d_sw, d_se], j);
                    let g = g_centre.load(isa, j);

                    let nw_grad = eps
                        + (d_nw - d_se).abs()
                        + (d_nw - d_nw3.load(isa, j)).abs()
                        + (g - g_nw2.load(isa, j)).abs();
                    let ne_grad = eps
                        + (d_ne - d_sw).abs()
                        + (d_ne - d_ne3.load(isa, j)).abs()
                        + (g - g_ne2.load(isa, j)).abs();
                    let sw_grad = eps
                        + (d_ne - d_sw).abs()
                        + (d_sw - d_sw3.load(isa, j)).abs()
                        + (g - g_sw2.load(isa, j)).abs();
                    let se_grad = eps
                        + (d_nw - d_se).abs()
                        + (d_se - d_se3.load(isa, j)).abs()
                        + (g - g_se2.load(isa, j)).abs();

                    let nw_est = d_nw - g_nw.load(isa, j);
                    let ne_est = d_ne - g_ne.load(isa, j);
                    let sw_est = d_sw - g_sw.load(isa, j);
                    let se_est = d_se - g_se.load(isa, j);

                    let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
                    let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

                    (g + intp(pq_disc, p_est, q_est)).store_partial(&mut out[span]);
                }
            }
        }
    }

    /// Step 4.3: red and blue at green sites, from the color differences of their four
    /// neighbours along the direction map.
    #[inline(always)]
    fn colors_at_green<S: Isa>(&mut self, isa: S, crop: Crop) {
        let Crop {
            size: Size2us { width, height },
            pattern,
            red_blue,
        } = crop;
        let cfa = view(&self.cfa);
        let vh_dir = view(&self.vh_dir);
        let eps = isa.splat_f32(EPS);
        for (parity, &red_blue) in red_blue.iter().enumerate() {
            let column = 1 - red_blue;
            let phase = 2 * parity + column;
            let [red, green, blue] = &mut self.rgb;
            let samples = |color| ColorSamples {
                cfa,
                pattern,
                color,
            };
            let green = samples(1).fill(view(green));
            let mut red = Split::of(red, phase, samples(0));
            let mut blue = Split::of(blue, phase, samples(2));
            for row in (BORDER + parity..height - BORDER).step_by(2) {
                let vh_centre = Tap::at(vh_dir, column, (row, 0));
                let vh_diagonals = diagonals(vh_dir, row, column);
                let [g_centre, g_n2, g_s2, g_w2, g_e2] = Tap::all(
                    green,
                    column,
                    [(row, 0), (row - 2, 0), (row + 2, 0), (row, -2), (row, 2)],
                );
                let g_neighbours = Tap::all(
                    green,
                    column,
                    [(row - 1, 0), (row + 1, 0), (row, -1), (row, 1)],
                );
                let mut planes = [
                    Neighbours::new(&mut red, row, column),
                    Neighbours::new(&mut blue, row, column),
                ];
                for span in Vectors::over(columns(column, BORDER..width - BORDER)) {
                    let j = span.start;
                    let vh_disc = discriminate(
                        isa,
                        vh_centre.load(isa, j),
                        vh_neighbourhood(isa, load_all(isa, vh_diagonals, j)),
                    );
                    let g = g_centre.load(isa, j);
                    let n1 = eps + (g - g_n2.load(isa, j)).abs();
                    let s1 = eps + (g - g_s2.load(isa, j)).abs();
                    let w1 = eps + (g - g_w2.load(isa, j)).abs();
                    let e1 = eps + (g - g_e2.load(isa, j)).abs();
                    let [g_n, g_s, g_w, g_e] = load_all(isa, g_neighbours, j);

                    for plane in &mut planes {
                        let [p_n3, p_n, p_s, p_s3, p_w3, p_w, p_e, p_e3] = load_all(
                            isa,
                            [
                                plane.n3, plane.n1, plane.s1, plane.s3, plane.w3, plane.w1,
                                plane.e1, plane.e3,
                            ],
                            j,
                        );
                        let sn_abs = (p_n - p_s).abs();
                        let ew_abs = (p_w - p_e).abs();
                        let n_grad = n1 + sn_abs + (p_n - p_n3).abs();
                        let s_grad = s1 + sn_abs + (p_s - p_s3).abs();
                        let w_grad = w1 + ew_abs + (p_w - p_w3).abs();
                        let e_grad = e1 + ew_abs + (p_e - p_e3).abs();

                        let n_est = p_n - g_n;
                        let s_est = p_s - g_s;
                        let w_est = p_w - g_w;
                        let e_est = p_e - g_e;

                        let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                        let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                        (g + intp(vh_disc, v_est, h_est))
                            .store_partial(&mut plane.out[span.clone()]);
                    }
                }
            }
        }
    }
}

impl Kernel for Stages<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self { tile, crop } = self;
        tile.directions(isa, crop.size);
        tile.green(isa, crop);
        tile.diagonal_directions(isa, crop);
        tile.opposite_colors(isa, crop);
        tile.colors_at_green(isa, crop);
    }
}

impl<'a> Tap<'a> {
    /// The photosite `dx` crop columns from each site of column phase `column`, in the row whose
    /// two column phases are `rows`.
    #[inline(always)]
    const fn new(rows: [&'a [f32]; 2], column: usize, dx: isize) -> Self {
        let col = column as isize + dx;
        Self {
            row: rows[col.rem_euclid(2) as usize],
            offset: col.div_euclid(2),
        }
    }

    /// The photosite `dx` crop columns from each site of column phase `column` in crop row `row`
    /// of `planes`, as one of [`Tap::all`]'s steps.
    #[inline(always)]
    fn at(planes: [&'a [f32]; 4], column: usize, (row, dx): (usize, isize)) -> Self {
        let col = column as isize + dx;
        let start = (row / 2) * STRIDE;
        Self {
            row: &planes[2 * (row % 2) + col.rem_euclid(2) as usize][start..start + STRIDE],
            offset: col.div_euclid(2),
        }
    }

    /// [`Tap::at`] each of `steps`, a crop row and a column step.
    #[inline(always)]
    fn all<const N: usize>(
        planes: [&'a [f32]; 4],
        column: usize,
        steps: [(usize, isize); N],
    ) -> [Self; N] {
        let mut taps = [Self {
            row: &[],
            offset: 0,
        }; N];
        for (tap, (row, dx)) in taps.iter_mut().zip(steps) {
            *tap = Self::at(planes, column, (row, dx));
        }
        taps
    }

    /// The photosites of the [`F32_LANES`] sites from plane column `j`.
    #[inline(always)]
    fn load<S: Isa>(self, isa: S, j: usize) -> S::F32 {
        isa.load_f32_at(self.row, j.wrapping_add_signed(self.offset))
    }
}

impl<'a> Split<'a> {
    /// A color's `planes` with phase `phase` taken out to write, its own phases read from its
    /// `samples`.
    #[inline(always)]
    fn of(planes: &'a mut [Vec<f32>; 4], phase: usize, samples: ColorSamples<'a>) -> Self {
        let mut written = None;
        let mut read: [&[f32]; 4] = [&[]; 4];
        for (index, plane) in planes.iter_mut().enumerate() {
            if index == phase {
                written = Some(plane.as_mut_slice());
            } else {
                read[index] = plane;
            }
        }
        Self {
            written: written.expect("a Bayer phase"),
            read: samples.fill(read),
        }
    }
}

impl<'a> Neighbours<'a> {
    /// The neighbours in `color` of the green sites of column phase `column` in crop row `row`.
    #[inline(always)]
    fn new(color: &'a mut Split<'_>, row: usize, column: usize) -> Self {
        let [n3, n1, s1, s3, w3, w1, e1, e3] = Tap::all(
            color.read,
            column,
            [
                (row - 3, 0),
                (row - 1, 0),
                (row + 1, 0),
                (row + 3, 0),
                (row, -3),
                (row, -1),
                (row, 1),
                (row, 3),
            ],
        );
        Self {
            n3,
            n1,
            s1,
            s3,
            w3,
            w1,
            e1,
            e3,
            out: row_mut(color.written, row),
        }
    }
}

impl Vectors {
    #[inline(always)]
    const fn over(columns: Range<usize>) -> Self {
        Self {
            next: columns.start,
            end: columns.end,
        }
    }
}

impl Iterator for Vectors {
    type Item = Range<usize>;

    #[inline(always)]
    fn next(&mut self) -> Option<Range<usize>> {
        let start = self.next;
        if start >= self.end {
            return None;
        }
        self.next = start + F32_LANES;
        Some(start..self.next.min(self.end))
    }
}

/// RCD's squared high-pass filter along a line of seven samples `[m3, m2, m1, c, p1, p2, p3]`:
/// `((m3 − m1 − p1 + p3) − 3·(m2 + p2) + 6·c)²`.
#[inline(always)]
fn squared_high_pass<V: F32x8>(line: [V; 7], three: V, six: V) -> V {
    let [m3, m2, m1, centre, p1, p2, p3] = line;
    let filter = (m3 - m1 - p1 + p3) - three * (m2 + p2) + six * centre;
    filter * filter
}

/// The share `a / (a + b)` of one direction's statistic, each statistic held to at least `floor`.
#[inline(always)]
fn direction_ratio<V: F32x8>(a: V, b: V, floor: V) -> V {
    let (a, b) = (a.max(floor), b.max(floor));
    a / (a + b)
}

/// The color of a frame of `pattern` at Bayer phase `phase`.
#[inline(always)]
const fn phase_color(pattern: CfaPattern, phase: usize) -> usize {
    pattern.color_at(Vec2us::new(phase % 2, phase / 2)).index()
}

/// Each of `taps` at the sites from plane column `j`.
#[inline(always)]
fn load_all<S: Isa, const N: usize>(isa: S, taps: [Tap<'_>; N], j: usize) -> [S::F32; N] {
    let mut values = [isa.splat_f32(0.0); N];
    for (value, tap) in values.iter_mut().zip(taps) {
        *value = tap.load(isa, j);
    }
    values
}

/// `planes` as [`Tap::at`] reads them.
#[inline(always)]
fn view<const N: usize>(planes: &[Vec<f32>; N]) -> [&[f32]; N] {
    let mut view: [&[f32]; N] = [&[]; N];
    for (slice, plane) in view.iter_mut().zip(planes) {
        *slice = plane;
    }
    view
}

impl<'a> ColorSamples<'a> {
    /// `planes` of the color, with its own samples at its own phases.
    #[inline(always)]
    fn fill(self, mut planes: [&'a [f32]; 4]) -> [&'a [f32]; 4] {
        for (phase, (plane, samples)) in planes.iter_mut().zip(self.cfa).enumerate() {
            if phase_color(self.pattern, phase) == self.color {
                *plane = samples;
            }
        }
        planes
    }
}

/// A set of red-and-blue planes, one per row parity, as [`Tap::at`] reads a set of four: only the
/// red and blue column phase of each row is read.
#[inline(always)]
fn red_blue_view(planes: &[Vec<f32>; 2]) -> [&[f32]; 4] {
    let [even, odd] = view(planes);
    [even, even, odd, odd]
}

/// Crop row `row` of a ring of the last three rows' red and blue sites, as [`Tap::new`] reads a
/// row's two column phases.
#[inline(always)]
const fn ring_row(rows: &[Vec<f32>; 3], row: usize) -> [&[f32]; 2] {
    let row = rows[row % 3].as_slice();
    [row, row]
}

/// Crop row `row` of `plane`, one plane of a set, to write.
#[inline(always)]
fn row_mut(plane: &mut [f32], row: usize) -> &mut [f32] {
    let start = (row / 2) * STRIDE;
    &mut plane[start..start + STRIDE]
}

/// The four diagonal neighbours of the sites of column phase `column` in crop row `row`: NW, NE,
/// SW, SE.
#[inline(always)]
fn diagonals(planes: [&[f32]; 4], row: usize, column: usize) -> [Tap<'_>; 4] {
    Tap::all(
        planes,
        column,
        [(row - 1, -1), (row - 1, 1), (row + 1, -1), (row + 1, 1)],
    )
}

/// The plane columns of the sites of column phase `column` among crop columns `cols`.
#[inline(always)]
const fn columns(column: usize, cols: Range<usize>) -> Range<usize> {
    cols.start.saturating_sub(column).div_ceil(2)..cols.end.saturating_sub(column).div_ceil(2)
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    impl Tile {
        /// Every buffer filled with `value`, as a tile finds them after other tiles.
        pub(crate) fn poison(&mut self, value: f32) {
            for buffer in self
                .cfa
                .iter_mut()
                .chain(&mut self.vh_dir)
                .chain(&mut self.red_blue)
                .chain(self.rgb.iter_mut().flatten())
                .chain(self.v_hpf.iter_mut().flatten())
                .chain(&mut self.h_hpf)
                .chain(&mut self.p_hpf)
                .chain(&mut self.q_hpf)
                .chain([&mut self.line])
            {
                buffer.fill(value);
            }
        }
    }
}
