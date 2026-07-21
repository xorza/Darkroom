//! Calibration master frame creation and management.

pub(crate) mod cosmic_ray;
pub(crate) mod defect_map;
mod fits;
mod prepared_flat;

#[cfg(test)]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod synthetic_tests;
#[cfg(test)]
mod tests;

use std::path::Path;

use common::CancelToken;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType};
use crate::stacking::combine::cache::CfaCache;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::stack::run_stacking;
use crate::stacking::frame_store::fits_in_memory;
use crate::stacking::progress::ProgressCallback;
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
    dark: Option<CfaImage>,
    flat: Option<CfaImage>,
    bias: Option<CfaImage>,
    flat_dark: Option<CfaImage>,
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
) -> bool {
    let Some(first) = [frames.dark, frames.flat, frames.bias, frames.flat_dark]
        .into_iter()
        .find(|paths| !paths.is_empty())
        .map(|paths| &paths[0])
    else {
        return true;
    };
    match CfaFrameInfo::from_file(first.as_ref()) {
        Ok(info) => fits_in_memory(
            info.dimensions.pixel_count() * std::mem::size_of::<f32>(),
            total_frames,
            available,
        ),
        Err(_) => true,
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
    cancel: CancelToken,
) -> Result<Option<CfaImage>, Error> {
    if paths.is_empty() {
        return Ok(None);
    }

    // `cancel` rides on the cache from construction, so the RAW-decode load loop
    // polls it too (not just the combine).
    let cache = CfaCache::from_paths(paths, &config.cache, ProgressCallback::default(), cancel)?;

    let result = run_stacking(&cache, &config);
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }

    Ok(Some(result))
}

impl CalibrationMasters {
    /// Components present in this bundle, in calibration order.
    pub fn components(&self) -> impl Iterator<Item = CalibrationComponent> {
        [
            self.dark.as_ref().map(|_| CalibrationComponent::Dark),
            self.flat.as_ref().map(|_| CalibrationComponent::Flat),
            self.bias.as_ref().map(|_| CalibrationComponent::Bias),
            self.flat_dark
                .as_ref()
                .map(|_| CalibrationComponent::FlatDark),
            self.defect_map
                .as_ref()
                .map(|_| CalibrationComponent::Defects),
        ]
        .into_iter()
        .flatten()
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
        let frame_bytes = [&self.dark, &self.flat, &self.bias, &self.flat_dark]
            .into_iter()
            .filter_map(|master| master.as_ref())
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
            dark,
            flat,
            bias,
            flat_dark,
            defect_map,
        })
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
    pub fn from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
    ) -> Result<Self, Error> {
        let counts = [
            frames.dark.len(),
            frames.flat.len(),
            frames.bias.len(),
            frames.flat_dark.len(),
        ];
        let total_frames: usize = counts.iter().sum();
        let available = StackConfig::dark().cache.get_available_memory();

        let (dark, flat, bias, flat_dark) =
            if frames_fit_in_memory(&frames, total_frames, available) {
                // Concurrent: frame-weighted budget per role keeps each in RAM (the whole set fits)
                // while bounding the combined in-flight decode footprint.
                let mut dark_cfg = StackConfig::dark();
                let mut flat_cfg = StackConfig::flat();
                let mut bias_cfg = StackConfig::bias();
                let mut flat_dark_cfg = StackConfig::dark();
                dark_cfg.cache.available_memory =
                    Some(weighted_budget(available, counts[0], total_frames));
                flat_cfg.cache.available_memory =
                    Some(weighted_budget(available, counts[1], total_frames));
                bias_cfg.cache.available_memory =
                    Some(weighted_budget(available, counts[2], total_frames));
                flat_dark_cfg.cache.available_memory =
                    Some(weighted_budget(available, counts[3], total_frames));

                // Independent stacks on the shared rayon pool — work-stealing interleaves their
                // parallel sections, filling the gaps a single sequential role would leave idle.
                let ((dark, flat), (bias, flat_dark)) = rayon::join(
                    move || {
                        rayon::join(
                            move || stack_cfa_master(frames.dark, dark_cfg, CancelToken::never()),
                            move || stack_cfa_master(frames.flat, flat_cfg, CancelToken::never()),
                        )
                    },
                    move || {
                        rayon::join(
                            move || stack_cfa_master(frames.bias, bias_cfg, CancelToken::never()),
                            move || {
                                stack_cfa_master(
                                    frames.flat_dark,
                                    flat_dark_cfg,
                                    CancelToken::never(),
                                )
                            },
                        )
                    },
                );
                (dark?, flat?, bias?, flat_dark?)
            } else {
                // Sequential, full budget each (each role's cache frees before the next loads), so a
                // role that fits the whole budget stays in RAM instead of being forced to disk.
                (
                    stack_cfa_master(frames.dark, StackConfig::dark(), CancelToken::never())?,
                    stack_cfa_master(frames.flat, StackConfig::flat(), CancelToken::never())?,
                    stack_cfa_master(frames.bias, StackConfig::bias(), CancelToken::never())?,
                    stack_cfa_master(frames.flat_dark, StackConfig::dark(), CancelToken::never())?,
                )
            };

        Self::from_images(
            CalibrationSet {
                dark,
                flat,
                bias,
                flat_dark,
            },
            sigma_threshold,
            CancelToken::never(),
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
        self.validate_cfa_patterns(image)?;
        image.metadata.calibrated = true;

        // 1. Dark subtraction
        if let Some(ref dark) = self.dark {
            image.subtract(dark);
        } else if let Some(ref bias) = self.bias {
            image.subtract(bias);
        }

        // 2. Flat division
        if let Some(ref flat) = self.flat {
            prepared_flat::apply(flat, image);
        }

        // 3. CFA-aware defective pixel correction
        if let Some(ref defect_map) = self.defect_map {
            defect_map.correct(image);
        }

        Ok(())
    }

    fn validate_cfa_patterns(&self, image: &CfaImage) -> Result<(), CalibrationError> {
        let light = image
            .metadata
            .cfa_type
            .as_ref()
            .ok_or(CalibrationError::MissingLightCfaPattern)?;

        for (component, metadata) in [
            (
                CalibrationComponent::Dark,
                self.dark.as_ref().map(|master| &master.metadata),
            ),
            (
                CalibrationComponent::Flat,
                self.flat.as_ref().map(|master| &master.metadata),
            ),
            (
                CalibrationComponent::Bias,
                self.bias.as_ref().map(|master| &master.metadata),
            ),
            (
                CalibrationComponent::FlatDark,
                self.flat_dark.as_ref().map(|master| &master.metadata),
            ),
        ] {
            let Some(metadata) = metadata else {
                continue;
            };
            let master_pattern = metadata
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
        }

        Ok(())
    }
}
