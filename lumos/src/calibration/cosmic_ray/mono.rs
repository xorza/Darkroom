//! Textbook L.A.Cosmic on one dense plane.
//!
//! Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)`
//! → fine structure `F` → flag → grow → in-paint → iterate. Also serves each deinterleaved Bayer
//! phase, whose dense neighbours are same-color in the mosaic.

use std::ops::Range;
use std::{array, fmt};

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::kept_out::KeptOut;
use crate::calibration::cosmic_ray::masks::{CrMasks, DetectionPlanes};
use crate::calibration::cosmic_ray::noise_model::{NoiseModel, PixelBg};
use crate::math::size2us::Size2us;
use crate::math::statistics;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::{ScratchPool, Shapeless};

/// Frame-sized `f32` planes the mono detector holds, however many iterations it runs.
pub(crate) const MONO_SCRATCH_PLANES: usize = 5;

/// The mono detector's frame-sized `f32` working set, allocated on the first iteration and reused
/// by every one after it.
///
/// Each buffer is written in full before it is read, so what the previous iteration — or the
/// previous Bayer plane — left in it never matters; only the capacity does. The four Bayer phase
/// planes differ by at most a row and a column, so one scratch serves all four: the `reshape` in
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
    significance: Buffer2<f32>,
    /// The object fine structure `F = median₃ − median₇(median₃)`.
    fine: Buffer2<f32>,
    /// Per-pixel noise `N`.
    noise: Buffer2<f32>,
    /// The window medians, one at a time: `median₇(median₃(I))`, then `median₅(I)`, then
    /// `median₅(S)`. Each is consumed by the step immediately after it, so the three never overlap.
    median: Buffer2<f32>,
    /// A stage's input, one at a time: `median₃(I)`, then `S = L⁺/(2N)`; then the read-only
    /// snapshot [`PixelRepair::apply`] gathers from.
    staged: Buffer2<f32>,
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
    /// The masks of the detection at hand, cleared for each plane.
    masks: CrMasks,
    /// The window medians' per-job scratch, reused by every pass.
    windows: ScratchPool<WindowScratch>,
    /// One flag a row: whether the last repair changed it, every row before the first, which
    /// computes them all. A row no repair reached recomputes to the same bits, so only the rows a
    /// stage's reach takes from a changed one are worked again.
    changed: Vec<bool>,
    /// The bands of rows the stage at hand works, as [`rows_within`] gives them.
    bands: Vec<Range<usize>>,
    /// Which rows a pass after the first works again.
    #[cfg(test)]
    recompute: Recompute,
}

/// Which rows a pass after the first works again.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recompute {
    /// The rows a stage's reach takes from one the last repair changed: what a detection runs.
    ChangedRows,
    /// Every row: the oracle the update of changed rows is held to.
    EveryRow,
}

