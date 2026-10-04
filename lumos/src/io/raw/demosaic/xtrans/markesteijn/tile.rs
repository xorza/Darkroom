//! [`Tile`]: one tile of the Markesteijn demosaic and the buffers it works in.

use std::ops::Range;

use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::markesteijn::hex_table::HexTable;
use crate::io::raw::demosaic::xtrans::markesteijn::{OutputPlanes, TILE};

/// Half the tile, the width of the green bounds, which hold one entry per pair of columns.
const HALF_TILE: usize = TILE / 2;
/// The side of the perceptual planes: the tile less four pixels each side.
const YUV_SIDE: usize = TILE - 8;
/// The side of the derivative planes: the tile less five pixels each side.
const DRV_SIDE: usize = TILE - 10;
/// librtprocess's `0.33333333f`, the f32 nearest a third.
const THIRD: f32 = 0.333_333_34;

/// The working buffers of one tile, kept from tile to tile by the worker that holds it.
///
/// Laid out as librtprocess lays its tile out, each in its own buffer rather than aliased: per
/// direction, the tile's RGB; its perceptual planes; per direction, their derivatives, the
/// homogeneity counts and their 5×5 sums; the largest sum; and the green bounds.
#[derive(Debug)]
pub(super) struct Tile {
    directions: usize,
    rgb: Vec<[f32; 3]>,
    yuv: Vec<f32>,
    drv: Vec<f32>,
    homo: Vec<u8>,
    homosum: Vec<u8>,
    homosummax: Vec<u8>,
    green_bounds: Vec<[f32; 2]>,
    /// The colour differences of a solitary green pixel, by colour and direction.
    dcolor: [[f32; 6]; 3],
}

/// Where a tile lies in the frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct TilePlace {
    pub(super) top: usize,
    pub(super) left: usize,
}

/// The rows and columns of a tile, in its own coordinates, that it computes in full and writes.
#[derive(Debug)]
struct OwnedPart {
    rows: Range<usize>,
    cols: Range<usize>,
}

impl Tile {
    pub(super) fn new(directions: usize) -> Self {
        Self {
            directions,
            rgb: vec![[0.0; 3]; directions * TILE * TILE],
            yuv: vec![0.0; 3 * YUV_SIDE * YUV_SIDE],
            drv: vec![0.0; directions * DRV_SIDE * DRV_SIDE],
            homo: vec![0; directions * TILE * TILE],
            homosum: vec![0; directions * TILE * TILE],
            homosummax: vec![0; TILE * TILE],
            green_bounds: vec![[0.0; 2]; TILE * HALF_TILE],
            dcolor: [[0.0; 6]; 3],
        }
    }

    /// The bytes one tile's buffers hold at `directions`.
    pub(super) const fn bytes(directions: usize) -> usize {
        directions * TILE * TILE * size_of::<[f32; 3]>()
            + 3 * YUV_SIDE * YUV_SIDE * size_of::<f32>()
            + directions * DRV_SIDE * DRV_SIDE * size_of::<f32>()
            + 2 * directions * TILE * TILE
            + TILE * TILE
            + TILE * HALF_TILE * size_of::<[f32; 2]>()
    }

    /// Demosaic the tile at `place` of `xtrans` with `passes` passes, and write the part of it
    /// that is its own into `out`.
    ///
    /// # Safety
    ///
    /// `out` covers the whole frame, and no other tile writes the pixels this one owns: the rows
    /// and columns from `margin` past its start to `margin` before its end, which tile the frame
    /// without overlap at the stride of `TILE − 2·margin`.
    pub(super) unsafe fn demosaic(
        &mut self,
        xtrans: &XTransImage<'_>,
        hex: &HexTable,
        place: TilePlace,
        passes: usize,
        margin: usize,
        out: OutputPlanes,
    ) {
        let TilePlace { top, left } = place;
        let width = xtrans.size.width;
        let height = xtrans.size.height;
        let mrow = (top + TILE).min(height - 3);
        let mcol = (left + TILE).min(width - 3);
        self.seed(xtrans, hex, place, mrow, mcol);
        self.interpolate(xtrans, hex, place, passes, mrow, mcol);
        let own = OwnedPart {
            rows: margin..mrow - top - margin,
            cols: margin..mcol - left - margin,
        };
        self.derivatives(mrow - top, mcol - left);
        self.homogeneity(mrow - top, mcol - left);
        self.homogeneity_sums(&own);
        // SAFETY: the caller hands over the frame's planes and the region this tile alone owns.
        unsafe { self.blend(place, &own, width, out) };
    }

