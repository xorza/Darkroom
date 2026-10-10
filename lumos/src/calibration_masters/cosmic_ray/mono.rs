//! Textbook L.A.Cosmic on one dense plane.
//!
//! Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)`
//! → fine structure `F` → flag → grow → in-paint → iterate. Also serves each deinterleaved Bayer
//! phase, whose dense neighbours are same-colour in the mosaic.

use std::array;
use std::iter;
use std::ops::Range;

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;

use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::kept_out::KeptOut;
use crate::calibration_masters::cosmic_ray::masks::CrMasks;
use crate::calibration_masters::cosmic_ray::noise_model::{NoiseModel, PixelBackground};

/// Frame-sized `f32` planes the mono detector holds, however many iterations it runs.
pub(crate) const MONO_SCRATCH_PLANES: usize = 5;

/// The mono detector's frame-sized `f32` working set, allocated on the first iteration and reused
/// by every one after it.
///
/// Each buffer is written in full before it is read, so what the previous iteration — or the
/// previous Bayer plane — left in it never matters; only the capacity does. The four Bayer phase
/// planes differ by at most a row and a column, so one scratch serves all four: the `resize` in
/// each producer keeps the largest allocation.
///
/// Five planes, not the eight the stages name. `significance`, `fine` and `noise` hold each pass's
/// result, which the next pass updates only where a repair can have moved it; `staged` and `median`
/// hold a stage's input and its window median, one stage at a time. On a 6144² mono frame that is
/// 720 MB of working set instead of 1.1 GB — and, the point of the struct, no allocation at all
/// after the first iteration.
#[derive(Debug, Default)]
struct MonoScratch {
    /// The significance with its smooth part taken out, `S' = S − median₅(S)`.
    significance: Vec<f32>,
    /// The object fine structure `F = median₃ − median₇(median₃)`.
    fine: Vec<f32>,
    /// Per-pixel noise `N`.
    noise: Vec<f32>,
    /// The window medians, one at a time: `median₇(median₃(I))`, then `median₅(I)`, then
    /// `median₅(S)`. Each is consumed by the step immediately after it, so the three never overlap.
    median: Vec<f32>,
    /// A stage's input, one at a time: `median₃(I)`, then `S = L⁺/(2N)`; then the read-only
    /// snapshot [`replace_flagged`] gathers from.
    staged: Vec<f32>,
}

/// The rows a stage's output can move by, beyond the rows a repair changed: `median₃` reaches 1,
/// `median₇` 3 more, so `F` moves within 4; `median₅` reaches 2, so `N` moves within 2, and `S`,
/// whose Laplacian reaches 1, within 2 too; `median₅(S)` 2 more, so `S'` moves within 4.
const FINE_REACH: usize = 4;
const NOISE_REACH: usize = 2;
const SIGNIFICANCE_REACH: usize = 4;
/// Rows of `median₃(I)` that `median₇` reads to give `F` within [`FINE_REACH`].
const MEDIAN3_REACH: usize = FINE_REACH + 3;
/// Rows of `S` that `median₅(S)` reads to give `S'` within [`SIGNIFICANCE_REACH`].
const S_REACH: usize = SIGNIFICANCE_REACH + 2;

/// The mono cosmic-ray detector: its configuration, and the working set it reuses.
///
/// Owning both is what lets one detector clean every Bayer phase plane with a single allocation —
/// `(0, 0)` is the largest phase and runs first, so no later one grows the buffers — without the
/// caller having to thread a scratch through by hand and know that rule.
#[derive(Debug)]
pub(super) struct MonoDetector<'a> {
    config: &'a CosmicRayConfig,
    noise: NoiseModel,
    scratch: MonoScratch,
    /// Recompute every row on every pass: the oracle the update of changed rows is held to.
    #[cfg(test)]
    full_recompute: bool,
}

