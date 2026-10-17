//! Stacking configuration: how the samples at a pixel combine ([`Combine`]), and the policy a
//! frame set's role asks for around it ([`StackConfig`]).

pub(crate) mod combine;
pub(crate) mod combine_method;
pub(crate) mod small_n;
pub(crate) mod weighting;

use std::path::Path;

use crate::combine::error::{StackConfigError, StackError};
use crate::combine::frame_cache::FrameCache;
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::normalization::Normalization;
use crate::combine::rejection::Rejection;
use crate::combine::stack;
use crate::combine::stack::planned_combine::PlannedCombine;
use crate::combine::stack::{CombinePlan, FrameSet, FrameSource};
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::stack_config::small_n::SmallN;
use crate::combine::stack_config::weighting::Weighting;
use crate::combine::stack_frame::StackFrame;
use crate::config_value::ConfigValue;
use crate::frame_store::stored_frame::StoredFrame;
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
#[derive(Debug, Clone, PartialEq)]
pub struct StackConfig {
    /// How the samples at a pixel combine.
    pub combine: Combine,
    /// Frame weighting strategy.
    pub weighting: Weighting,
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    /// Which ancillary per-pixel planes the combine should produce. Defaults to coverage, weight
    /// and variance — they are what makes the stacked master measurable — but each is a full
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
    /// This configuration for the frames left once the input frames at `dropped` (ascending)
    /// are gone: manual weights, given one per input frame, follow their frames; every other
    /// setting applies unchanged.
    pub(crate) fn for_survivors(&self, dropped: &[usize]) -> Self {
        let mut config = self.clone();
        if let Weighting::Manual(weights) = &mut config.weighting {
            let mut index = 0;
            weights.retain(|_| {
                let kept = dropped.binary_search(&index).is_err();
                index += 1;
                kept
            });
        }
        config
    }

    /// Preset for light frames: σ-clip σ=2.5, global normalization, noise weighting. Frames of one
    /// field differ in sky level and transparency, so lights are put on one scale and weighted by
    /// their noise, as Siril, PixInsight and DSS do by default.
    pub const fn light() -> Self {
        Self {
            combine: Combine::sigma_clip(2.5),
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
            combine: Combine {
                method: CombineMethod::Mean(Rejection::sigma_clip(3.0)),
                // Stricter than the default floor: a master flat from fewer than 8 frames uses the
                // median, since σ-clip statistics on so few smooth flats aren't worth the noise.
                small_n: SmallN::median_below(8),
            },
            weighting: Weighting::Equal,
            normalization: Normalization::Multiplicative,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Validate the configuration for a run over `frame_count` frames: every parameter, and one
    /// manual weight per frame.
    pub(crate) fn validate(&self, frame_count: usize) -> Result<(), StackConfigError> {
        ConfigValue::count("min_survivors", self.min_survivors)
            .must_be("at least 1", self.min_survivors >= 1)?;
        if let CombineMethod::Mean(rejection) = &self.combine.method {
            rejection.validate()?;
        }

        if matches!(
            self.combine.small_n.fallback,
            CombineMethod::Mean(rejection) if rejection != Rejection::None
        ) {
            return Err(StackConfigError::RejectingSmallNFallback);
        }

        // A weight of 0 would keep a frame in the gather that adds nothing to the mean: a pixel
        // only such frames cover would read 0 as if measured. Dropping the frame is the honest
        // form.
        if let Weighting::Manual(weights) = &self.weighting {
            if let Some((index, &value)) = weights
                .iter()
                .enumerate()
                .find(|(_, value)| !value.is_finite() || **value <= 0.0)
            {
                return Err(StackConfigError::InvalidManualWeight { index, value });
            }
            let sum: f32 = weights.iter().sum();
            if !sum.is_finite() || sum <= 0.0 {
                return Err(StackConfigError::InvalidManualWeightSum);
            }
            if weights.len() != frame_count {
                return Err(StackConfigError::ManualWeightCountMismatch {
                    expected: frame_count,
                    actual: weights.len(),
                });
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
    /// - The configuration is invalid or its manual-weight count doesn't match the paths
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
    ///     combine: Combine::median(),
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
        let set = FrameSet {
            source: FrameSource::Paths,
            count: paths.len(),
        };
        // Files on disk carry no coverage, so the combine treats every pixel as fully covered.
        // The cancel token rides on the cache from construction, so the load loop polls it too.
        self.combine_cached(set, || {
            FrameCache::from_paths(
                &paths,
                PlannedCombine {
                    normalization: self.normalization,
                    quality: self.quality,
                },
                IngestRun::new(ingest, monitor),
            )
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
    /// Returns an error when the configuration is invalid, manual-weight count doesn't match the
    /// frame count, an image contains a non-finite sample, image/quality-plane dimensions differ,
    /// or normalization is requested for registered frames with no common valid support.
    pub fn stack_images(
        &self,
        frames: Vec<StackFrame>,
        monitor: ProgressMonitor,
    ) -> Result<StackProduct, StackError> {
        let set = FrameSet {
            source: FrameSource::Memory,
            count: frames.len(),
        };
        self.combine_cached(set, || {
            FrameCache::from_stack_frames(frames, self.normalization, monitor)
        })
    }

    /// Combine `frames`, which the shared frame store produced, over `core`.
    pub(crate) fn stack_stored_frames(
        &self,
        frames: Vec<StoredFrame>,
        core: CacheCore,
    ) -> Result<StackProduct, StackError> {
        let set = FrameSet {
            source: FrameSource::FrameStore,
            count: frames.len(),
        };
        self.combine_cached(set, || {
            FrameCache::from_stored_frames(frames, core, self.normalization)
        })
    }

    /// The single gate every combine passes through: reject an empty frame set, validate the
    /// configuration against the frame count, build the cache, log what the run turned out to be,
    /// reduce, and decide whether the result is real.
    ///
    /// Every entry point routes here — including `MasterStack`, which builds a CFA cache of its
    /// own — so config validation happens in exactly one place rather than being a convention each
    /// entry point has to remember.
    ///
    /// `build` is deferred rather than taken as a built cache so that validation runs before a
    /// potentially long load, and so the cache — whose spilled planes hold their disk space —
    /// drops at the end of this scope on every path.
    pub(crate) fn combine_cached<E: From<StackError>>(
        &self,
        set: FrameSet,
        build: impl FnOnce() -> Result<FrameCache, E>,
    ) -> Result<StackProduct, E> {
        let FrameSet {
            source,
            count: frame_count,
        } = set;
        if frame_count == 0 {
            return Err(StackError::NoFrames.into());
        }
        self.validate(frame_count).map_err(StackError::from)?;

        let cache = build()?;
        // Logged after the load so the tier and quality facts come from the cache itself rather
        // than being restated at each entry point.
        tracing::info!(
            ?source,
            frame_count,
            method = ?self.combine.method,
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

        Ok(self.run_stacking(&cache)?)
    }

    /// Combine the cached frames into one stacked product under this configuration — see
    /// [`stack::combine`].
    pub(crate) fn run_stacking(&self, cache: &FrameCache) -> Result<StackProduct, StackError> {
        stack::combine(
            cache,
            CombinePlan {
                combine: self.combine,
                weighting: &self.weighting,
                quality: self.quality,
                min_survivors: self.min_survivors,
            },
        )
    }
}

#[cfg(test)]
mod tests;
