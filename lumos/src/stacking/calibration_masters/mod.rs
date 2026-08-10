//! Calibration master frame creation and management.

pub(crate) mod cosmic_ray;
pub(crate) mod defect_map;
mod fits;
mod prepared_flat;

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod tests;

use std::path::Path;

use common::CancelToken;
use rayon::prelude::*;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType};
use crate::io::image::error::ImageError;
use crate::io::image::load_context::LoadContext;
use crate::math::size2us::Size2us;
use crate::memory;
use crate::stacking::combine::cache::FrameCache;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::stack::combine_cached;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack_product::quality_planes::QualityPlanes;
use defect_map::DefectMap;

/// Default sigma threshold for defect detection.
///
/// A pixel is flagged as defective if it exceeds the per-color residual median by more than
/// `sigma_threshold × σ`, where robust bulk statistics and source resolution determine σ.
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_SIGMA_THRESHOLD: f32 = 5.0;

/// Four calibration roles carrying values of one common type.
///
/// Named fields prevent swapping roles when `T` is the same for all four. Raw inputs use path
/// slices, while prebuilt inputs use optional CFA images.
#[derive(Debug, Default)]
pub struct CalibrationSet<T> {
    /// Thermal-noise calibration data.
    pub dark: T,
    /// Vignetting and dust correction data.
    pub flat: T,
    /// Read-noise calibration data.
    pub bias: T,
    /// Dark calibration data taken at the flat exposure time.
    pub flat_dark: T,
}

impl<T> CalibrationSet<T> {
    /// The value for `component`, or `None` for [`CalibrationComponent::Defects`], which is not
    /// one of the four master roles this set holds.
    pub fn get(&self, component: CalibrationComponent) -> Option<&T> {
        match component {
            CalibrationComponent::Dark => Some(&self.dark),
            CalibrationComponent::Flat => Some(&self.flat),
            CalibrationComponent::Bias => Some(&self.bias),
            CalibrationComponent::FlatDark => Some(&self.flat_dark),
            CalibrationComponent::Defects => None,
        }
    }

    /// [`Self::get`] by unique reference, for filling a set one role at a time.
    pub(crate) fn get_mut(&mut self, component: CalibrationComponent) -> Option<&mut T> {
        match component {
            CalibrationComponent::Dark => Some(&mut self.dark),
            CalibrationComponent::Flat => Some(&mut self.flat),
            CalibrationComponent::Bias => Some(&mut self.bias),
            CalibrationComponent::FlatDark => Some(&mut self.flat_dark),
            CalibrationComponent::Defects => None,
        }
    }

    /// The four roles in calibration order, each with the component that names it. The single
    /// place that decides what "all the roles" means — a caller that iterates cannot miss one,
    /// and adding a fifth is a compile error here rather than a silent omission elsewhere.
    pub fn iter(&self) -> impl Iterator<Item = (CalibrationComponent, &T)> {
        CalibrationComponent::MASTER_ROLES
            .into_iter()
            .filter_map(|component| self.get(component).map(|value| (component, value)))
    }

    /// The four roles by value, in [`CalibrationComponent::MASTER_ROLES`] order.
    ///
    /// An array rather than an iterator because the concurrent half of
    /// [`CalibrationMasters::from_files`] hands it straight to rayon, which parallelizes `[T; N]`
    /// but not an array iterator. [`Self::from_roles`] is its inverse; the two are the only place
    /// the field-to-role correspondence is written, and `roles_round_trip_in_master_order` pins it.
    pub(crate) fn into_roles(self) -> [(CalibrationComponent, T); 4] {
        [
            (CalibrationComponent::Dark, self.dark),
            (CalibrationComponent::Flat, self.flat),
            (CalibrationComponent::Bias, self.bias),
            (CalibrationComponent::FlatDark, self.flat_dark),
        ]
    }

    /// Rebuild a set from values in [`CalibrationComponent::MASTER_ROLES`] order.
    pub(crate) fn from_roles(roles: [T; 4]) -> Self {
        let [dark, flat, bias, flat_dark] = roles;
        Self {
            dark,
            flat,
            bias,
            flat_dark,
        }
    }