impl<'a> MonoDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig, noise: NoiseModel) -> Self {
        Self {
            config,
            noise,
            scratch: MonoScratch::default(),
            #[cfg(test)]
            full_recompute: false,
        }
    }

    /// The bytes a detection on a `size` plane allocates beside it: the scratch planes and the
    /// masks, or nothing on a plane too small to scan.
    pub(super) fn heap_bytes(size: Size2us) -> usize {
        if size.width < 3 || size.height < 3 {
            return 0;
        }
        MONO_SCRATCH_PLANES * size.pixel_count() * size_of::<f32>() + CrMasks::heap_bytes(size)
    }

    /// Detect and in-paint cosmic rays in one dense plane, in place, marking every in-painted pixel
    /// in `found` (the plane's size) and returning how many there are.
    ///
    /// Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` →
    /// `S' = S − median₅(S)` → fine structure `F` → flag → grow → in-paint → iterate.
    ///
    /// `local` gives each pixel's background sky and σ and its flat gain, by flat index into the
    /// plane.
    pub(super) fn reject(
        &mut self,
        data: &mut [f32],
        size: Size2us,
        local: &(dyn Fn(usize) -> PixelBackground + Sync),
        kept_out: Option<&KeptOut>,
        found: &mut BitBuffer2,
    ) -> usize {
        debug_assert_eq!(data.len(), size.pixel_count());
        if size.width < 3 || size.height < 3 {
            return 0;
        }
        let mut masks = CrMasks::new(size);
        let width = size.width;
        // The rows the last repair changed; `None` before the first pass, which computes them all.
        // A row no repair reached recomputes to the same bits, so only the rows a stage's reach
        // takes from a changed one are worked again.
        let mut changed: Option<Vec<bool>> = None;

        for _ in 0..self.config.niter {
            let MonoScratch {
                significance,
                fine,
                noise,
                median,
                staged,
            } = &mut self.scratch;
            for plane in [
                &mut *significance,
                &mut *fine,
                &mut *noise,
                &mut *median,
                &mut *staged,
            ] {
                plane.resize(size.pixel_count(), 0.0);
            }
            let pix = &*data;
            let rows = |reach: usize| match &changed {
                Some(changed) => rows_within(changed, reach),
                None => iter::once(0..size.height).collect(),
            };

            // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources,
            // ~0 at a CR (median₃ already erased the spike). Clamped non-negative and no further:
            // the only consumer divides by the noise and floors the result at
            // `FINE_STRUCTURE_SIGMA_FLOOR`, so the guard against a vanishing F belongs there, in σ
            // units, where it holds whatever scale the samples are in. An absolute floor here would
            // instead read as enormous fine structure on a frame whose noise is below it, and
            // suppress every detection.
            for band in rows(MEDIAN3_REACH) {
                median_window_rows(pix, size, 1, band, staged);
            }
            for band in rows(FINE_REACH) {
                median_window_rows(staged, size, 3, band.clone(), median);
                let span = band.start * width..band.end * width;
                for ((f, &m3), &m7) in fine[span.clone()]
                    .iter_mut()
                    .zip(&staged[span.clone()])
                    .zip(&median[span])
                {
                    *f = (m3 - m7).max(0.0);
                }
            }

            for band in rows(NOISE_REACH) {
                median_window_rows(pix, size, 2, band.clone(), median);
                noise_map_rows(median, local, self.noise, band, width, noise);
            }

            // Significance S = L⁺/(2N), L⁺ the clipped Laplacian of the ×2-subsampled frame, then
            // S' = S − median₅(S) to strip smooth large-scale structure.
            for band in rows(S_REACH) {
                laplacian_plus_rows(pix, size, band.clone(), staged);
                let span = band.start * width..band.end * width;
                for (l, &nz) in staged[span.clone()].iter_mut().zip(&noise[span]) {
                    *l /= 2.0 * nz;
                }
            }
            for band in rows(SIGNIFICANCE_REACH) {
                median_window_rows(staged, size, 2, band.clone(), median);
                let span = band.start * width..band.end * width;
                for ((v, &l), &m) in significance[span.clone()]
                    .iter_mut()
                    .zip(&staged[span.clone()])
                    .zip(&median[span])
                {
                    *v = l - m;
                }
            }

            let never_flagged = kept_out.map(|kept_out| &kept_out.never_flagged);
            if masks.detect_and_grow(significance, fine, noise, self.config, never_flagged) == 0 {
                break;
            }
            let unread = kept_out.map(|kept_out| &kept_out.unread);
            replace_flagged(data, size, &masks.accumulated, unread, staged);
            changed = Some(
                data.par_chunks(width)
                    .zip(staged.par_chunks(width))
                    .map(|(now, before)| {
                        now.iter()
                            .zip(before)
                            .any(|(now, before)| now.to_bits() != before.to_bits())
                    })
                    .collect(),
            );
            #[cfg(test)]
            if self.full_recompute {
                changed = None;
            }
        }

        found.copy_from(&masks.accumulated);
        masks.accumulated.count_ones()
    }
}

