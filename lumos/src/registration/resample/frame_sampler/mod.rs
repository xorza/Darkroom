//! [`FrameSampler`]: one frame's warp from a source position to every channel's sample and the
//! pixel's quality.
//!
//! Each output pixel's window is built once — its taps, their weights and the weight sums — and
//! every channel samples through it, so the coefficients a channel uses are the ones its quality
//! describes. Coverage is the share of the kernel's magnitude `Σ |L|` the window's taps with data
//! hold: 1 where the whole kernel lands on data, falling across the source edge and around flagged
//! pixels. Confidence is the Kish effective sample size of the coefficients the sample was taken
//! with. The flags are the carried flags of the source pixels the sample gave weight; see
//! `FlaggedSources`.
//!
//! The two agree on where there is data: both are zero exactly where the sample is the border. The
//! combine gates on coverage alone and divides the noise by confidence, so a covered pixel at zero
//! confidence would enter its statistics with no finite noise; see `PixelCoverage`, and
//! `FrameCheck::quality_pair`, which holds caller-supplied planes to the same pairing.
//!
//! **Where the clamp acts, confidence describes the unclamped coefficients.** The clamp scales the
//! negative lobes down per channel, which raises the true effective sample size (a kernel's
//! negative lobes cost it variance), so the map understates it at those pixels — beside bright
//! stars, by up to the ratio of the positive lobes' Kish size to the whole kernel's.

use arrayvec::ArrayVec;

use crate::image::flags::pixel_flags::Reach;
use crate::image::flags::quality_flags::QualityFlags;
use crate::registration::resample::flagged_sources::{FlaggedSources, Validity};
use crate::registration::resample::interior_window::InteriorWindow;
use crate::registration::resample::ringing_clamp::RingingClamp;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::source_pos::{SourceAt, SourcePos};
use crate::registration::resample::tap_window::{TapWindow, WindowWeights};
use crate::registration::resample::warp_config::WarpConfig;
use crate::registration::resample::warp_kernel::{Filter, TapAxis, WarpKernel};
use crate::registration::warp_transform::WarpTransform;
use common::USize2;
use simd::{Isa, Kernel};

/// How a frame's pixels are sampled.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SampleMethod {
    /// The pixel nearest the position, whole: coverage and confidence are 1 where it holds data.
    Nearest,
    /// A separable kernel over a window.
    Filter(FilterSampling),
}

/// [`SampleMethod::Filter`]: the kernel, and the ringing clamp where the kernel has negative lobes
/// and the clamp is on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FilterSampling {
    kernel: WarpKernel,
    clamp: Option<RingingClamp>,
}

impl SampleMethod {
    /// What `params` asks for, at the stretch `warp` needs over an output frame of `size`.
    pub(super) fn for_frame(params: WarpConfig, warp: &WarpTransform, size: USize2) -> Self {
        match Filter::of(params.method) {
            None => Self::Nearest,
            Some(filter) => Self::Filter(FilterSampling {
                kernel: WarpKernel::for_frame(filter, warp, size),
                clamp: params
                    .clamping_threshold
                    .filter(|_| filter.has_negative_lobes())
                    .map(RingingClamp::new),
            }),
        }
    }

    /// Whether a sample mixes source samples: every method but the nearest pixel.
    pub(crate) const fn interpolates(self) -> bool {
        matches!(self, Self::Filter(_))
    }

    /// The source pixels around a sample's cell a filter's windows read; `None` for the nearest
    /// pixel, which reads its own flags.
    pub(super) const fn window_reach(self) -> Option<Reach> {
        match self {
            Self::Nearest => None,
            Self::Filter(filter) => Some(filter.kernel.window_reach()),
        }
    }
}

/// One frame's sources and how they are sampled.
#[derive(Debug)]
pub(crate) struct FrameSampler<'a> {
    method: SampleMethod,
    sources: ArrayVec<SourcePlane<'a>, 3>,
    flagged: Option<&'a FlaggedSources<'a>>,
    border: f32,
    size: USize2,
}

/// The tap axes of the pixel a row is sampling, kept across pixels and rows for their storage.
#[derive(Debug, Default)]
pub(crate) struct WindowAxes {
    x: TapAxis,
    y: TapAxis,
}

/// Where one output row's samples and quality go.
#[derive(Debug)]
pub(crate) struct RowOutput<'o, 'r> {
    pub(crate) channels: &'o mut [&'r mut [f32]],
    pub(crate) coverage: &'o mut [f32],
    pub(crate) confidence: &'o mut [f32],
    /// For a frame that carries flags.
    pub(crate) flags: Option<&'o mut [u8]>,
}

