//! Stacking configuration: how the samples at a pixel combine ([`Combine`]), and the policy a
//! frame set's role asks for around it ([`StackConfig`]).

pub(crate) mod combine;
pub(crate) mod small_n;
pub(crate) mod weighting;

use std::path::Path;

use crate::combine::error::StackError;
use crate::combine::frame_cache::CombineRequest;
use crate::combine::frame_cache::FrameCache;
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::normalization::Normalization;
use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::sample_quality::SampleQuality;
use crate::combine::rejection::Rejection;
use crate::combine::stack::planned_combine::PlannedCombine;
use crate::combine::stack::quantization;
use crate::combine::stack::rejected_mean::{RejectedMean, mean_of_all};
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::small_n::SmallN;
use crate::combine::stack_config::weighting::Weighting;
use crate::combine::stack_frame::StackFrame;
use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::frame_origin::FrameOrigin;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::stack_product::StackProduct;
use crate::stack_product::quality_planes::QualityPlanes;

/// Configuration for stacking a set of frames: how each pixel combines, and the normalization,
/// weighting and output planes the set's role asks for.
///
/// There is no default: the policy depends on what the frames are, so every configuration starts
/// from a role preset — [`Self::light`], [`Self::flat`] or [`Self::bias_or_dark`] — and changes
/// what it needs.
///
/// # Examples
///
/// ```no_run
/// use lumos::{Combine, IngestConfig, ProgressMonitor, Rejection, StackConfig};
///
/// let paths = ["frame1.fits", "frame2.fits", "frame3.fits"];
/// let ingest = IngestConfig::default();
///
/// // Light frames, as the preset combines them.
/// let result = StackConfig::light().stack_paths(
///     &paths,
///     &ingest,
///     ProgressMonitor::unattended(),
/// )?;
///
/// // Light frames by another method, normalized and weighted as lights still.
/// let config = StackConfig {
///     combine: Combine::winsorized(3.0),
///     ..StackConfig::light()
/// };
/// let result = config.stack_paths(
///     &paths,
///     &ingest,
///     ProgressMonitor::unattended(),
/// )?;
/// # Ok::<(), lumos::StackError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackConfig {
    /// How the samples at a pixel combine.
    pub combine: Combine,
    /// Frame weighting strategy.
    pub weighting: Weighting,
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    /// Which ancillary per-pixel planes the combine produces. The presets take
    /// [`QualityPlanes::STANDARD`], which makes the master measurable; each plane is a full
    /// image-sized allocation, so a caller that discards them should say so.
    pub quality: QualityPlanes,
    /// The fewest samples a pixel keeps when the combine leaves samples out. A flagged sample
    /// (saturated, repaired, cosmic ray, defect, flat floor) is left out only while this many
    /// unflagged samples remain at its pixel; otherwise every sample stays. Rejection never leaves
    /// fewer: a pixel with no more samples than this is not rejected from, and a pass that would
    /// keep fewer keeps this many nearest its centre instead. PixInsight keeps 3, Siril 4. At
    /// least 1.
    pub min_survivors: usize,
}

/// [`StackConfig::min_survivors`] by default, as PixInsight's `ImageIntegration`.
pub(super) const DEFAULT_MIN_SURVIVORS: usize = 3;

impl StackConfig {
    /// The σ the light preset clips at.
    pub const LIGHT_SIGMA: f32 = 2.5;