    /// Each direction's colours from the seeded samples: green, then `passes` passes of red and
    /// blue.
    fn interpolate(
        &mut self,
        xtrans: &XTransImage<'_>,
        hex: &HexTable,
        place: TilePlace,
        passes: usize,
        mrow: usize,
        mcol: usize,
    ) {
        self.green_bounds(xtrans, hex, place, mrow, mcol);
        self.interpolate_green(xtrans, hex, place, mrow, mcol);
        for pass in 0..passes {
            let base = if pass == 0 { 0 } else { 4 };
            if pass == 1 {
                self.rgb.copy_within(0..4 * TILE * TILE, 4 * TILE * TILE);
            }
            if pass > 0 {
                self.refine_green(hex, place, mrow, mcol, base);
            }
            self.solitary_green_colours(hex, place, mrow, mcol, base);
            self.opposite_colours(hex, place, mrow, mcol, base);
            self.green_block_colours(hex, place, mrow, mcol, base);
        }
    }

    #[inline(always)]
    const fn at(d: usize, row: usize, col: usize) -> usize {
        (d * TILE + row) * TILE + col
    }

    /// The least and the greatest green of each non-green pixel's hexagon, which bound its
    /// interpolated green.
    fn green_bounds(
        &mut self,
        xtrans: &XTransImage<'_>,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
    ) {
        let width = xtrans.size.width;
        let bounds = |row: usize, col: usize, offsets: &[isize; 8]| {
            let pix = row * width + col;
            let mut minval = f32::MAX;
            let mut maxval = 0.0f32;
            for &offset in &offsets[..6] {
                let val = xtrans.data[pix.wrapping_add_signed(offset)];
                minval = if minval < val { minval } else { val };
                maxval = if maxval > val { maxval } else { val };
            }
            [minval, maxval]
        };
        for row in top..mrow {
            let leftstart = (left..mcol)
                .find(|&col| !hex.is_green(row, col))
                .unwrap_or(mcol);
            let coloffset = if hex.right_shift(row) {
                3
            } else {
                1 + usize::from(hex.colour(row, leftstart + 1) & 1)
            };
            let entry = |col: usize| (row - top) * HALF_TILE + ((col - left) >> 1);
            if coloffset == 3 {
                let offsets = hex.image(row, leftstart);
                let mut col = leftstart;
                while col < mcol {
                    self.green_bounds[entry(col)] = bounds(row, col, offsets);
                    col += coloffset;
                }
            } else {
                let mut col = leftstart;
                if coloffset == 2 {
                    self.green_bounds[entry(col)] = bounds(row, col, hex.image(row, col));
                    col += 2;
                }
                let offsets = hex.image(row, col);
                while col + 1 < mcol {
                    let value = bounds(row, col, offsets);
                    self.green_bounds[entry(col)] = value;
                    self.green_bounds[entry(col + 1)] = value;
                    col += 3;
                }
                if col < mcol {
                    self.green_bounds[entry(col)] = bounds(row, col, offsets);
                }
            }
        }
    }

    /// The tile's samples, each in its own colour, in the first four directions.
    fn seed(
        &mut self,
        xtrans: &XTransImage<'_>,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
    ) {
        let plane = TILE * TILE;
        self.rgb[..plane].fill([0.0; 3]);
        for row in top..mrow {
            for col in left..mcol {
                let colour = usize::from(hex.colour(row, col));
                self.rgb[Self::at(0, row - top, col - left)][colour] = xtrans.read(row, col);
            }
        }
        for d in 1..4 {
            self.rgb.copy_within(0..plane, d * plane);
        }
    }

