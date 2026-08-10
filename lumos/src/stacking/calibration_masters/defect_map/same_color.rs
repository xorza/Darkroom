//! Repairing a defective pixel from its same-colour neighbours.
//!
//! Correction runs on raw CFA data, so a defect can only be replaced from pixels behind the same
//! filter — interpolating across colours would corrupt the mosaic for the demosaic that follows.
//! Where those neighbours sit depends on the pattern: 8-connected for mono, stride 2 for any Bayer
//! phase, and for X-Trans a per-phase table precomputed once, since recomputing the 13×13 colour
//! sweep and its distance sort per pixel dominated the cold-pixel scan.

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::math::statistics::median_f32_mut;
use crate::math::vec2us::Vec2us;

/// Mono: 8-connected neighbours, every one of which is the same "colour".
const MONO_OFFSETS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Bayer: same-colour neighbours sit at stride 2 in each axis, true for every 2×2 phase.
const BAYER_OFFSETS: [(i32, i32); 8] = [
    (-2, 0),
    (2, 0),
    (0, -2),
    (0, 2),
    (-2, -2),
    (-2, 2),
    (2, -2),
    (2, 2),
];

/// Median of the neighbours at `offsets` from `pos`, skipping those outside the frame and those
/// flagged in `defect_mask` so a defect is never repaired from another defect. Falls back to the
/// centre pixel when every candidate is rejected.
///
/// Gathers at most `N`. Mono and Bayer pass exactly `N` offsets so the cap never binds; X-Trans
/// passes more, ordered nearest-first, which turns the cap into "the closest `N` valid
/// neighbours". One walk for all three because they differ only in where the offsets come from.
fn median_of_neighbors<const N: usize>(
    pixels: &Buffer2<f32>,
    pos: Vec2us,
    offsets: impl IntoIterator<Item = (i32, i32)>,
    defect_mask: Option<&BitBuffer2>,
) -> f32 {
    let width = pixels.width() as i32;
    let height = pixels.height() as i32;
    let mut neighbors = [0.0f32; N];
    let mut count = 0;

    for (dx, dy) in offsets {
        if count == N {
            break;
        }
        let nx = pos.x as i32 + dx;
        let ny = pos.y as i32 + dy;
        if nx < 0 || ny < 0 || nx >= width || ny >= height {
            continue;
        }
        let (nx, ny) = (nx as usize, ny as usize);
        if defect_mask.is_some_and(|m| m.get_at(Vec2us::new(nx, ny))) {
            continue;
        }
        neighbors[count] = *pixels.get(nx, ny);
        count += 1;
    }

    if count == 0 {
        return *pixels.get(pos.x, pos.y);
    }

    median_f32_mut(&mut neighbors[..count])
}

/// Same-color CFA neighbour median strategy, built once per master so the per-pixel scan stays
/// cheap. The X-Trans variant precomputes its neighbour geometry up front (see [`XTransOffsets`]);
/// Mono and Bayer carry no state because their offsets are fixed.
#[derive(Debug)]
pub(super) enum SameColorMedian {
    /// Mono: 8-connected neighbours.
    Mono,
    /// Bayer: same-color neighbours at stride 2 (true for every 2×2 Bayer phase).
    Bayer,
    /// X-Trans: same-color offsets precomputed per 6×6 phase. Boxed — the 36-phase table dwarfs the
    /// other (zero-size) variants, and the strategy is built once per master, not per pixel.
    XTrans(Box<XTransOffsets>),
}

impl SameColorMedian {
    /// `None` (no CFA metadata) is treated as Mono, matching [`cfa_color_at`](super::cfa_color_at).
    pub(super) fn new(cfa: Option<&CfaType>) -> Self {
        match cfa {
            None | Some(CfaType::Mono) => Self::Mono,
            Some(CfaType::Bayer(_)) => Self::Bayer,
            Some(CfaType::XTrans(pattern)) => Self::XTrans(Box::new(XTransOffsets::new(pattern))),
        }
    }

