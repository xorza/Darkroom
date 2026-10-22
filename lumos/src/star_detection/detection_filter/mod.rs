//! [`DetectionFilter`]: a detection plane's filters, run a band of rows at a time from its source
//! with no plane between them.
//!
//! A band of output rows reads its matched filter's taps from a ring of the rows above and below
//! it, each row the filter's input — the source row, or its 3×3 median — run through the row pass.
//! A band computes the rows of its halo again rather than share them, so bands run on their own,
//! and what each worker holds is its ring, a few rows wide, not a plane. SEP filters a ring of
//! rows the same way, one band the height of the image.

use std::mem;
use std::ops::Range;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bg_mesh::mesh_surface::{MeshSurface, SurfaceReader};
use crate::math::size2us::Size2us;
use crate::scratch_pool::{ScratchPool, Shapeless};
use crate::star_detection::convolution::filter_kernels::MatchedKernel;
use crate::star_detection::detection_plane::Prefilter;
use crate::star_detection::median_filter::MedianRows;

/// The bands each worker takes of a plane on average: more than one, so a worker that finishes
/// early takes another.
const BANDS_PER_WORKER: usize = 2;

/// The fewest rows a band holds for each row of the halo it reads above and below it: at 4, a
/// band computes at most a quarter more rows than it writes.
const ROWS_PER_HALO_ROW: usize = 4;

/// Where a detection plane's filters read their rows.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FilterSource<'a> {
    /// A plane as it is: the measurement plane, its sky already out.
    Plane(&'a Buffer2<f32>),
    /// A plane less a sky read off its surface a row at a time: the residual a refinement
    /// thresholds, which no plane holds.
    Residual {
        pixels: &'a Buffer2<f32>,
        sky: &'a MeshSurface,
    },
}

impl<'a> FilterSource<'a> {
    const fn pixels(self) -> &'a Buffer2<f32> {
        match self {
            Self::Plane(pixels) | Self::Residual { pixels, .. } => pixels,
        }
    }

    /// A reader of the sky a residual subtracts, for one worker.
    fn sky_reader(self) -> Option<SurfaceReader<'a>> {
        match self {
            Self::Plane(_) => None,
            Self::Residual { sky, .. } => Some(sky.reader()),
        }
    }
}

/// The filters a detection plane passes through: the 3×3 median of a demosaiced frame, then the
/// matched filter when a FWHM is known.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DetectionFilter<'a> {
    pub(crate) prefilter: Prefilter,
    pub(crate) matched: Option<MatchedKernel<'a>>,
}

/// The rows one worker holds while it filters a band, kept from band to band: the ring of rows the
/// matched filter's taps read, the residual rows a median or a filter reads, three slots by row,
/// and one row's median.
#[derive(Debug, Default)]
pub(crate) struct BandScratch {
    ring: Vec<f32>,
    residual: Vec<f32>,
    median: Vec<f32>,
}

impl Shapeless for BandScratch {}

impl BandScratch {
    /// The bytes one worker's scratch holds for frames `width` wide under a matched filter of
    /// `radius` rows at most: its ring, three residual rows and a median row.
    pub(crate) const fn heap_bytes(width: usize, radius: usize) -> usize {
        (2 * radius + 1 + 3 + 1) * width * size_of::<f32>()
    }
}

impl DetectionFilter<'_> {
    /// `source` through the filters into `output`, a plane of its size, a band of rows a task,
    /// each worker's rows from `scratch`.
    pub(crate) fn run(
        self,
        source: FilterSource<'_>,
        output: &mut Buffer2<f32>,
        scratch: &ScratchPool<BandScratch>,
    ) {
        let height = source.pixels().height();
        let radius = self.matched.map_or(0, MatchedKernel::radius);
        let band = height
            .div_ceil(BANDS_PER_WORKER * rayon::current_num_threads())
            .max(ROWS_PER_HALO_ROW * 2 * radius)
            .clamp(1, height);
        self.run_in_bands(source, output, scratch, band);
    }

    /// [`Self::run`] in bands of `band` rows.
    fn run_in_bands(
        self,
        source: FilterSource<'_>,
        output: &mut Buffer2<f32>,
        scratch: &ScratchPool<BandScratch>,
        band: usize,
    ) {
        let pixels = source.pixels();
        let (width, height) = (pixels.width(), pixels.height());
        debug_assert_eq!((output.width(), output.height()), (width, height));
        output
            .pixels_mut()
            .par_chunks_mut(band * width)
            .enumerate()
            .for_each_init(
                || (scratch.acquire(), source.sky_reader()),
                |(scratch, sky), (idx, rows)| {
                    let start = idx * band;
                    let BandScratch {
                        ring,
                        residual,
                        median,
                    } = &mut **scratch;
                    let mut inputs = InputRows {
                        source,
                        sky: sky.as_mut(),
                        median: self.prefilter == Prefilter::Median3x3 && width >= 3 && height >= 3,
                        residual,
                        held: 0..0,
                        median_row: median,
                    };
                    self.band(&mut inputs, start..start + rows.len() / width, rows, ring);
                },
            );
    }

    /// Output rows `rows` into `output`, from `inputs`, the matched filter's taps from `ring`.
    fn band(
        self,
        inputs: &mut InputRows<'_, '_>,
        rows: Range<usize>,
        output: &mut [f32],
        ring: &mut Vec<f32>,
    ) {
        let pixels = inputs.source.pixels();
        let (width, height) = (pixels.width(), pixels.height());
        let Some(kernel) = self.matched else {
            for (y, out) in rows.zip(output.chunks_exact_mut(width)) {
                inputs.write_row(y, out);
            }
            return;
        };
        let radius = kernel.radius();
        let slots = (2 * radius + 1).min(height);
        // The rows output row `y`'s taps read, mirrored at the edges: every row of an image no
        // taller than the kernel.
        let window = |y: usize| {
            if slots == height {
                0..height
            } else {
                y.saturating_sub(radius)..(y + radius + 1).min(height)
            }
        };
        ring.resize(slots * width, 0.0);
        let mut next = window(rows.start).start;
        for (y, out) in rows.zip(output.chunks_exact_mut(width)) {
            let end = window(y).end;
            for n in next..end {
                let slot = n % slots;
                inputs.with_row(n, |row| {
                    kernel.ring_row(row, &mut ring[slot * width..(slot + 1) * width]);
                });
            }
            next = next.max(end);
            kernel.output_row(ring, Size2us::new(width, height), y, out);
        }
    }
}