    /// Green at each non-green pixel horizontally, vertically and along both diagonals, held to
    /// its hexagon's bounds.
    fn interpolate_green(
        &mut self,
        xtrans: &XTransImage<'_>,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
    ) {
        let width = xtrans.size.width;
        let colours = |row: usize, col: usize, h: &[isize; 8]| {
            let pix = row * width + col;
            let p = |offset: isize| xtrans.data[pix.wrapping_add_signed(offset)];
            let mut color = [0.0f32; 4];
            color[0] =
                0.679_687_5 * (p(h[1]) + p(h[0])) - 0.179_687_5 * (p(2 * h[1]) + p(2 * h[0]));
            color[1] =
                0.871_093_75 * p(h[3]) + p(h[2]) * 0.128_906_25 + 0.359_375 * (p(0) - p(-h[2]));
            for c in 0..2 {
                color[2 + c] = 0.640_625 * p(h[4 + c])
                    + 0.359_375 * p(-2 * h[4 + c])
                    + 0.128_906_25 * (2.0 * p(0) - p(3 * h[4 + c]) - p(-3 * h[4 + c]));
            }
            color
        };
        for row in top..mrow {
            let leftstart = (left..mcol)
                .find(|&col| !hex.is_green(row, col))
                .unwrap_or(mcol);
            let mut coloffset = if hex.right_shift(row) {
                3
            } else {
                1 + usize::from(hex.colour(row, leftstart + 1) & 1)
            };
            let bounds_entry = |col: usize| (row - top) * HALF_TILE + ((col - left) >> 1);
            if coloffset == 3 {
                let h = hex.image(row, leftstart);
                let mut col = leftstart;
                while col < mcol {
                    let color = colours(row, col, h);
                    let [lo, hi] = self.green_bounds[bounds_entry(col)];
                    for (c, &value) in color.iter().enumerate() {
                        self.rgb[Self::at(c, row - top, col - left)][1] = lim(value, lo, hi);
                    }
                    col += coloffset;
                }
            } else {
                let hexmod = [
                    hex.image(row, leftstart),
                    hex.image(row, leftstart + coloffset),
                ];
                let (mut col, mut hexindex) = (leftstart, 0);
                while col < mcol {
                    let color = colours(row, col, hexmod[hexindex]);
                    let [lo, hi] = self.green_bounds[bounds_entry(col)];
                    for (c, &value) in color.iter().enumerate() {
                        self.rgb[Self::at(c ^ 1, row - top, col - left)][1] = lim(value, lo, hi);
                    }
                    col += coloffset;
                    coloffset ^= 3;
                    hexindex ^= 1;
                }
            }
        }
    }

    /// The second pass's green, from the interpolated values of nearer pixels.
    fn refine_green(
        &mut self,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
        base: usize,
    ) {
        let rgb = &mut self.rgb;
        let bounds = &self.green_bounds;
        let mut refine =
            |plane: usize, row: usize, col: usize, h: &[isize; 8], d: usize, f: usize| {
                let rix = Self::at(plane, row - top, col - left);
                let at = |offset: isize| rgb[rix.wrapping_add_signed(offset)];
                let val = THIRD
                    * (at(-2 * h[d])[1] + 2.0 * (at(h[d])[1] - at(h[d])[f]) - at(-2 * h[d])[f])
                    + at(0)[f];
                let [lo, hi] = bounds[(row - top) * HALF_TILE + ((col - left) >> 1)];
                rgb[rix][1] = lim(val, lo, hi);
            };
        for row in top + 2..mrow - 2 {
            let leftstart = (left + 2..mcol - 2)
                .find(|&col| !hex.is_green(row, col))
                .unwrap_or(mcol - 2);
            let mut coloffset = if hex.right_shift(row) {
                3
            } else {
                1 + usize::from(hex.colour(row, leftstart + 1) & 1)
            };
            let mut f = usize::from(hex.colour(row, leftstart));
            if coloffset == 3 {
                let h = hex.tile(row, leftstart);
                let mut col = leftstart;
                while col < mcol - 2 {
                    for d in 3..6 {
                        refine(base + (d - 2), row, col, h, d, f);
                    }
                    col += coloffset;
                    f ^= 2;
                }
            } else {
                let hexmod = [
                    hex.tile(row, leftstart),
                    hex.tile(row, leftstart + coloffset),
                ];
                let (mut col, mut hexindex) = (leftstart, 0);
                while col < mcol - 2 {
                    for d in 3..6 {
                        refine(base + ((d - 2) ^ 1), row, col, hexmod[hexindex], d, f);
                    }
                    col += coloffset;
                    coloffset ^= 3;
                    f ^= coloffset & 2;
                    hexindex ^= 1;
                }
            }
        }
    }