    /// Median of `(x, y)`'s same-color neighbours, skipping any flagged in `defect_mask` so a defect
    /// is never repaired from another defect.
    pub(super) fn at(
        &self,
        pixels: &Buffer2<f32>,
        pos: Vec2us,
        defect_mask: Option<&BitBuffer2>,
    ) -> f32 {
        match self {
            Self::Mono => median_of_neighbors::<8>(pixels, pos, MONO_OFFSETS, defect_mask),
            Self::Bayer => median_of_neighbors::<8>(pixels, pos, BAYER_OFFSETS, defect_mask),
            Self::XTrans(offsets) => offsets.median(pixels, pos, defect_mask),
        }
    }
}

/// Search radius (in pixels) for X-Trans same-color neighbours — one full pattern period.
pub(super) const XTRANS_RADIUS: i32 = 6;

/// Same-color neighbours used for the X-Trans median: ≈4 per cardinal/diagonal direction in the
/// 6×6 pattern — enough for a robust median without directional bias.
pub(super) const XTRANS_NEIGHBORS: usize = 24;

/// Precomputed X-Trans same-color neighbour offsets, indexed by the pixel's 6×6 phase.
///
/// The X-Trans pattern is periodic with period 6, so for a given phase `(x % 6, y % 6)` the set of
/// same-color neighbours within the search window — and their Manhattan distances — is fixed. The
/// old path recomputed this on every pixel: a 13×13 `color_at` sweep plus a per-pixel distance
/// sort, which dominated the cold-pixel scan of a full X-Trans master. Precomputing it once turns
/// the per-pixel work into a bounded gather + median over the nearest valid neighbours.
#[derive(Debug)]
pub(super) struct XTransOffsets {
    /// `per_phase[(y % 6) * 6 + (x % 6)]` = same-color `(dx, dy)` offsets, nearest-first by
    /// Manhattan distance (ties broken by scan order: `dy` then `dx`).
    per_phase: [Vec<(i32, i32)>; 36],
}

impl XTransOffsets {
    pub(super) fn new(pattern: &[[u8; 6]; 6]) -> Self {
        let per_phase = std::array::from_fn(|phase| {
            let px = (phase % 6) as i32;
            let py = (phase / 6) as i32;
            let my_color = pattern[py as usize][px as usize];

            let mut candidates: Vec<(i32, (i32, i32))> = Vec::new();
            for dy in -XTRANS_RADIUS..=XTRANS_RADIUS {
                for dx in -XTRANS_RADIUS..=XTRANS_RADIUS {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    // The pattern is globally periodic, so a neighbour's color depends only on its
                    // phase; `rem_euclid` keeps that correct for negative offsets.
                    let cy = (py + dy).rem_euclid(6) as usize;
                    let cx = (px + dx).rem_euclid(6) as usize;
                    if pattern[cy][cx] == my_color {
                        candidates.push((dx.abs() + dy.abs(), (dx, dy)));
                    }
                }
            }
            // Stable sort: equal-distance neighbours keep scan order.
            candidates.sort_by_key(|&(dist, _)| dist);
            candidates.into_iter().map(|(_, off)| off).collect()
        });
        Self { per_phase }
    }

    /// Median of the nearest valid same-color neighbours of `(x, y)`. Walks the precomputed
    /// nearest-first offsets, skipping out-of-bounds and `defect_mask`ed positions, and stops once
    /// [`XTRANS_NEIGHBORS`] valid samples are gathered — equivalent to "closest-N valid neighbours".
    pub(super) fn median(
        &self,
        pixels: &Buffer2<f32>,
        pos: Vec2us,
        defect_mask: Option<&BitBuffer2>,
    ) -> f32 {
        let phase = &self.per_phase[(pos.y % 6) * 6 + (pos.x % 6)];
        median_of_neighbors::<XTRANS_NEIGHBORS>(pixels, pos, phase.iter().copied(), defect_mask)
    }
}
