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

use crate::io::image::pixel_flags::QualityFlags;
use crate::io::image::pixel_flags::Reach;
use crate::math::size2us::Size2us;
use crate::registration::registration_config::WarpParams;
use crate::registration::resample::flagged_sources::FlaggedSources;
use crate::registration::resample::interior_window::InteriorWindow;
use crate::registration::resample::kernel;
use crate::registration::resample::kernel::warp_kernel::{Filter, TapAxis, WarpKernel};
use crate::registration::resample::ringing_clamp::RingingClamp;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::resample::tap_window::{TapWindow, WindowWeights};
use crate::registration::transform::WarpTransform;
use crate::simd::{F32_LANES, Isa, Kernel};

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
    pub(crate) fn for_frame(params: WarpParams, warp: &WarpTransform, size: Size2us) -> Self {
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

    /// The source pixels around a sample's cell any of its windows reads. Nearest reads the cell or
    /// the next.
    pub(crate) const fn reach(self) -> Reach {
        match self {
            Self::Nearest => Reach {
                before: 0,
                after: 1,
            },
            Self::Filter(filter) => filter.kernel.window_reach(),
        }
    }
}

/// One frame's sources and how they are sampled.
#[derive(Debug)]
pub(crate) struct FrameSampler<'a> {
    method: SampleMethod,
    /// The channels as the windows read them: with excluded pixels at zero for a frame that has
    /// any.
    sources: ArrayVec<SourcePlane<'a>, 3>,
    flagged: Option<&'a FlaggedSources<'a>>,
    border: f32,
    size: Size2us,
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
    /// `image` sampled by `method`; `flagged` holds its sources when it flags pixels.
    pub(crate) fn new(
        method: SampleMethod,
        image: &SourceImage<'a>,
        flagged: Option<&'a FlaggedSources<'a>>,
        border: f32,
    ) -> Self {
        let sources = match flagged.and_then(FlaggedSources::zeroed_planes) {
            Some(zeroed) => zeroed.collect(),
            None => image.planes.clone(),
        };
        Self {
            method,
            sources,
            flagged,
            border,
            size: image.size(),
        }
    }

    /// Every channel and the quality of one output row at its source `positions`, `None` outside
    /// the source footprint.
    pub(crate) fn sample_row(
        &self,
        positions: &[Option<SourcePosition>],
        axes: &mut WindowAxes,
        output: RowOutput<'_, '_>,
    ) {
        debug_assert_eq!(output.channels.len(), self.sources.len());
        debug_assert!(
            output
                .channels
                .iter()
                .all(|row| row.len() == positions.len())
        );
        SampleRow {
            sampler: self,
            positions,
            axes,
            output,
        }
        .dispatch();
    }

    /// One pixel `x` of the row, every channel written into `channels`.
    #[inline(always)]
    fn pixel<S: Isa>(
        &self,
        isa: S,
        position: SourcePosition,
        axes: &mut WindowAxes,
        x: usize,
        channels: &mut [&mut [f32]],
    ) -> PixelQuality {
        match self.method {
            SampleMethod::Nearest => self.nearest(position, x, channels),
            SampleMethod::Filter(filter) => self.filtered(isa, filter, position, axes, x, channels),
        }
    }

    fn nearest(
        &self,
        position: SourcePosition,
        x: usize,
        channels: &mut [&mut [f32]],
    ) -> PixelQuality {
        let index = kernel::nearest_index(self.size, position);
        if self
            .flagged
            .and_then(FlaggedSources::validity)
            .is_some_and(|validity| validity.pixels[index] == 0.0)
        {
            return self.no_data(x, channels);
        }
        for (row, source) in channels.iter_mut().zip(&self.sources) {
            row[x] = source.pixels[index];
        }
        PixelQuality {
            coverage: 1.0,
            confidence: 1.0,
            flags: self
                .flagged
                .map_or(QualityFlags::default(), |flagged| flagged.carried_at(index)),
        }
    }

    /// The filter's kernel over the window at `position`: the whole kernel where it lands on data;
    /// its taps with data, normalized, where the source edge or a flagged pixel cut it and what is
    /// left is [well conditioned](WindowWeights::well_conditioned); otherwise Bilinear at the same
    /// stretch over its own taps with data, whose weights are never negative and so always average.
    #[inline(always)]
    fn filtered<S: Isa>(
        &self,
        isa: S,
        FilterSampling { kernel, clamp }: FilterSampling,
        position: SourcePosition,
        axes: &mut WindowAxes,
        x: usize,
        channels: &mut [&mut [f32]],
    ) -> PixelQuality {
        let near = self
            .flagged
            .map_or(QualityFlags::default(), |flagged| flagged.near(position));
        let x_taps = kernel.taps(position.fx);
        let y_taps = kernel.taps(position.fy);
        if near == QualityFlags::default()
            && x_taps.count <= F32_LANES
            && y_taps.count <= F32_LANES
            && let Some(window) =
                InteriorWindow::new(isa, &kernel, position, x_taps, y_taps, self.size)
        {
            let total = window.total_weight();
            for (row, source) in channels.iter_mut().zip(&self.sources) {
                row[x] = match clamp {
                    Some(clamp) => {
                        clamp.sample(window.lobe_sums(isa, *source), total, || window.lobes(isa))
                    }
                    None => window.total(isa, *source) / total,
                };
            }
            return PixelQuality {
                coverage: 1.0,
                confidence: window.confidence(),
                flags: QualityFlags::default(),
            };
        }

        kernel.axis(isa, position.cell_x, position.fx, &mut axes.x);
        kernel.axis(isa, position.cell_y, position.fy, &mut axes.y);
        let Some(window) = TapWindow::new(&axes.x, &axes.y, self.size) else {
            return self.no_data(x, channels);
        };
        let validity = self
            .flagged
            .filter(|_| near.intersects(QualityFlags::RESAMPLE_EXCLUDED))
            .and_then(FlaggedSources::validity);
        let carried = |axes: &WindowAxes| match self.flagged {
            Some(flagged) if near.intersects(QualityFlags::RESAMPLE_CARRIED) => {
                flagged.carried_in(&axes.x, &axes.y)
            }
            _ => QualityFlags::default(),
        };
        let weights = match validity {
            Some(validity) => window.masked_weights(isa, validity),
            None => window.weights(),
        };
        if validity.is_none() && !window.is_clipped() {
            self.write(isa, &window, weights, clamp, x, channels);
            return PixelQuality {
                coverage: 1.0,
                confidence: weights.confidence(),
                flags: carried(axes),
            };
        }
        let coverage = (weights.magnitude() / window.whole_magnitude()).min(1.0);
        if weights.well_conditioned() {
            self.write(isa, &window, weights, clamp, x, channels);
            return PixelQuality {
                coverage,
                confidence: weights.confidence(),
                flags: carried(axes),
            };
        }

        let fallback = kernel.bilinear();
        fallback.axis(isa, position.cell_x, position.fx, &mut axes.x);
        fallback.axis(isa, position.cell_y, position.fy, &mut axes.y);
        let Some(window) = TapWindow::new(&axes.x, &axes.y, self.size) else {
            return self.no_data(x, channels);
        };
        let weights = match validity {
            Some(validity) => window.masked_weights(isa, validity),
            None => window.weights(),
        };
        // Bilinear's taps with weight sit within the kernel's, where it is positive too, so data
        // under them means coverage; the test keeps the pairing should rounding say otherwise.
        if weights.total() <= 0.0 || coverage <= 0.0 {
            return self.no_data(x, channels);
        }
        self.write(isa, &window, weights, None, x, channels);
        PixelQuality {
            coverage,
            confidence: weights.confidence(),
            flags: carried(axes),
        }
    }

    /// Every channel's sample over `window`, normalized by `weights`.
    #[inline(always)]
    fn write<S: Isa>(
        &self,
        isa: S,
        window: &TapWindow<'_>,
        weights: WindowWeights,
        clamp: Option<RingingClamp>,
        x: usize,
        channels: &mut [&mut [f32]],
    ) {
        for (row, source) in channels.iter_mut().zip(&self.sources) {
            row[x] = match clamp {
                Some(clamp) => {
                    clamp.sample(window.lobe_sums(isa, *source), weights.total(), || {
                        weights.lobes()
                    })
                }
                None => window.total(isa, *source) / weights.total(),
            };
        }
    }

    fn no_data(&self, x: usize, channels: &mut [&mut [f32]]) -> PixelQuality {
        for row in channels.iter_mut() {
            row[x] = self.border;
        }
        PixelQuality::default()
    }
}

/// [`FrameSampler::sample_row`] as a kernel, dispatched once per row.
#[derive(Debug)]
struct SampleRow<'s, 'a, 'o, 'r> {
    sampler: &'s FrameSampler<'a>,
    positions: &'s [Option<SourcePosition>],
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
        for (x, position) in self.positions.iter().enumerate() {
            let quality = match *position {
                Some(position) => self.sampler.pixel(isa, position, self.axes, x, channels),
                None => self.sampler.no_data(x, channels),
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