    /// Red and blue at each solitary green pixel, from the colour differences across it.
    fn solitary_green_colours(
        &mut self,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
        base: usize,
    ) {
        let (sgrow, sgcol) = (hex.sgrow(), hex.sgcol());
        let sgstartcol = (left + 4 - sgcol) / 3 * 3 + sgcol;
        let mut row = (top + 4 - sgrow) / 3 * 3 + sgrow;
        let ts = TILE as isize;
        while row < mrow - 2 {
            let mut col = sgstartcol;
            let mut h = usize::from(hex.colour(row, col + 1));
            while col < mcol - 2 {
                let mut rix = Self::at(base, row - top, col - left);
                let mut diff = [0.0f32; 6];
                let mut i: isize = 1;
                for d in 0..6 {
                    for c in 0..2 {
                        let at = |offset: isize| self.rgb[rix.wrapping_add_signed(offset)];
                        let (plus, minus) = (at(i << c), at(-i << c));
                        let centre = at(0);
                        let g = centre[1] + centre[1] - plus[1] - minus[1];
                        self.dcolor[h][d] = g + plus[h] + minus[h];
                        if d > 1 {
                            let x = plus[1] - minus[1] - plus[h] + minus[h];
                            diff[d] += x * x + g * g;
                        }
                        h ^= 2;
                    }
                    if d > 2 && (d & 1) == 1 && diff[d - 1] < diff[d] {
                        for c in 0..2 {
                            self.dcolor[c * 2][d] = self.dcolor[c * 2][d - 1];
                        }
                    }
                    if (d & 1) == 1 || d < 2 {
                        for c in 0..2 {
                            self.rgb[rix][c * 2] = 0.5 * self.dcolor[c * 2][d];
                        }
                        rix += TILE * TILE;
                    }
                    i ^= ts ^ 1;
                    h ^= 2;
                }
                col += 3;
                h ^= 2;
            }
            row += 3;
        }
    }

    /// Red at blue pixels and blue at red ones, along whichever axis the green varies less.
    fn opposite_colours(
        &mut self,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
        base: usize,
    ) {
        let ts = TILE as isize;
        let sgrow = hex.sgrow();
        let rgb = &mut self.rgb;
        let fill =
            |rgb: &mut Vec<[f32; 3]>, row: usize, col: usize, c: isize, h: isize, f: usize| {
                let mut rix = Self::at(base, row - top, col - left);
                for d in 0..4isize {
                    let at = |offset: isize| rgb[rix.wrapping_add_signed(offset)];
                    let g = at(0)[1];
                    let i = if d > 1
                        || ((d ^ c) & 1) != 0
                        || ((g - at(c)[1]).abs() + (g - at(-c)[1]).abs())
                            < 2.0 * ((g - at(h)[1]).abs() + (g - at(-h)[1]).abs())
                    {
                        c
                    } else {
                        h
                    };
                    let value = g + 0.5 * (at(i)[f] + at(-i)[f] - at(i)[1] - at(-i)[1]);
                    rgb[rix][f] = value;
                    rix += TILE * TILE;
                }
            };
        for row in top + 3..mrow - 3 {
            let leftstart = (left + 3..mcol - 1)
                .find(|&col| !hex.is_green(row, col))
                .unwrap_or(mcol - 1);
            let c = if (row - sgrow).is_multiple_of(3) {
                1
            } else {
                ts
            };
            let h = 3 * (c ^ ts ^ 1);
            let mut f = 2 - usize::from(hex.colour(row, leftstart));
            if hex.right_shift(row) {
                let mut col = leftstart;
                while col < mcol - 3 {
                    fill(rgb, row, col, c, h, f);
                    col += 3;
                    f ^= 2;
                }
            } else {
                let mut coloffset = if hex.colour(row, leftstart + 1) == 1 {
                    2
                } else {
                    1
                };
                let mut col = leftstart;
                while col < mcol - 3 {
                    fill(rgb, row, col, c, h, f);
                    col += coloffset;
                    coloffset ^= 3;
                    f ^= coloffset & 2;
                }
            }
        }
    }