    /// Convert every role, in calibration order, stopping at the first failure.
    pub(crate) fn try_map<U, E>(
        self,
        mut convert: impl FnMut(CalibrationComponent, T) -> Result<U, E>,
    ) -> Result<CalibrationSet<U>, E> {
        let [dark, flat, bias, flat_dark] = self.into_roles();
        Ok(CalibrationSet {
            dark: convert(dark.0, dark.1)?,
            flat: convert(flat.0, flat.1)?,
            bias: convert(bias.0, bias.1)?,
            flat_dark: convert(flat_dark.0, flat_dark.1)?,
        })
    }
}

impl CalibrationSet<Option<CfaImage>> {
    /// The sensor extent every present master shares, or `None` when the set is empty.
    ///
    /// The masters are combined pixel-for-pixel by flat index — dark subtracted from flat,
    /// defects detected on one and corrected on another — so a set that spans two sensors has no
    /// coherent interpretation. Reported as an error rather than left to the individual
    /// operations, which each assert on only the pair they touch and cover the set unevenly: a
    /// bias in a set with no flat is never anyone's operand.
    fn common_dimensions(&self) -> Result<Option<Size2us>, CalibrationError> {
        let mut expected = None;
        for (component, master) in self
            .iter()
            .filter_map(|(component, master)| master.as_ref().map(|master| (component, master)))
        {
            let size = Size2us::new(master.data.width(), master.data.height());
            match expected {
                None => expected = Some(size),
                Some(expected) if expected == size => {}
                Some(expected) => {
                    return Err(CalibrationError::DimensionMismatch {
                        component,
                        expected,
                        master: size,
                    });
                }
            }
        }
        Ok(expected)
    }
}

/// A component present in a [`CalibrationMasters`] bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationComponent {
    /// Master dark frame.
    Dark,
    /// Master flat frame.
    Flat,
    /// Master bias frame.
    Bias,
    /// Master dark frame taken at the flat exposure time.
    FlatDark,
    /// Defect map derived from a dark, flat, or both.
    Defects,
}

impl CalibrationComponent {
    /// The four master-frame roles, in calibration order. [`Self::Defects`] is derived rather
    /// than loaded, so it is not one of them.
    pub const MASTER_ROLES: [Self; 4] = [Self::Dark, Self::Flat, Self::Bias, Self::FlatDark];

    /// The component's `EXTNAME` in a saved bundle — the name a writer stamps on its HDU and a
    /// reader recognizes it by, so the two cannot disagree about where a role lives.
    pub(crate) fn extname(self) -> &'static str {
        match self {
            Self::Dark => "MASTER_DARK",
            Self::Flat => "MASTER_FLAT",
            Self::Bias => "MASTER_BIAS",
            Self::FlatDark => "MASTER_FLAT_DARK",
            Self::Defects => "DEFECT_MAP",
        }
    }

    /// The component an `EXTNAME` names, or `None` for an extension this format does not define.
    pub(crate) fn from_extname(extname: &str) -> Option<Self> {
        [Self::MASTER_ROLES.as_slice(), &[Self::Defects]]
            .concat()
            .into_iter()
            .find(|component| component.extname() == extname)
    }

    /// Whether this role is stored already prepared — bias/flat-dark subtracted, per-colour
    /// normalized and clamped. Only the flat is; the others are stored as stacked.
    pub(crate) fn prepared(self) -> bool {
        matches!(self, Self::Flat)
    }
}

impl std::fmt::Display for CalibrationComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dark => f.write_str("dark"),
            Self::Flat => f.write_str("flat"),
            Self::Bias => f.write_str("bias"),
            Self::FlatDark => f.write_str("flat-dark"),
            Self::Defects => f.write_str("defects"),
        }
    }
}