/// Clipped Laplacian of the ×2-subsampled frame, block-averaged back down to `size`, over `rows` of
/// `out`.
///
/// Convolves the ×2 image with `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clips negatives to 0 (keeping only
/// sharp positive peaks), then averages each 2×2 block. Edge-clamped on the ×2 grid.
///
/// The ×2 image is never materialized. Subsampling here is a block replication — every `sub`
/// sample is `data[y2 / 2][x2 / 2]` — so it is read through that index instead of being written to
/// a buffer four times pixel count, and the clipped Laplacian is averaged as it is produced rather
/// than stored in a second buffer of the same size. That is 8n floats of allocation and traffic
/// removed from every iteration; what remains is the result, written into the caller's buffer.
fn laplacian_plus_rows(data: &[f32], size: Size2us, rows: Range<usize>, out: &mut [f32]) {
    let (w2, h2) = (size.width * 2, size.height * 2);
    // The ×2 sample at (x2, y2), which is the native pixel under it.
    let at = |y2: usize, x2: usize| data[(y2 / 2) * size.width + (x2 / 2)];

    let first = rows.start;
    out[rows.start * size.width..rows.end * size.width]
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(row_in_band, row)| {
            let y = first + row_in_band;
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
/// every output sees a full window, over `rows` of `out`. Row-parallel.
///
/// Deliberately not star detection's own `median_filter_3x3`, even
/// though `r == 1` describes the same 3×3 median: that one *shrinks* its window at the border to
/// the 4 or 6 in-bounds samples where this one replicates. L.A.Cosmic differences two of these
/// windows against each other — the fine structure is `median₃ − median₇(median₃)` — so every
/// radius here has to share one border convention. Feeding a shrunk `median₃` into that
/// difference while `median₇` stayed replicated would corrupt the border in a way neither
/// convention does alone. Replication is also the usual choice for astronomical median filtering.
///
/// Eight pixels whose windows lie inside the row take their medians together by forgetful
/// selection ([`median_of_lanes`]) on `total_cmp`'s integer keys, so each is the value
/// `median_mut` picks, NaN and signed zero included; a pixel whose window crosses a side edge
/// gathers its replicated window and takes `median_mut`.
fn median_window_rows(data: &[f32], size: Size2us, r: usize, rows: Range<usize>, out: &mut [f32]) {
    let side = 2 * r + 1;
    let first = rows.start;
    out[rows.start * size.width..rows.end * size.width]
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each_init(
            || WindowScratch {
                values: Vec::with_capacity(side * side),
                lanes: Vec::with_capacity(side * side),
            },
            |scratch, (row_in_band, row)| {
                let y = first + row_in_band;
                let width = size.width;
                let rows = |dy: usize| (y + dy).saturating_sub(r).min(size.height - 1);
                let mut x = 0;
                while x < width {
                    if x >= r && x + LANES + r <= width {
                        scratch.lanes.clear();
                        for dy in 0..side {
                            let start = rows(dy) * width + x - r;
                            for dx in 0..side {
                                let at = &data[start + dx..start + dx + LANES];
                                scratch
                                    .lanes
                                    .push(array::from_fn(|lane| total_key(at[lane])));
                            }
                        }
                        let medians = median_of_lanes(&mut scratch.lanes);
                        for (o, key) in row[x..x + LANES].iter_mut().zip(medians) {
                            *o = from_total_key(key);
                        }
                        x += LANES;
                    } else {
                        scratch.values.clear();
                        for dy in 0..side {
                            let yy = rows(dy);
                            for dx in 0..side {
                                let xx = (x + dx).saturating_sub(r).min(width - 1);
                                scratch
                                    .values
                                    .push(data[size.index_of(Vec2us::new(xx, yy))]);
                            }
                        }
                        row[x] = median_mut(&mut scratch.values);
                        x += 1;
                    }
                }
            },
        );
}

/// The pixels one forgetful selection runs over at once: an AVX2 register of `i32`s.
const LANES: usize = 8;

/// One worker's windows: a pixel's values, and eight pixels' keys, value by value.
#[derive(Debug)]
struct WindowScratch {
    values: Vec<f32>,
    lanes: Vec<[i32; LANES]>,
}

/// `total_cmp`'s key for `value`: an `i32` whose order is the total order of the f32s.
#[inline(always)]
const fn total_key(value: f32) -> i32 {
    let bits = value.to_bits().cast_signed();
    bits ^ ((bits >> 31).cast_unsigned() >> 1).cast_signed()
}

/// The f32 of a [`total_key`]: the same transform, which undoes itself.
#[inline(always)]
const fn from_total_key(key: i32) -> f32 {
    f32::from_bits((key ^ ((key >> 31).cast_unsigned() >> 1).cast_signed()).cast_unsigned())
}

/// Each lane's median of `lanes`, an odd count of them, by forgetful selection (Paeth): keep
/// `k + 2` of the `2k + 1` values, move the least to the front and the greatest to the back and
/// drop both, take the next value, and repeat until three are left, whose middle is the median.
///
/// With `W` the kept values and `U` the unseen ones, `|W| = |U| + 3` throughout. The median of
/// `W ∪ U` has `|U| + 1` values below it, so not all of `W` lies above it, nor all below: `W`'s
/// least is at or below it and `W`'s greatest at or above, and dropping one from each side leaves
/// the median where it was. Taking the next value moves it from `U` to `W`, which changes neither.
/// Each move is a compare-exchange of every lane at once, so the lanes share one branch-free
/// sweep. Overwrites `lanes`.
fn median_of_lanes(lanes: &mut [[i32; LANES]]) -> [i32; LANES] {
    let count = lanes.len();
    debug_assert!(
        count % 2 == 1 && count >= 3,
        "an odd window of three or more"
    );
    let exchange = |lanes: &mut [[i32; LANES]], low: usize, high: usize| {
        let (a, b) = (lanes[low], lanes[high]);
        lanes[low] = array::from_fn(|lane| a[lane].min(b[lane]));
        lanes[high] = array::from_fn(|lane| a[lane].max(b[lane]));
    };
    let (mut first, mut end, mut next) = (0, count / 2 + 2, count / 2 + 2);
    loop {
        for index in first + 1..end {
            exchange(lanes, first, index);
        }
        for index in first + 1..end - 1 {
            exchange(lanes, index, end - 1);
        }
        if end - first == 3 {
            debug_assert_eq!(next, count);
            return lanes[first + 1];
        }
        first += 1;
        end -= 1;
        lanes[end] = lanes[next];
        end += 1;
        next += 1;
    }
}

/// Per-pixel noise `N` from the median-filtered (CR-free) signal estimate `m5` and each pixel's
/// local background, over `rows`, `width` wide, of `out`.
fn noise_map_rows(
    m5: &[f32],
    local: &(dyn Fn(usize) -> PixelBackground + Sync),
    noise: NoiseModel,
    rows: Range<usize>,
    width: usize,
    out: &mut [f32],
) {
    let span = rows.start * width..rows.end * width;
    let first = span.start;
    out[span.clone()]
        .par_iter_mut()
        .zip(&m5[span])
        .enumerate()
        .for_each(|(index, (out, &signal))| *out = noise.noise(signal, local(first + index)));
}

/// The rows within `reach` of a row `changed` marks, as ascending, disjoint bands.
fn rows_within(changed: &[bool], reach: usize) -> Vec<Range<usize>> {
    let height = changed.len();
    let mut bands: Vec<Range<usize>> = Vec::new();
    for (row, _) in changed.iter().enumerate().filter(|&(_, &changed)| changed) {
        let band = row.saturating_sub(reach)..(row + reach + 1).min(height);
        match bands.last_mut() {
            Some(last) if band.start <= last.end => last.end = last.end.max(band.end),
            _ => bands.push(band),
        }
    }
    bands
}

/// Replace masked pixels with the median of their 5×5 neighbors (edge-clamped) that neither `mask`
/// nor `unread` holds; fully-masked neighborhoods (huge CRs) are left for the next iteration to
/// shrink.
///
/// The frame copy is not what makes replacements independent of each other — writes land only on
/// masked pixels and reads only on unmasked ones, so no replacement can consult a replaced
/// neighbour whatever order the rows run in. It is here because `pixels_mut()` is held across the
/// reads, and it pays for itself besides: reading a separate, read-only array keeps one row's
/// writes off the cache lines the rows above and below are reading. Aliasing the two through a raw
/// pointer would be sound — the sets are disjoint — but measures *slower* on every run of
/// `bench_cosmic_ray_reject_mono`: the false sharing costs more than the copy. The copy lands in
/// the caller's `snapshot` buffer, so it is a memcpy per iteration and not an allocation.
pub(super) fn replace_flagged(
    data: &mut [f32],
    size: Size2us,
    mask: &BitBuffer2,
    unread: Option<&BitBuffer2>,
    snapshot: &mut Vec<f32>,
) {
    snapshot.clear();
    snapshot.extend_from_slice(data);
    let src: &[f32] = snapshot;
    data.par_chunks_mut(size.width).enumerate().for_each_init(
        || Vec::<f32>::with_capacity(25),
        |buf, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask.get_at(Vec2us::new(x, y)) {
                    continue;
                }
                buf.clear();
                // The 5×5 window about `(x, y)`, replicated into the frame at both edges.
                for dy in 0..=4 {
                    let yy = (y + dy).saturating_sub(2).min(size.height - 1);
                    for dx in 0..=4 {
                        let xx = (x + dx).saturating_sub(2).min(size.width - 1);
                        let j = size.index_of(Vec2us::new(xx, yy));
                        if !mask.get(j) && unread.is_none_or(|unread| !unread.get(j)) {
                            buf.push(src[j]);
                        }
                    }
                }
                if !buf.is_empty() {
                    *o = median_mut(buf);
                }
            }
        },
    );
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::background_mesh::colour_mesh::LocalBackground;
    use crate::bit_buffer2::BitBuffer2;
    use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
    use crate::calibration_masters::cosmic_ray::config::NoiseEstimation;
    use crate::calibration_masters::cosmic_ray::mono::median_window_rows;
    use crate::calibration_masters::cosmic_ray::mono::{MonoDetector, MonoScratch};
    use crate::calibration_masters::cosmic_ray::noise_model::{NoiseModel, PixelBackground};
    use crate::io::image::image_metadata::ImageMetadata;
    use crate::math::size2us::Size2us;

    /// The `(2r+1)²` window median of `data`, into a fresh plane.
    pub(crate) fn median_window(data: &[f32], size: Size2us, r: usize) -> Vec<f32> {
        let mut out = vec![0.0; size.pixel_count()];
        median_window_rows(data, size, r, 0..size.height, &mut out);
        out
    }

    /// Clean `data` as the mono detector does, every row recomputed on every pass when `full`,
    /// against a flat background of sky 0.1 and σ 0.01; the pixels it found.
    pub(crate) fn reject_recomputing(
        data: &mut [f32],
        size: Size2us,
        config: &CosmicRayConfig,
        full: bool,
    ) -> BitBuffer2 {
        let noise = NoiseModel::resolve(&NoiseEstimation::Measured, &ImageMetadata::default())
            .expect("the measured model needs nothing from the frame");
        let local = |_| PixelBackground {
            local: LocalBackground {
                sky: 0.1,
                noise: 0.01,
            },
            flat_gain: 1.0,
        };
        let mut detector = MonoDetector::new(config, noise);
        detector.full_recompute = full;
        let mut found = BitBuffer2::new_default(size);
        detector.reject(data, size, &local, None, &mut found);
        found
    }

    /// Total capacity, in floats, of the mono detector's working set after a run on `data` — what
    /// `mem_budget` weighs against
    /// [`MONO_SCRATCH_PLANES`](crate::calibration_masters::cosmic_ray::mono::MONO_SCRATCH_PLANES).
    ///
    /// Destructured rather than summed through a helper, so a plane added to or dropped from
    /// [`MonoScratch`] fails to compile here instead of silently drifting from the constant.
    pub(crate) fn mono_scratch_floats(
        data: &mut [f32],
        size: Size2us,
        config: &CosmicRayConfig,
    ) -> usize {
        let noise = NoiseModel::resolve(&NoiseEstimation::Measured, &ImageMetadata::default())
            .expect("the measured model needs nothing from the frame");
        let local = |_| PixelBackground {
            local: LocalBackground {
                sky: 0.1,
                noise: 0.01,
            },
            flat_gain: 1.0,
        };
        let mut detector = MonoDetector::new(config, noise);
        detector.reject(data, size, &local, None, &mut BitBuffer2::new_default(size));
        let MonoScratch {
            significance,
            fine,
            noise,
            median,
            staged,
        } = &detector.scratch;
        significance.capacity()
            + fine.capacity()
            + noise.capacity()
            + median.capacity()
            + staged.capacity()
    }
}