    /// Red and blue for each 2×2 block of green, from the hexagon's neighbours, in each of the
    /// pass's four directions.
    fn green_block_colours(
        &mut self,
        hex: &HexTable,
        TilePlace { top, left }: TilePlace,
        mrow: usize,
        mcol: usize,
        base: usize,
    ) {
        let (sgrow, sgcol) = (hex.sgrow(), hex.sgcol());
        let topstart = (top + 2..mrow - 2)
            .find(|&row| (row - sgrow) % 3 != 0)
            .unwrap_or(mrow - 2);
        let leftstart = (left + 2..mcol - 2)
            .find(|&col| (col - sgcol) % 3 != 0)
            .unwrap_or(mcol - 2);
        let coloffsetstart = 2 - usize::from(hex.colour(topstart, leftstart + 1) & 1);
        for row in topstart..mrow - 2 {
            if (row - sgrow) % 3 == 0 {
                continue;
            }
            let hexmod = [
                hex.tile(row, leftstart),
                hex.tile(row, leftstart + coloffsetstart),
            ];
            let (mut col, mut coloffset, mut hexindex) = (leftstart, coloffsetstart, 0);
            while col < mcol - 2 {
                let h = hexmod[hexindex];
                let mut rix = Self::at(base, row - top, col - left);
                // Each of the pass's four directions takes its pair of the hexagon. dcraw bounds
                // this loop by the direction count, so one pass, of four directions, fills two.
                for d in (0..8).step_by(2) {
                    let at = |offset: isize| self.rgb[rix.wrapping_add_signed(offset)];
                    let (centre, near, far) = (at(0), at(h[d]), at(h[d + 1]));
                    let mut out = centre;
                    if h[d] + h[d + 1] != 0 {
                        let g = 3.0 * centre[1] - 2.0 * near[1] - far[1];
                        for c in [0, 2] {
                            out[c] = (g + 2.0 * near[c] + far[c]) * THIRD;
                        }
                    } else {
                        let g = 2.0 * centre[1] - near[1] - far[1];
                        for c in [0, 2] {
                            out[c] = (g + near[c] + far[c]) * 0.5;
                        }
                    }
                    self.rgb[rix] = out;
                    rix += TILE * TILE;
                }
                col += coloffset;
                coloffset ^= 3;
                hexindex ^= 1;
            }
        }
    }

    /// Every direction's ITU-R BT.2020 YPbPr, and the squared second difference of each along its
    /// own direction.
    fn derivatives(&mut self, mrow: usize, mcol: usize) {
        const DIRECTION_STEPS: [isize; 4] =
            [1, TILE as isize, TILE as isize + 1, TILE as isize - 1];
        let plane = YUV_SIDE * YUV_SIDE;
        for d in 0..self.directions {
            for row in 4..mrow - 4 {
                for col in 4..mcol - 4 {
                    let [r, g, b] = self.rgb[Self::at(d, row, col)];
                    let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
                    let at = (row - 4) * YUV_SIDE + (col - 4);
                    self.yuv[at] = y;
                    self.yuv[plane + at] = (b - y) * 0.56433;
                    self.yuv[2 * plane + at] = (r - y) * 0.67815;
                }
            }
            let step = DIRECTION_STEPS[d & 3];
            let f = if step == 1 { 1 } else { step - 8 };
            for row in 5..mrow - 5 {
                for col in 5..mcol - 5 {
                    let at = (row - 4) * YUV_SIDE + (col - 4);
                    let second = |channel: usize| {
                        let base = channel * plane + at;
                        let v = |offset: isize| self.yuv[base.wrapping_add_signed(offset)];
                        let s = 2.0 * v(0) - v(f) - v(-f);
                        s * s
                    };
                    self.drv[(d * DRV_SIDE + (row - 5)) * DRV_SIDE + (col - 5)] =
                        second(0) + second(1) + second(2);
                }
            }
        }
    }

    /// How many of each pixel's 3×3 neighbours vary least, at most eight times the least, in each
    /// direction.
    fn homogeneity(&mut self, mrow: usize, mcol: usize) {
        let drv = |d: usize, row: usize, col: usize| {
            self.drv[(d * DRV_SIDE + (row - 5)) * DRV_SIDE + (col - 5)]
        };
        for row in 6..mrow - 6 {
            for col in 6..mcol - 6 {
                let mut tr = if drv(0, row, col) < drv(1, row, col) {
                    drv(0, row, col)
                } else {
                    drv(1, row, col)
                };
                for d in 2..self.directions {
                    tr = if drv(d, row, col) < tr {
                        drv(d, row, col)
                    } else {
                        tr
                    };
                }
                tr *= 8.0;
                for d in 0..self.directions {
                    let mut count = 0u8;
                    for v in 0..3 {
                        for h in 0..3 {
                            count += u8::from(drv(d, row + v - 1, col + h - 1) <= tr);
                        }
                    }
                    self.homo[Self::at(d, row, col)] = count;
                }
            }
        }
    }