impl<'a> MonoDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig, noise: NoiseModel) -> Self {
        Self {
            config,
            noise,
            scratch: MonoScratch::default(),
            masks: CrMasks::empty(),
            windows: ScratchPool::default(),
            changed: Vec::new(),
            bands: Vec::new(),
            #[cfg(test)]
            recompute: Recompute::ChangedRows,
        }
    }

    /// Whether a detection scans a `size` plane: one under 3 pixels on either side is left as it
    /// is.
    pub(super) const fn scans(size: Size2us) -> bool {
        size.width >= 3 && size.height >= 3
    }

    /// The bytes a detection on a `size` plane allocates beside it: the scratch planes and the
    /// masks, or on a plane too small to scan the empty mask of what it found alone.
    pub(super) const fn heap_bytes(size: Size2us) -> usize {
        if !Self::scans(size) {
            return BitBuffer2::heap_bytes(size);
        }
        MONO_SCRATCH_PLANES * size.pixel_count() * size_of::<f32>() + CrMasks::heap_bytes(size)
    }

    /// Detect and in-paint cosmic rays in one dense plane, in place; the mask of every pixel it
    /// in-painted, the plane's size, which the detector holds until its next detection.
    ///
    /// Subsample ×2 → clipped Laplacian → resample → significance `S = L⁺/(2N)` →
    /// `S' = S − median₅(S)` → fine structure `F` → flag → grow → in-paint → iterate.
    ///
    /// `local` gives each pixel's background sky and σ and its flat gain, by flat index into the
    /// plane.
    pub(super) fn reject(
        &mut self,
        data: &mut Buffer2<f32>,
        local: &(dyn Fn(usize) -> PixelBg + Sync),
        kept_out: Option<&KeptOut>,
    ) -> &BitBuffer2 {
        let size = Size2us::from(&*data);
        let Self {
            config,
            noise: noise_model,
            scratch,
            masks,
            windows,
            changed,
            bands,
            #[cfg(test)]
            recompute,
        } = self;
        if !Self::scans(size) {
            masks.accumulated.clear_to(size);
            return &masks.accumulated;
        }
        masks.clear_to(size);
        let noise_of = PixelNoise {
            model: *noise_model,
            local,
        };
        let width = size.width;
        changed.clear();
        changed.resize(size.height, true);

        for _ in 0..config.max_iterations {
            let MonoScratch {
                significance,
                fine,
                noise,
                median,
                staged,
            } = &mut *scratch;
            for plane in [
                &mut *significance,
                &mut *fine,
                &mut *noise,
                &mut *median,
                &mut *staged,
            ] {
                plane.reshape(size.width, size.height, 0.0);
            }
            let pix = &*data;

            // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources,
            // ~0 at a CR (median₃ already erased the spike). Clamped non-negative and no further:
            // the only consumer divides by the noise and floors the result at
            // `FINE_STRUCTURE_SIGMA_FLOOR`, so the guard against a vanishing F belongs there, in σ
            // units, where it holds whatever scale the samples are in. An absolute floor here would
            // instead read as enormous fine structure on a frame whose noise is below it, and
            // suppress every detection.
            rows_within(changed, MEDIAN3_REACH, bands);
            for band in bands.iter().cloned() {
                WindowMedian { radius: 1 }.rows(pix, OutputBand::of(staged, band), windows);
            }
            rows_within(changed, FINE_REACH, bands);
            for band in bands.iter().cloned() {
                WindowMedian { radius: 3 }.rows(
                    staged,
                    OutputBand::of(median, band.clone()),
                    windows,
                );
                let span = band.start * width..band.end * width;
                for ((f, &m3), &m7) in fine[span.clone()]
                    .iter_mut()
                    .zip(&staged[span.clone()])
                    .zip(&median[span])
                {
                    *f = (m3 - m7).max(0.0);
                }
            }

            rows_within(changed, NOISE_REACH, bands);
            for band in bands.iter().cloned() {
                WindowMedian { radius: 2 }.rows(pix, OutputBand::of(median, band.clone()), windows);
                noise_of.rows(median, OutputBand::of(noise, band));
            }

            // Significance S = L⁺/(2N), L⁺ the clipped Laplacian of the ×2-subsampled frame, then
            // S' = S − median₅(S) to strip smooth large-scale structure.
            rows_within(changed, S_REACH, bands);
            for band in bands.iter().cloned() {
                laplacian_plus_rows(pix, OutputBand::of(staged, band.clone()));
                let span = band.start * width..band.end * width;
                for (l, &nz) in staged[span.clone()].iter_mut().zip(&noise[span]) {
                    *l /= 2.0 * nz;
                }
            }
            rows_within(changed, SIGNIFICANCE_REACH, bands);
            for band in bands.iter().cloned() {
                WindowMedian { radius: 2 }.rows(
                    staged,
                    OutputBand::of(median, band.clone()),
                    windows,
                );
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
            let planes = DetectionPlanes {
                significance,
                fine,
                noise,
            };
            if masks.detect_and_grow(planes, config, never_flagged) == 0 {
                break;
            }
            PixelRepair {
                flagged: &masks.accumulated,
                unread: kept_out.map(|kept_out| &kept_out.unread),
                local,
            }
            .apply(data, staged, windows);
            changed.clear();
            changed.par_extend(data.par_chunks(width).zip(staged.par_chunks(width)).map(
                |(now, before)| {
                    now.iter()
                        .zip(before)
                        .any(|(now, before)| now.to_bits() != before.to_bits())
                },
            ));
            #[cfg(test)]
            if *recompute == Recompute::EveryRow {
                changed.fill(true);
            }
        }
        &masks.accumulated
    }
}