/// Invalid CFA metadata supplied to raw-frame calibration.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CalibrationError {
    /// The light frame does not identify its sensor pattern.
    #[error("light frame is missing CFA pattern metadata")]
    MissingLightCfaPattern,
    /// A calibration master does not identify its sensor pattern.
    #[error("{component} master is missing CFA pattern metadata")]
    MissingMasterCfaPattern { component: CalibrationComponent },
    /// A calibration master was captured with a different sensor pattern.
    #[error(
        "{component} master CFA pattern {master:?} does not match light frame pattern {light:?}"
    )]
    CfaPatternMismatch {
        component: CalibrationComponent,
        light: CfaType,
        master: CfaType,
    },
    /// A calibration master covers a different sensor area than the frame it has to line up with:
    /// the rest of the bundle when the set is assembled, or the light when one is calibrated.
    #[error("{component} master is {master}, expected {expected}")]
    DimensionMismatch {
        component: CalibrationComponent,
        expected: Size2us,
        master: Size2us,
    },
}

/// Read-only defect statistics derived from a calibration bundle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefectSummary {
    /// Hot pixels detected from the master dark.
    pub hot_pixels: usize,
    /// Cold or dead pixels detected from the master flat.
    pub cold_pixels: usize,
    /// Percentage of sensor pixels present in either class.
    pub percentage: f32,
}

/// Master calibration frames, prepared flat divisor, and derived defect map.
///
/// Construction subtracts the flat's bias/flat-dark, detects cold pixels from that unfloored
/// response, then consumes it into a normalized, clamped divisor. Calibration operates on raw CFA
/// data before demosaicing so defect correction can use same-color neighbors.
#[derive(Debug, Default)]
pub struct CalibrationMasters {
    masters: CalibrationSet<Option<CfaImage>>,
    defect_map: Option<DefectMap>,
}

/// Frame-weighted share of the memory budget for one role when [`CalibrationMasters::from_files`]
/// loads the roles concurrently: proportional to the role's frame count. Two properties matter — the
/// shares sum to at most `available` (flooring, so concurrent loads can't overcommit RAM), and a
/// role fits its share *exactly* when the whole set fits the budget (so no role is pushed to disk
/// while the total fits). An empty role gets nothing; a degenerate `total == 0` gets the whole
/// budget (no divide-by-zero).
fn weighted_budget(available: u64, role_frames: usize, total: usize) -> u64 {
    if total == 0 {
        return available;
    }
    // `available · role_frames` can't overflow u64 for any real RAM size × frame count.
    available * role_frames as u64 / total as u64
}

/// Whether all of `frames`' roles fit in RAM at once — the in-memory-vs-disk decision for the whole
/// set. The per-frame footprint is peeked from one frame's header (all calibration frames share a
/// sensor) without a full decode. No frames, or a peek failure, returns `true`: per-role tiering is
/// memory-safe regardless, so this only governs whether to *optimize* for staying in RAM.
fn frames_fit_in_memory<P: AsRef<Path> + Sync>(
    frames: &CalibrationSet<&[P]>,
    total_frames: usize,
    available: u64,
    context: &LoadContext,
) -> Result<bool, Error> {
    let Some(first) = frames
        .iter()
        .map(|(_, paths)| *paths)
        .find(|paths| !paths.is_empty())
        .map(|paths| &paths[0])
    else {
        return Ok(true);
    };
    match CfaFrameInfo::from_file(first.as_ref(), context) {
        Ok(info) => Ok(memory::fits_in_memory(
            info.dimensions.pixel_count() * std::mem::size_of::<f32>(),
            total_frames,
            available,
        )),
        Err(ImageError::Cancelled { .. }) => Err(Error::Cancelled),
        Err(_) => Ok(true),
    }
}

/// One role's stack waiting to run: the frames, and the preset they combine under.
///
/// The unit [`CalibrationMasters::from_files`] schedules, so the concurrent and sequential paths
/// differ only in which iterator drives them rather than in what each role does.
#[derive(Debug)]
struct RoleStack<'a, P> {
    paths: &'a [P],
    config: StackConfig,
}

impl<P: AsRef<Path> + Sync> RoleStack<'_, P> {
    /// Charge this role its frame-weighted share of `available`, for roles that load concurrently.
    fn with_shared_budget(mut self, available: u64, total_frames: usize) -> Self {
        self.config.cache.available_memory =
            Some(weighted_budget(available, self.paths.len(), total_frames));
        self
    }

    fn run(self, cancel: CancelToken) -> Result<Option<CfaImage>, Error> {
        stack_cfa_master(self.paths, self.config, ProgressCallback::default(), cancel)
    }
}