    /// Each direction's homogeneity summed over 5×5, and the largest sum less an eighth of it,
    /// over the tile's own part.
    fn homogeneity_sums(&mut self, own: &OwnedPart) {
        for d in 0..self.directions {
            for row in own.rows.clone() {
                for col in own.cols.clone() {
                    let mut sum = 0u32;
                    for v in 0..5 {
                        for h in 0..5 {
                            sum += u32::from(self.homo[Self::at(d, row + v - 2, col + h - 2)]);
                        }
                    }
                    self.homosum[Self::at(d, row, col)] =
                        u8::try_from(sum).expect("25 counts of at most 9");
                }
            }
        }
        for row in own.rows.clone() {
            for col in own.cols.clone() {
                let mut maxval = self.homosum[Self::at(0, row, col)];
                for d in 1..self.directions {
                    maxval = maxval.max(self.homosum[Self::at(d, row, col)]);
                }
                self.homosummax[row * TILE + col] = maxval - (maxval >> 3);
            }
        }
    }

    /// The mean of the directions within an eighth of the most homogeneous, into `out`.
    ///
    /// # Safety
    ///
    /// As [`Self::demosaic`].
    unsafe fn blend(
        &self,
        TilePlace { top, left }: TilePlace,
        own: &OwnedPart,
        width: usize,
        out: OutputPlanes,
    ) {
        for row in own.rows.clone() {
            for col in own.cols.clone() {
                let mut hm = [0u8; 8];
                for (d, sum) in hm.iter_mut().enumerate().take(self.directions) {
                    *sum = self.homosum[Self::at(d, row, col)];
                }
                for d in 4..self.directions {
                    if hm[d - 4] < hm[d] {
                        hm[d - 4] = 0;
                    } else if hm[d - 4] > hm[d] {
                        hm[d] = 0;
                    }
                }
                let maxval = self.homosummax[row * TILE + col];
                let mut avg = [0.0f32; 4];
                for (d, &sum) in hm.iter().enumerate().take(self.directions) {
                    if sum >= maxval {
                        let rgb = self.rgb[Self::at(d, row, col)];
                        for c in 0..3 {
                            avg[c] += rgb[c];
                        }
                        avg[3] += 1.0;
                    }
                }
                let index = (row + top) * width + col + left;
                // SAFETY: as this function's contract.
                unsafe {
                    out.r.get().add(index).write(avg[0] / avg[3]);
                    out.g.get().add(index).write(avg[1] / avg[3]);
                    out.b.get().add(index).write(avg[2] / avg[3]);
                }
            }
        }
    }
}

/// librtprocess's `LIM(x, lo, hi)`: `max(lo, min(x, hi))`, each by `<`.
#[inline(always)]
fn lim(x: f32, lo: f32, hi: f32) -> f32 {
    let low = if hi < x { hi } else { x };
    if lo < low { low } else { lo }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    impl Tile {
        /// The tile's colours in every direction after `passes`, from samples seeded as
        /// [`Tile::demosaic`] seeds them, but with every channel no sample gives set to `poison`:
        /// a cell that no stage computes keeps or spreads it.
        pub(crate) fn interpolate_poisoned(
            &mut self,
            xtrans: &XTransImage<'_>,
            hex: &HexTable,
            place: TilePlace,
            passes: usize,
            poison: f32,
        ) -> &[[f32; 3]] {
            let mrow = (place.top + TILE).min(xtrans.size.height - 3);
            let mcol = (place.left + TILE).min(xtrans.size.width - 3);
            self.seed(xtrans, hex, place, mrow, mcol);
            for d in 0..4 {
                for row in 0..TILE {
                    for col in 0..TILE {
                        let colour = usize::from(hex.colour(place.top + row, place.left + col));
                        for (channel, value) in
                            self.rgb[Self::at(d, row, col)].iter_mut().enumerate()
                        {
                            if channel != colour {
                                *value = poison;
                            }
                        }
                    }
                }
            }
            self.interpolate(xtrans, hex, place, passes, mrow, mcol);
            &self.rgb
        }
    }
}