/// The rows of a plane a stage writes: the band it works, from row `first` of a plane `width`
/// wide.
#[derive(Debug)]
struct OutputBand<'a> {
    first: usize,
    width: usize,
    pixels: &'a mut [f32],
}

impl<'a> OutputBand<'a> {
    /// `rows` of `plane`.
    fn of(plane: &'a mut Buffer2<f32>, rows: Range<usize>) -> Self {
        let width = plane.width();
        Self {
            first: rows.start,
            width,
            pixels: &mut plane[rows.start * width..rows.end * width],
        }
    }

    /// Where the band lies in its plane's pixels.
    const fn span(&self) -> Range<usize> {
        let start = self.first * self.width;
        start..start + self.pixels.len()
    }
}

/// Clipped Laplacian of the ×2-subsampled `data`, block-averaged back down to its size, over the
/// rows of `out`.
///
/// Convolves the ×2 image with `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clips negatives to 0 (keeping only
/// sharp positive peaks), then averages each 2×2 block. Edge-clamped on the ×2 grid.
///
/// The ×2 image is never materialized. Subsampling here is a block replication — every `sub`
/// sample is `data[y2 / 2][x2 / 2]` — so it is read through that index instead of being written to
/// a buffer four times pixel count, and the clipped Laplacian is averaged as it is produced rather
/// than stored in a second buffer of the same size. That is 8n floats of allocation and traffic
/// removed from every iteration; what remains is the result, written into the caller's buffer.
fn laplacian_plus_rows(data: &Buffer2<f32>, out: OutputBand<'_>) {
    let size = Size2us::from(data);
    let (w2, h2) = (size.width * 2, size.height * 2);
    // The ×2 sample at (x2, y2), which is the native pixel under it.
    let at = |y2: usize, x2: usize| data[(y2 / 2) * size.width + (x2 / 2)];

    let OutputBand { first, pixels, .. } = out;
    pixels
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

/// The median over a `(2r+1)²` window, `r` its radius, replicating the border pixel for
/// out-of-bounds coordinates so every output sees a full window.
///
/// Deliberately not star detection's own `median_filter::apply_3x3`, even
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
#[derive(Debug, Clone, Copy)]
struct WindowMedian {
    radius: usize,
}

impl WindowMedian {
    /// The window medians of `data` over the rows of `out`. Row-parallel.
    fn rows(self, data: &Buffer2<f32>, out: OutputBand<'_>, windows: &ScratchPool<WindowScratch>) {
        let (size, r) = (Size2us::from(data), self.radius);
        let side = 2 * r + 1;
        let OutputBand { first, pixels, .. } = out;
        pixels.par_chunks_mut(size.width).enumerate().for_each_init(
            || windows.acquire(),
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
                                scratch.values.push(data[size.idx_of(Vec2us::new(xx, yy))]);
                            }
                        }
                        row[x] = statistics::median_mut(&mut scratch.values);
                        x += 1;
                    }
                }
            },
        );
    }
}

/// The pixels one forgetful selection runs over at once: an AVX2 register of `i32`s.
const LANES: usize = 8;

/// One worker's windows: a pixel's values, and eight pixels' keys, value by value.
#[derive(Debug, Default)]
pub(super) struct WindowScratch {
    values: Vec<f32>,
    lanes: Vec<[i32; LANES]>,
}

impl Shapeless for WindowScratch {}

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
        for idx in first + 1..end {
            exchange(lanes, first, idx);
        }
        for idx in first + 1..end - 1 {
            exchange(lanes, idx, end - 1);
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

/// Each pixel's noise `N`: the noise model at the pixel's local background.
#[derive(Clone, Copy)]
struct PixelNoise<'a> {
    model: NoiseModel,
    /// Each pixel's background sky and σ and its flat gain, by flat index into the plane.
    local: &'a (dyn Fn(usize) -> PixelBg + Sync),
}