/// The quality of one output pixel.
#[derive(Debug, Clone, Copy, Default)]
struct PixelQuality {
    coverage: f32,
    confidence: f32,
    flags: QualityFlags,
}

impl<'a> FrameSampler<'a> {
    /// `image` sampled under `rule`; `flagged` holds its sources when it flags pixels.
    pub(crate) fn new(
        rule: SampleRule,
        image: &SourceImage<'a>,
        flagged: Option<&'a FlaggedSources<'a>>,
    ) -> Self {
        Self {
            method: rule.method,
            sources: image.planes.clone(),
            flagged,
            border: rule.border,
            size: image.size(),
        }
    }

    /// Every channel and the quality of one output row at its source `poss`, `None` outside
    /// the source footprint.
    pub(crate) fn sample_row(
        &self,
        poss: &[Option<SourcePos>],
        axes: &mut WindowAxes,
        output: RowOutput<'_, '_>,
    ) {
        debug_assert_eq!(output.channels.len(), self.sources.len());
        #[expect(
            clippy::debug_assert_with_mut_call,
            reason = "`iter` only reads the rows, which are mutable for the caller"
        )]
        {
            debug_assert!(output.channels.iter().all(|row| row.len() == poss.len()));
        }
        SampleRow {
            sampler: self,
            poss,
            axes,
            output,
        }
        .dispatch();
    }

    fn nearest(&self, pos: SourcePos, out: &mut PixelOut<'_, '_>) -> PixelQuality {
        let idx = pos.nearest_idx(self.size);
        if self
            .flagged
            .and_then(FlaggedSources::validity)
            .is_some_and(|validity| !validity.holds(idx))
        {
            return self.no_data(out);
        }
        for (row, source) in out.channels.iter_mut().zip(&self.sources) {
            row[out.x] = source.pixels[idx];
        }
        PixelQuality {
            coverage: 1.0,
            confidence: 1.0,
            flags: self
                .flagged
                .map_or_else(QualityFlags::default, |flagged| flagged.carried_at(idx)),
        }
    }

    /// The border in every channel, with no quality.
    fn no_data(&self, out: &mut PixelOut<'_, '_>) -> PixelQuality {
        for row in out.channels.iter_mut() {
            row[out.x] = self.border;
        }
        PixelQuality::default()
    }
}

/// How a frame is sampled: its method, and the value of a pixel outside the source footprint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SampleRule {
    pub(crate) method: SampleMethod,
    pub(crate) border: f32,
}

/// Where one output pixel lands: column `x` of each channel's row.
#[derive(Debug)]
struct PixelOut<'c, 'r> {
    channels: &'c mut [&'r mut [f32]],
    x: usize,
}

/// A window's taps summed into a sample: the window, the validity its sums are masked by near an
/// excluded pixel, the weights that normalize it, and the clamp that holds its ringing.
#[derive(Debug, Clone, Copy)]
struct WindowSum<'w> {
    window: &'w TapWindow<'w>,
    validity: Option<Validity<'w>>,
    weights: WindowWeights,
    clamp: Option<RingingClamp>,
}

/// A [`FrameSampler`] at work on one row: the Isa it samples on, and the tap axes of the pixel at
/// hand.
#[derive(Debug)]
struct RowSampler<'s, 'a, S> {
    sampler: &'s FrameSampler<'a>,
    isa: S,
    axes: &'s mut WindowAxes,
}

impl<S: Isa> RowSampler<'_, '_, S> {
    /// One pixel at its source `pos`, every channel written into `out`.
    #[inline(always)]
    fn pixel(&mut self, pos: SourcePos, out: &mut PixelOut<'_, '_>) -> PixelQuality {
        match self.sampler.method {
            SampleMethod::Nearest => self.sampler.nearest(pos, out),
            SampleMethod::Filter(filter) => self.filtered(filter, pos, out),
        }
    }