/// Stack one calibration role's raw CFA frames into a single master, using that
/// role's preset `config` (`StackConfig::dark()` / `flat()` / `bias()`).
///
/// The preset carries its own small-frame fallback (`StackConfig::small_n`): the combine engine
/// downgrades to the median below the preset's `min_frames` (e.g. `flat()` below 8), so no
/// frame-count special-casing is needed here. Returns `None` if `paths` is empty.
///
/// The per-role half of [`CalibrationMasters::from_files`], public so a caller
/// can stack/cache each role independently and assemble the set with
/// [`CalibrationMasters::from_images`].
pub fn stack_cfa_master(
    paths: &[impl AsRef<Path> + Sync],
    config: StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<Option<CfaImage>, Error> {
    // `None` rather than `Error::NoFrames`: an absent calibration role is normal, and this is
    // the one thing `combine_cached` cannot decide for us.
    if paths.is_empty() {
        return Ok(None);
    }

    // A master is mosaic data for the calibration stage to consume, not a science product: the
    // ancillary planes would be allocated and written per pixel for nothing.
    let config = StackConfig {
        quality: QualityPlanes::IMAGE_ONLY,
        ..config
    };
    // `cancel` rides on the cache from construction, so the RAW-decode load loop
    // polls it too (not just the combine).
    let product = combine_cached(&config, paths.len(), "cfa paths", || {
        FrameCache::from_cfa_paths(paths, &config.cache, config.normalization, progress, cancel)
    })?;

    Ok(Some(product.into_cfa_master()))
}

impl CalibrationMasters {
    /// Components present in this bundle, in calibration order.
    pub fn components(&self) -> impl Iterator<Item = CalibrationComponent> {
        self.masters
            .iter()
            .filter(|(_, master)| master.is_some())
            .map(|(component, _)| component)
            .chain(
                self.defect_map
                    .as_ref()
                    .map(|_| CalibrationComponent::Defects),
            )
    }

    /// Defect statistics, or `None` when no dark or flat supplied a defect map.
    pub fn defect_summary(&self) -> Option<DefectSummary> {
        self.defect_map.as_ref().map(|map| DefectSummary {
            hot_pixels: map.hot_indices.len(),
            cold_pixels: map.cold_indices.len(),
            percentage: map.percentage(),
        })
    }

    /// Resident RAM held by this bundle: the present master frames' pixel bytes
    /// plus the defect map's index lists.
    pub fn ram_bytes(&self) -> usize {
        let frame_bytes = self
            .masters
            .iter()
            .filter_map(|(_, master)| master.as_ref())
            .map(CfaImage::ram_bytes)
            .sum::<usize>();
        frame_bytes + self.defect_map.as_ref().map_or(0, DefectMap::ram_bytes)
    }

    /// Save this coherent master bundle as a versioned, checksummed multi-extension FITS file.
    ///
    /// The flat is already bias/flat-dark subtracted, per-color normalized, and clamped in this
    /// representation. Loading the bundle does not repeat flat preparation or defect detection.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        fits::save(path, self)
    }

    /// Load a bundle written by [`Self::save`] without rebuilding its prepared flat or defect map.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        fits::load(path)
    }

    /// Create CalibrationMasters from pre-built CFA images.
    ///
    /// Generates defect map from the CFA dark if provided.
    /// `images.flat_dark` is a dark frame taken at the flat's exposure time — used
    /// instead of bias for flat normalization when provided.
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if cancellation is requested before defect detection completes.
    pub fn from_images(
        images: CalibrationSet<Option<CfaImage>>,
        sigma_threshold: f32,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        // Before any of the work below, all of which combines the masters by flat pixel index and
        // asserts on only the pair it touches.
        images.common_dimensions()?;

        let CalibrationSet {
            dark,
            flat,
            bias,
            flat_dark,
        } = images;
        let flat_subtractor = flat_dark.as_ref().or(bias.as_ref());
        let subtracted_flat = flat.map(|flat| prepared_flat::subtract(flat, flat_subtractor));

        // Hot pixels from the dark, cold/dead pixels from the subtracted flat — None if neither
        // exists. Detection must precede normalization's near-zero floor.
        let defect_map = if dark.is_some() || subtracted_flat.is_some() {
            let mut map = DefectMap::default();
            if let Some(dark) = dark.as_ref() {
                map = map.detect_hot(dark, sigma_threshold, &cancel)?;
            }
            if let Some(flat) = subtracted_flat.as_ref() {
                map = map.detect_cold(flat, &cancel)?;
            }
            Some(map)
        } else {
            None
        };

        let flat = subtracted_flat.map(prepared_flat::normalize);
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        Ok(Self {
            masters: CalibrationSet {
                dark,
                flat,
                bias,
                flat_dark,
            },
            defect_map,
        })
    }

    /// Every present master, and the defect map, describes one sensor.
    ///
    /// `from_images` checks its inputs before it touches them, so this only has to re-check a
    /// bundle assembled some other way — which is `fits::load`, where the masters and the map are
    /// read as independent HDUs. Between the two, no `CalibrationMasters` can exist spanning two
    /// sensors, so `save` cannot write a bundle `load` would reject.
    fn validate_dimensions(&self) -> Result<(), CalibrationError> {
        let expected = self.masters.common_dimensions()?;
        // The map's dimensions come from whichever master it was detected on, so within a bundle
        // built here it always agrees; a file can disagree.
        if let (Some(expected), Some(defects)) = (
            expected,
            self.defect_map.as_ref().and_then(|map| map.dimensions),
        ) && expected != defects
        {
            return Err(CalibrationError::DimensionMismatch {
                component: CalibrationComponent::Defects,
                expected,
                master: defects,
            });
        }
        Ok(())
    }

    /// Create CalibrationMasters by stacking raw CFA files.
    ///
    /// Uses sigma-clipped mean (>= 8 frames) or median (< 8 frames)
    /// with the full stacking pipeline (rejection, normalization, chunked processing).
    /// Empty slices produce `None` for that master.
    ///
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// The roles are independent stacks. When the whole set **fits in RAM** they run
    /// **concurrently** to fill cores left idle by any one role's serial phases (first-frame decode,
    /// stats/reference selection), with the budget split per role (frame-weighted) so the concurrent
    /// loads provably can't overcommit — the per-role peaks sum to at most the usable budget.
    ///
    /// When the set does **not** fit in RAM, splitting the budget would force roles to disk-tier;
    /// running them **sequentially with the full budget each** (freed between roles) keeps every
    /// role that individually fits in RAM, which beats parallel-on-disk. The fit check peeks one
    /// frame's header for the per-frame footprint (all calibration frames share a sensor).
    ///
    /// Returns [`Error::Cancelled`] when cancellation is requested during loading, stacking, or
    /// defect detection.
    pub fn from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let total_frames: usize = frames.iter().map(|(_, paths)| paths.len()).sum();
        // One reading for the whole calibration run: the fit check below and the decode ceiling
        // the context carries are both sized against it, so no role can be tiered against one
        // figure and decoded against another.
        let available = memory::available_memory();
        let context = LoadContext::new(cancel.clone(), memory::memory_budget(available));
        let concurrent = frames_fit_in_memory(&frames, total_frames, available, &context)?;

        // The one role → preset table. Each preset carries its own small-frame fallback
        // (`StackConfig::small_n`), so no frame-count special-casing is needed here.
        let jobs = CalibrationSet {
            dark: RoleStack {
                paths: frames.dark,
                config: StackConfig::dark(),
            },
            flat: RoleStack {
                paths: frames.flat,
                config: StackConfig::flat(),
            },
            bias: RoleStack {
                paths: frames.bias,
                config: StackConfig::bias(),
            },
            // A flat-dark is a dark taken at the flat's exposure time, so it stacks as one.
            flat_dark: RoleStack {
                paths: frames.flat_dark,
                config: StackConfig::dark(),
            },
        };
        // Concurrent roles split the budget by frame count, so their combined in-flight decodes
        // provably cannot overcommit. Run sequentially, each role gets the whole budget instead:
        // its cache frees before the next one loads, so a share would push a role that fits in RAM
        // onto disk for nothing.
        let jobs = jobs.into_roles().map(|(_, job)| {
            if concurrent {
                job.with_shared_budget(available, total_frames)
            } else {
                job
            }
        });

        // Independent stacks on the shared rayon pool — work-stealing interleaves their parallel
        // sections, filling the gaps a single sequential role would leave idle. Four concurrent
        // stacks cannot share one progress callback: each reports its own (current, total) against
        // a different frame count, so the streams would interleave into nonsense. Watch a single
        // role by stacking it with `stack_cfa_master`.
        let masters: Vec<Option<CfaImage>> = if concurrent {
            jobs.into_par_iter()
                .map(|job| job.run(cancel.clone()))
                .collect::<Result<_, Error>>()?
        } else {
            jobs.into_iter()
                .map(|job| job.run(cancel.clone()))
                .collect::<Result<_, Error>>()?
        };

        Self::from_images(
            CalibrationSet::from_roles(
                masters.try_into().expect("four roles in, four masters out"),
            ),
            sigma_threshold,
            cancel,
        )
    }

    /// Calibrate a raw CFA light frame in place.
    ///
    /// Applies calibration on raw (un-demosaiced) data:
    /// 1. Dark subtraction (or bias if no dark)
    /// 2. Flat division with normalization
    /// 3. CFA-aware defect pixel correction
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError`] when the light or any stored master is missing CFA metadata,
    /// or when a master's Mono, Bayer, or X-Trans pattern differs from the light. Validation
    /// completes before the light is mutated.
    pub fn calibrate(&self, image: &mut CfaImage) -> Result<(), CalibrationError> {
        // Double application would silently subtract the dark / divide the flat twice.
        assert!(
            !image.metadata.calibrated,
            "calibrate() called on an already-calibrated frame"
        );
        self.validate_against_light(image)?;
        image.metadata.calibrated = true;

        // 1. Dark subtraction
        if let Some(ref dark) = self.masters.dark {
            image.subtract(dark);
        } else if let Some(ref bias) = self.masters.bias {
            image.subtract(bias);
        }

        // 2. Flat division
        if let Some(ref flat) = self.masters.flat {
            prepared_flat::apply(flat, image);
        }

        // 3. CFA-aware defective pixel correction
        if let Some(ref defect_map) = self.defect_map {
            defect_map.correct(image);
        }

        Ok(())
    }

    /// Every master must describe the same sensor as `image`, in both pattern and extent.
    ///
    /// Extent as well as pattern because the operations below index a master by the light's flat
    /// index: a mismatch is caught here, as an error naming the offending role, rather than as
    /// `CfaImage::subtract`'s assert. Masters are read from user-chosen files, so a set that does
    /// not fit the light is bad input, not a broken invariant.
    fn validate_against_light(&self, image: &CfaImage) -> Result<(), CalibrationError> {
        let light = image
            .metadata
            .cfa_type
            .as_ref()
            .ok_or(CalibrationError::MissingLightCfaPattern)?;
        let light_size = Size2us::new(image.data.width(), image.data.height());

        for (component, master) in self
            .masters
            .iter()
            .filter_map(|(component, master)| master.as_ref().map(|master| (component, master)))
        {
            let master_pattern = master
                .metadata
                .cfa_type
                .as_ref()
                .ok_or(CalibrationError::MissingMasterCfaPattern { component })?;
            if master_pattern != light {
                return Err(CalibrationError::CfaPatternMismatch {
                    component,
                    light: light.clone(),
                    master: master_pattern.clone(),
                });
            }
            let master_size = Size2us::new(master.data.width(), master.data.height());
            if master_size != light_size {
                return Err(CalibrationError::DimensionMismatch {
                    component,
                    expected: light_size,
                    master: master_size,
                });
            }
        }

        // The defect map indexes the light by flat index too, and it is the one component that
        // can be present without a master of its own to have been checked above.
        if let Some(defects) = self.defect_map.as_ref().and_then(|map| map.dimensions)
            && defects != light_size
        {
            return Err(CalibrationError::DimensionMismatch {
                component: CalibrationComponent::Defects,
                expected: light_size,
                master: defects,
            });
        }

        Ok(())
    }
}