/// Hand-written: the `local` closure has no `Debug`.
impl fmt::Debug for PixelNoise<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PixelNoise")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl PixelNoise<'_> {
    /// Each pixel's noise over the rows of `out`, from `m5`, the plane's median-filtered (CR-free)
    /// signal estimate.
    fn rows(self, m5: &Buffer2<f32>, out: OutputBand<'_>) {
        let span = out.span();
        let first = span.start;
        let OutputBand { pixels, .. } = out;
        pixels
            .par_iter_mut()
            .zip(&m5[span])
            .enumerate()
            .for_each(|(idx, (out, &signal))| {
                *out = self.model.noise(signal, (self.local)(first + idx));
            });
    }
}

/// Into `bands`, the rows within `reach` of a row `changed` marks, as ascending, disjoint bands:
/// one band of every row when every row is marked.
fn rows_within(changed: &[bool], reach: usize, bands: &mut Vec<Range<usize>>) {
    let height = changed.len();
    bands.clear();
    for (row, _) in changed.iter().enumerate().filter(|&(_, &changed)| changed) {
        let band = row.saturating_sub(reach)..(row + reach + 1).min(height);
        match bands.last_mut() {
            Some(last) if band.start <= last.end => last.end = last.end.max(band.end),
            _ => bands.push(band),
        }
    }
}

/// The in-painting of the pixels `flagged` holds: each takes the median of its 5×5 neighbors
/// (edge-clamped) that neither `flagged` nor `unread` holds, as L.A.Cosmic does. Where none of
/// them is measured — a hit of 5×5 or more, or one ringed by saturated or star-core pixels — the
/// window widens a ring at a time up to [`REPAIR_REACH`], and past that the pixel takes its local
/// sky. Every flagged pixel is replaced, so none keeps the hit's value under a `REPAIRED` flag.
///
/// The frame copy is not what makes replacements independent of each other — writes land only on
/// masked pixels and reads only on unmasked ones, so no replacement can consult a replaced
/// neighbour whatever order the rows run in. It is here because `pixels_mut()` is held across the
/// reads, and it pays for itself besides: reading a separate, read-only array keeps one row's
/// writes off the cache lines the rows above and below are reading. Aliasing the two through a raw
/// pointer would be sound — the sets are disjoint — but measures *slower* on every run of
/// `bench_cosmic_ray_reject_mono`: the false sharing costs more than the copy. The copy lands in
/// the caller's `snapshot` buffer, so it is a memcpy per iteration and not an allocation.
#[derive(Clone, Copy)]
pub(super) struct PixelRepair<'a> {
    pub(super) flagged: &'a BitBuffer2,
    pub(super) unread: Option<&'a BitBuffer2>,
    /// Each pixel's local background, whose sky stands in where no neighbour within
    /// [`REPAIR_REACH`] is measured.
    pub(super) local: &'a (dyn Fn(usize) -> PixelBg + Sync),
}

/// The widest ring a repair reaches for a measured neighbour, a 15×15 window: past twice the
/// 5×5 window's reach, a pixel's neighbours describe the star or gradient around the hit less well
/// than the local sky does.
const REPAIR_REACH: usize = 7;

impl fmt::Debug for PixelRepair<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Repair")
            .field("flagged", &self.flagged)
            .field("unread", &self.unread)
            .finish_non_exhaustive()
    }
}

impl PixelRepair<'_> {
    /// In-paint `data` in place, reading its copy in `snapshot`.
    pub(super) fn apply(
        self,
        data: &mut Buffer2<f32>,
        snapshot: &mut Buffer2<f32>,
        windows: &ScratchPool<WindowScratch>,
    ) {
        let Self {
            flagged: mask,
            unread,
            local,
        } = self;
        let size = Size2us::from(&*data);
        snapshot.reshape(size.width, size.height, 0.0);
        snapshot.copy_from_slice(data);
        let src: &[f32] = snapshot;
        data.par_chunks_mut(size.width).enumerate().for_each_init(
            || windows.acquire(),
            |scratch, (y, row)| {
                let buf = &mut scratch.values;
                for (x, o) in row.iter_mut().enumerate() {
                    if !mask.get_at(Vec2us::new(x, y)) {
                        continue;
                    }
                    buf.clear();
                    let measured =
                        |j: usize| !mask.get(j) && unread.is_none_or(|unread| !unread.get(j));
                    // The window about `(x, y)`, replicated into the frame at both edges: the 5×5
                    // first, then a ring at a time while none of it is measured.
                    for reach in 0..=REPAIR_REACH {
                        if reach > 2 && !buf.is_empty() {
                            break;
                        }
                        for (xx, yy) in ring(Vec2us::new(x, y), reach, size) {
                            let j = size.idx_of(Vec2us::new(xx, yy));
                            if measured(j) {
                                buf.push(src[j]);
                            }
                        }
                    }
                    *o = if buf.is_empty() {
                        local(size.idx_of(Vec2us::new(x, y))).local.sky
                    } else {
                        statistics::median_mut(buf)
                    };
                }
            },
        );
    }
}