    /// Preset for light frames: σ-clip at [`Self::LIGHT_SIGMA`], global normalization, noise
    /// weighting. Frames of one field differ in sky level and transparency, so lights are put on
    /// one scale and weighted by their noise, as Siril, PixInsight and DSS do by default.
    pub const fn light() -> Self {
        Self {
            combine: Combine::sigma_clip(Self::LIGHT_SIGMA),
            weighting: Weighting::Noise,
            normalization: Normalization::Global,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Preset for bias and dark frames: Winsorized σ=3.0, no normalization, equal weights. One
    /// preset, because both measure a level that every frame shares, with nothing between frames
    /// to normalize.
    pub const fn bias_or_dark() -> Self {
        Self {
            combine: Combine::winsorized(3.0),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Preset for flat frames: σ-clip σ=3.0, multiplicative normalization, equal weights.
    pub const fn flat() -> Self {
        // σ=3.0 matches the bias-or-dark preset and ccdproc's `combine` default (3σ low/high);
        // flats are smooth, so a permissive cut just trims clear outliers (dust shadows move
        // between flats).
        Self {
            combine: Combine::RejectedMean {
                rejection: Rejection::sigma_clip(3.0),
                // Stricter than the default floor: a master flat from fewer than 8 frames uses the
                // median, since σ-clip statistics on so few smooth flats aren't worth the noise.
                small_n: Some(SmallN::median_below(8)),
            },
            weighting: Weighting::Equal,
            normalization: Normalization::Multiplicative,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Validate every parameter: the survivor floor, the rejection's, and its small-sample floor,
    /// which the median's tabled variance factor bounds.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::count("min_survivors", self.min_survivors)
            .must_be("at least 1", self.min_survivors >= 1)?;
        if let Combine::RejectedMean { rejection, small_n } = &self.combine {
            rejection.validate()?;
            if let Some(small_n) = small_n {
                ConfigField::count("small_n.min_frames", small_n.min_frames).must_be(
                    "between 2 and 33",
                    (2..=SmallN::MAX_MIN_FRAMES).contains(&small_n.min_frames),
                )?;
            }
        }
        Ok(())
    }

    /// Stack multiple images from disk into a single result.
    ///
    /// This is the main entry point for stacking frames stored as files. To stack frames already
    /// held in memory, use [`Self::stack_images`].
    ///
    /// # Arguments
    ///
    /// * `paths` - Paths to input images
    /// * `ingest` - How the frames are decoded, the memory the run plans against, and where it
    ///   spills
    ///
    /// # Returns
    ///
    /// A [`StackProduct`] whose coverage is the fraction of frames with geometric support at each
    /// pixel. Its per-channel weight map describes the surviving samples; `variance` is available
    /// for mean output and absent for median output.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No paths are provided
    /// - The configuration is invalid
    /// - Image loading fails
    /// - Image dimensions don't match
    /// - A decoded image contains a non-finite sample
    /// - Cache directory creation fails (for disk-backed storage)
    ///
    /// # Examples
    ///
    /// Pass [`ProgressMonitor::unattended()`] when you neither follow nor cancel the run.
    ///
    /// ```no_run
    /// use lumos::{Combine, IngestConfig, ProgressMonitor, StackConfig};
    ///
    /// let paths = ["frame1.fits", "frame2.fits", "frame3.fits"];
    /// let ingest = IngestConfig::default();
    /// let config = StackConfig::light();
    /// let result = config.stack_paths(
    ///     &paths,
    ///     &ingest,
    ///     ProgressMonitor::unattended(),
    /// )?;
    /// let median = StackConfig {
    ///     combine: Combine::Median,
    ///     ..StackConfig::light()
    /// };
    /// let result = median.stack_paths(
    ///     &paths,
    ///     &ingest,
    ///     ProgressMonitor::unattended(),
    /// )?;
    /// # Ok::<(), lumos::StackError>(())
    /// ```
    pub fn stack_paths<P: AsRef<Path>>(
        &self,
        paths: &[P],
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, StackError> {
        let paths: Vec<&Path> = paths.iter().map(AsRef::as_ref).collect();
        // Files on disk carry no coverage, so the combine treats every pixel as fully covered.
        // The cancel token rides on the cache from construction, so the load loop polls it too.
        self.combine_cached(FrameOrigin::Files, paths.len(), || {
            FrameCache::from_paths(&paths, self.planned(), IngestRun::new(ingest, monitor))
        })
    }

    /// Stack frames already held in memory into a single result.
    ///
    /// The in-memory counterpart to [`Self::stack_paths`]: skips the disk round-trip when the
    /// caller already owns the decoded frames (e.g. straight off calibration or warping). The
    /// frames are consumed. Pass [`ProgressMonitor::unattended()`] when you neither follow nor
    /// cancel the run.
    ///
    /// Each [`StackFrame`] may carry per-pixel `coverage` and `confidence`. Coverage gates
    /// inclusion; confidence divides the sample's noise variance, for rejection and the variance
    /// plane, and does not weight the mean. Frames without either plane use full support and unit
    /// confidence. Plain `LinearImage`s convert via `.into()` and [`StackFrame::registered`]
    /// converts a source image plus its [`WarpResult`](crate::registration::resample::WarpResult)
    /// without remeasuring noise after interpolation.
    ///
    /// Returns an error when the configuration is invalid, an image contains a non-finite sample,
    /// image/quality-plane dimensions differ, or normalization is requested for registered frames
    /// with no common valid support.
    pub fn stack_images(
        &self,
        frames: Vec<StackFrame>,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, StackError> {
        self.combine_cached(FrameOrigin::Held, frames.len(), || {
            FrameCache::from_stack_frames(frames, self.normalization, monitor)
        })
    }

    /// Combine `frames`, which the shared frame store produced, over `core`.
    pub(crate) fn stack_stored_frames(
        &self,
        frames: Vec<StoredFrame>,
        core: CacheCore,
    ) -> Result<StackProduct, StackError> {
        self.combine_cached(FrameOrigin::Stored, frames.len(), || {
            FrameCache::from_stored_frames(frames, core, self.normalization)
        })
    }

    /// What a run's memory plan reads of this configuration: the normalization and the planes the
    /// combine holds.
    pub(crate) const fn planned(&self) -> PlannedCombine {
        PlannedCombine {
            normalization: self.normalization,
            quality: self.quality,
        }
    }

    /// The single gate every combine of `frame_count` frames from `origin` passes through, so
    /// validation happens in exactly one place.
    ///
    /// `build` is deferred so that validation runs before a potentially long load, and so the
    /// cache, whose spilled planes hold their disk space, drops at the end of this scope on every
    /// path.
    pub(crate) fn combine_cached<E: From<StackError>>(
        &self,
        origin: FrameOrigin,
        frame_count: usize,
        build: impl FnOnce() -> Result<FrameCache, E>,
    ) -> Result<StackProduct, E> {
        if frame_count == 0 {
            return Err(StackError::NoFrames.into());
        }
        self.validate().map_err(StackError::from)?;

        let cache = build()?;
        // Logged after the load so the tier and quality facts come from the cache itself.
        tracing::info!(
            ?origin,
            frame_count,
            combine = ?self.combine,
            weighting = ?self.weighting,
            normalization = ?self.normalization,
            disk_tier = cache.core.tier.spills(),
            // Not "warped": a frame carries these when a warp produced them *or* when its source
            // declared pixels with no measurement.
            frame_quality = cache
                .frames
                .iter()
                .any(|frame| !frame.quality.is_none()),
            "Combining frames"
        );

        Ok(self.combine(&cache)?)
    }

    /// Combine the cached frames into one stacked product.
    ///
    /// Coverage gates a frame's contribution at a pixel while confidence divides its noise model; a
    /// frame with neither plane contributes everywhere at unit confidence.
    ///
    /// # Errors
    ///
    /// [`StackError::Cancelled`] if the cache's token was set. The chunk walk abandons the output
    /// between chunks rather than unwinding, so a cancelled run still produces a `StackProduct` —
    /// one holding zeros wherever it stopped. An error keeps the partial image from being mistaken
    /// for a stack, which trusting each caller to consult the token did not.
    pub(crate) fn combine(&self, cache: &FrameCache) -> Result<StackProduct, StackError> {
        let Self {
            combine,
            weighting,
            quality: requested,
            min_survivors,
            ..
        } = *self;
        let stats = || cache.frames.iter().map(|frame| &frame.admitted.stats);
        let frame_count = cache.frames.len();
        if let Combine::RejectedMean {
            small_n: Some(small_n),
            ..
        } = combine
            && small_n.falls_back(frame_count)
        {
            tracing::warn!(
                frame_count,
                min_frames = small_n.min_frames,
                "too few frames for the configured rejection; every pixel combines by the fallback",
            );
        }
        let norms = cache.frame_norms.as_deref();
        let slots = cache.slots;
        let normalized = || NormalizedStats {
            stats: stats(),
            norms,
            slots,
        };
        let weights = match combine {
            Combine::Mean | Combine::RejectedMean { .. } => {
                FrameWeights::resolve(weighting, normalized())?
            }
            Combine::Median => None,
        };

        // A median is not a linear combination, so it has no variance to report whatever the caller
        // asked for. Resolving here means the reducer never allocates a plane it would drop.
        let planes = combine.planes(requested);
        let quality = SampleQuality::of(planes);

        // A master states the step of its inputs: the largest any frame contributes once its
        // normalization scaled it, whatever each pixel's survivors were.
        let quantization_sigma = quantization::largest_step(stats(), norms);

        let combined = match combine {
            Combine::Median => {
                let request = CombineRequest {
                    weights: None,
                    planes,
                    min_survivors,
                    noise: None,
                };
                cache.process_chunked(request, |samples, _| {
                    CombinedSample::of_median(samples, quality)
                })
            }
            Combine::Mean => {
                let noise = planes
                    .inverse_variance
                    .then(|| SampleNoise::new(normalized()));
                let request = CombineRequest {
                    weights: weights.as_ref(),
                    planes,
                    min_survivors,
                    noise: noise.as_ref(),
                };
                cache.process_chunked(request, |samples, _| mean_of_all(&samples, quality))
            }
            Combine::RejectedMean { rejection, small_n } => {
                let noise = (rejection.measures_spread() || planes.inverse_variance)
                    .then(|| SampleNoise::new(normalized()));
                if rejection.uses_ccd_model()
                    && let Some(idx) = noise.as_ref().and_then(SampleNoise::first_without_gain)
                {
                    return Err(StackError::UnknownGain { idx });
                }
                let request = CombineRequest {
                    weights: weights.as_ref(),
                    planes,
                    min_survivors,
                    noise: noise.as_ref(),
                };
                let mean = RejectedMean {
                    rejection,
                    small_n,
                    min_survivors,
                    quality,
                };
                cache.process_chunked(request, mean.reducer())
            }
        };

        // Every reduction leaves here, so none can hand back a partial stack as a whole one.
        if cache.core.monitor.cancel.is_cancelled() {
            return Err(StackError::Cancelled);
        }
        Ok(cache.finish_product(combined, quantization_sigma))
    }
}

#[cfg(test)]
mod tests;