/// The rows a band's filters read, computed in order: the source's, less its sky for a residual,
/// and their 3×3 median when the frame takes one.
#[derive(Debug)]
struct InputRows<'s, 'w> {
    source: FilterSource<'s>,
    sky: Option<&'w mut SurfaceReader<'s>>,
    median: bool,
    /// The residual rows `held`, row `n` in slot `n % 3`.
    residual: &'w mut Vec<f32>,
    held: Range<usize>,
    median_row: &'w mut Vec<f32>,
}

impl InputRows<'_, '_> {
    /// Hold residual row `n`, past the rows held, beside the two before it.
    fn hold_residual(&mut self, n: usize) {
        if self.held.contains(&n) {
            return;
        }
        debug_assert!(
            n >= self.held.end || self.held.is_empty(),
            "rows are read in order"
        );
        let FilterSource::Residual { pixels, .. } = self.source else {
            unreachable!("a plane's rows are read where they lie");
        };
        let width = pixels.width();
        self.residual.resize(3 * width, 0.0);
        let sky = self
            .sky
            .as_mut()
            .expect("a residual's worker reads its sky")
            .row(n);
        let slot = &mut self.residual[(n % 3) * width..(n % 3 + 1) * width];
        for ((out, &value), &sky) in slot.iter_mut().zip(pixels.row(n)).zip(sky) {
            *out = value - sky;
        }
        self.held = if self.held.end == n {
            self.held.start.max((n + 1).saturating_sub(3))..n + 1
        } else {
            n..n + 1
        };
    }

    /// Source row `n`, which a residual holds.
    fn source_row(&self, n: usize) -> &[f32] {
        match self.source {
            FilterSource::Plane(pixels) => pixels.row(n),
            FilterSource::Residual { pixels, .. } => {
                debug_assert!(
                    self.held.contains(&n),
                    "a residual row is held before it is read"
                );
                let width = pixels.width();
                &self.residual[(n % 3) * width..(n % 3 + 1) * width]
            }
        }
    }

    /// Hold every source row that row `n`'s median or filter reads.
    fn hold(&mut self, n: usize) {
        if matches!(self.source, FilterSource::Plane(_)) {
            return;
        }
        let height = self.source.pixels().height();
        let rows = if self.median {
            n.saturating_sub(1)..(n + 2).min(height)
        } else {
            n..n + 1
        };
        for row in rows {
            self.hold_residual(row);
        }
    }

    /// Row `n`'s 3×3 neighbourhood among the rows held.
    fn neighbourhood(&self, n: usize) -> MedianRows<'_> {
        let height = self.source.pixels().height();
        MedianRows {
            above: n.checked_sub(1).map(|above| self.source_row(above)),
            cur: self.source_row(n),
            below: (n + 1 < height).then(|| self.source_row(n + 1)),
        }
    }

    /// Hand `read` input row `n`: the source row, or its median.
    fn with_row(&mut self, n: usize, read: impl FnOnce(&[f32])) {
        self.hold(n);
        if self.median {
            let mut median = mem::take(self.median_row);
            median.resize(self.source.pixels().width(), 0.0);
            self.neighbourhood(n).filter_into(&mut median);
            read(&median);
            *self.median_row = median;
        } else {
            read(self.source_row(n));
        }
    }

    /// Input row `n` into `out`, a row of the output.
    fn write_row(&mut self, n: usize, out: &mut [f32]) {
        self.hold(n);
        if self.median {
            self.neighbourhood(n).filter_into(out);
        } else {
            out.copy_from_slice(self.source_row(n));
        }
    }
}

#[cfg(test)]
mod tests;