/// The pixels at Chebyshev distance `reach` from `centre`, each replicated into the `size` frame
/// at its edges: the centre itself for 0.
fn ring(centre: Vec2us, reach: usize, size: Size2us) -> impl Iterator<Item = (usize, usize)> {
    let side = 2 * reach + 1;
    let clamp =
        move |at: usize, limit: usize| (at + reach).saturating_sub(2 * reach).min(limit - 1);
    (0..side).flat_map(move |dy| {
        let on_edge = dy == 0 || dy == side - 1;
        let step = if on_edge { 1 } else { (side - 1).max(1) };
        (0..side).step_by(step).map(move |dx| {
            (
                clamp(centre.x + dx, size.width),
                clamp(centre.y + dy, size.height),
            )
        })
    })
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bg_mesh::color_mesh::LocalBg;
    use crate::bit_buffer2::BitBuffer2;
    use crate::calibration::cosmic_ray::cosmic_ray_config::{CosmicRayConfig, NoiseEstimation};
    use crate::calibration::cosmic_ray::mono::{
        MonoDetector, MonoScratch, OutputBand, Recompute, WindowMedian,
    };
    use crate::calibration::cosmic_ray::noise_model::{NoiseModel, PixelBg};
    use crate::image::image_metadata::ImageMetadata;
    use crate::scratch_pool::ScratchPool;

    /// The `(2r+1)²` window median of `data`, into a fresh plane.
    pub(crate) fn median_window(data: &Buffer2<f32>, r: usize) -> Buffer2<f32> {
        let mut out = Buffer2::new_default(data.width(), data.height());
        let rows = 0..data.height();
        WindowMedian { radius: r }.rows(
            data,
            OutputBand::of(&mut out, rows),
            &ScratchPool::default(),
        );
        out
    }

    /// A mono detector on `config`, against a flat background of sky 0.1 and σ 0.01.
    fn detector(config: &CosmicRayConfig) -> MonoDetector<'_> {
        let noise = NoiseModel::resolve(&NoiseEstimation::Measured, &ImageMetadata::default())
            .expect("the measured model needs nothing from the frame");
        MonoDetector::new(config, noise)
    }

    const FLAT: PixelBg = PixelBg {
        local: LocalBg {
            sky: 0.1,
            noise: 0.01,
        },
        flat_gain: 1.0,
    };

    /// Clean `data` as the mono detector does, each pass after the first working the rows
    /// `recompute` names, against [`detector`]'s flat background; the pixels it found.
    pub(crate) fn reject_recomputing(
        data: &mut Buffer2<f32>,
        config: &CosmicRayConfig,
        recompute: Recompute,
    ) -> BitBuffer2 {
        let mut detector = detector(config);
        detector.recompute = recompute;
        detector.reject(data, &|_| FLAT, None).clone()
    }

    /// Total capacity, in floats, of the mono detector's working set after a run on `data` — what
    /// `cosmic_ray_scratch` weighs against
    /// [`MONO_SCRATCH_PLANES`](crate::calibration::cosmic_ray::mono::MONO_SCRATCH_PLANES).
    ///
    /// Destructured rather than summed through a helper, so a plane added to or dropped from
    /// [`MonoScratch`] fails to compile here instead of silently drifting from the constant.
    pub(crate) fn mono_scratch_floats(data: &mut Buffer2<f32>, config: &CosmicRayConfig) -> usize {
        let mut detector = detector(config);
        detector.reject(data, &|_| FLAT, None);
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