    /// The filter's kernel over the window at `pos`: the whole kernel where it lands on data;
    /// its taps with data, normalized, where the source edge or a flagged pixel cut it and what is
    /// left is [well conditioned](WindowWeights::well_conditioned); otherwise Bilinear at the same
    /// stretch over its own taps with data, whose weights are never negative and so always average.
    #[inline(always)]
    fn filtered(
        &mut self,
        FilterSampling { kernel, clamp }: FilterSampling,
        pos: SourcePos,
        out: &mut PixelOut<'_, '_>,
    ) -> PixelQuality {
        let (sampler, isa) = (self.sampler, self.isa);
        let near = sampler
            .flagged
            .map_or_else(QualityFlags::default, |flagged| flagged.near(pos));
        if near == QualityFlags::default()
            && let Some(window) = InteriorWindow::new(
                isa,
                &kernel,
                SourceAt {
                    pos,
                    size: sampler.size,
                },
            )
        {
            let weights = window.weights();
            let total = weights.total();
            for (row, source) in out.channels.iter_mut().zip(&sampler.sources) {
                row[out.x] = match clamp {
                    Some(clamp) => {
                        clamp.sample(window.lobe_sums(isa, *source), total, || weights.lobes())
                    }
                    None => window.total(isa, *source) / total,
                };
            }
            return PixelQuality {
                coverage: 1.0,
                confidence: weights.confidence(),
                flags: QualityFlags::default(),
            };
        }

        kernel.axis(isa, pos.x(), &mut self.axes.x);
        kernel.axis(isa, pos.y(), &mut self.axes.y);
        let Some(window) = TapWindow::new(&self.axes.x, &self.axes.y, sampler.size) else {
            return sampler.no_data(out);
        };
        let validity = sampler
            .flagged
            .filter(|_| near.intersects(QualityFlags::RESAMPLE_EXCLUDED))
            .and_then(FlaggedSources::validity);
        let carried = |axes: &WindowAxes| match sampler.flagged {
            Some(flagged) if near.intersects(QualityFlags::RESAMPLE_CARRIED) => {
                flagged.carried_in(&axes.x, &axes.y)
            }
            _ => QualityFlags::default(),
        };
        let weights = match validity {
            Some(validity) => window.masked_weights(isa, validity),
            None => window.weights(),
        };
        let sum = WindowSum {
            window: &window,
            validity,
            weights,
            clamp,
        };
        if validity.is_none() && !window.is_clipped() {
            self.write(sum, out);
            return PixelQuality {
                coverage: 1.0,
                confidence: weights.confidence(),
                flags: carried(self.axes),
            };
        }
        let coverage = (weights.magnitude() / window.whole_magnitude()).min(1.0);
        if weights.well_conditioned() {
            self.write(sum, out);
            return PixelQuality {
                coverage,
                confidence: weights.confidence(),
                flags: carried(self.axes),
            };
        }

        let fallback = kernel.bilinear();
        fallback.axis(isa, pos.x(), &mut self.axes.x);
        fallback.axis(isa, pos.y(), &mut self.axes.y);
        let Some(window) = TapWindow::new(&self.axes.x, &self.axes.y, sampler.size) else {
            return sampler.no_data(out);
        };
        let weights = match validity {
            Some(validity) => window.masked_weights(isa, validity),
            None => window.weights(),
        };
        // Bilinear's taps with weight sit within the kernel's, where it is positive too, so data
        // under them means coverage; the test keeps the pairing should rounding say otherwise.
        if weights.total() <= 0.0 || coverage <= 0.0 {
            return sampler.no_data(out);
        }
        let sum = WindowSum {
            window: &window,
            validity,
            weights,
            clamp: None,
        };
        self.write(sum, out);
        PixelQuality {
            coverage,
            confidence: weights.confidence(),
            flags: carried(self.axes),
        }
    }

    /// Every channel's sample of `sum` into `out`.
    #[inline(always)]
    fn write(&self, sum: WindowSum<'_>, out: &mut PixelOut<'_, '_>) {
        let WindowSum {
            window,
            validity,
            weights,
            clamp,
        } = sum;
        let isa = self.isa;
        for (row, source) in out.channels.iter_mut().zip(&self.sampler.sources) {
            row[out.x] = match clamp {
                Some(clamp) => clamp.sample(
                    window.lobe_sums(isa, *source, validity),
                    weights.total(),
                    || weights.lobes(),
                ),
                None => window.total(isa, *source, validity) / weights.total(),
            };
        }
    }
}

/// [`FrameSampler::sample_row`] as a kernel, dispatched once per row.
#[derive(Debug)]
struct SampleRow<'s, 'a, 'o, 'r> {
    sampler: &'s FrameSampler<'a>,
    poss: &'s [Option<SourcePos>],
    axes: &'s mut WindowAxes,
    output: RowOutput<'o, 'r>,
}

impl Kernel for SampleRow<'_, '_, '_, '_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let RowOutput {
            channels,
            coverage,
            confidence,
            mut flags,
        } = self.output;
        let mut row = RowSampler {
            sampler: self.sampler,
            isa,
            axes: self.axes,
        };
        for (x, pos) in self.poss.iter().enumerate() {
            let out = &mut PixelOut {
                channels: &mut *channels,
                x,
            };
            let quality = match *pos {
                Some(pos) => row.pixel(pos, out),
                None => row.sampler.no_data(out),
            };
            coverage[x] = quality.coverage;
            confidence[x] = quality.confidence;
            if let Some(flags) = flags.as_deref_mut() {
                flags[x] = quality.flags.byte();
            }
        }
    }
}

#[cfg(test)]
mod tests;
