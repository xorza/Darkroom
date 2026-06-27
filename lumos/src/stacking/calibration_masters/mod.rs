//! Calibration master frame creation and management.

pub(crate) mod cosmic_ray;
pub mod defect_map;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod real_data_tests;
#[cfg(test)]
mod synthetic_tests;
#[cfg(test)]
mod tests;

use std::path::Path;

use common::CancelToken;

use crate::io::astro_image::cfa::CfaImage;
use crate::stacking::combine::cache::CfaCache;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::combine::stack::run_stacking;
use defect_map::DefectMap;

/// Default sigma threshold for defect detection.
///
/// A pixel is flagged as defective if it deviates from the per-color median
/// by more than `sigma_threshold × σ` (where σ is estimated from MAD).
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_SIGMA_THRESHOLD: f32 = 5.0;

/// Raw frame paths for [`CalibrationMasters::from_files`], grouped by role.
///
/// Naming each role at the call site prevents accidentally swapping, say, flats
/// and bias (all four are path slices). An empty slice produces `None` for that
/// master.
#[derive(Debug)]
pub struct CalibrationFrames<'a, P: AsRef<Path> + Sync> {
    /// Dark frames (thermal-noise calibration).
    pub darks: &'a [P],
    /// Flat frames (vignetting / dust correction).
    pub flats: &'a [P],
    /// Bias frames (read-noise calibration).
    pub bias: &'a [P],
    /// Dark frames taken at the flat exposure time. When non-empty, subtracted
    /// from the flat instead of bias during flat normalization — matters for
    /// narrowband, where flat exposures accumulate dark current.
    pub flat_darks: &'a [P],
}

/// Pre-stacked master frames for [`CalibrationMasters::from_images`], grouped by role.
///
/// Like [`CalibrationFrames`] but for already-stacked CFA images. Naming each
/// role at the call site prevents swapping them (all four are `Option<CfaImage>`).
/// `None` omits that master.
#[derive(Debug, Default)]
pub struct CalibrationImages {
    /// Master dark frame (raw CFA).
    pub dark: Option<CfaImage>,
    /// Master flat frame (raw CFA).
    pub flat: Option<CfaImage>,
    /// Master bias frame (raw CFA).
    pub bias: Option<CfaImage>,
    /// Master flat-dark frame (dark taken at the flat exposure time).
    pub flat_dark: Option<CfaImage>,
}

/// Holds master calibration frames (dark, flat, bias) and defect map.
///
/// Operates on raw CFA (single-channel sensor) data before demosaicing,
/// giving mathematically correct defect pixel correction using same-color
/// CFA neighbors.
#[derive(Debug)]
pub struct CalibrationMasters {
    /// Master dark frame (raw CFA)
    pub master_dark: Option<CfaImage>,
    /// Master flat frame (raw CFA)
    pub master_flat: Option<CfaImage>,
    /// Master bias frame (raw CFA)
    pub master_bias: Option<CfaImage>,
    /// Master flat dark frame (dark taken at flat exposure time)
    pub master_flat_dark: Option<CfaImage>,
    /// Defect map (hot + cold pixels) derived from master dark
    pub defect_map: Option<DefectMap>,
}

/// Per-role share of the memory budget when [`CalibrationMasters::from_files`] loads the roles
/// concurrently: an even split across the `active` (non-empty) roles. Flooring guarantees the
/// shares sum to at most `available`, so the concurrent loads can't overcommit RAM. A lone role
/// (or `active == 0`) gets the whole budget.
fn role_memory_budget(available: u64, active: usize) -> u64 {
    available / active.max(1) as u64
}

/// Stack one calibration role's raw CFA frames into a single master, using that
/// role's preset `config` (`StackConfig::dark()` / `flat()` / `bias()`).
///
/// Uses the frame-type preset config. Falls back to median for < 8 frames
/// (too few for rejection to work well). This is a stricter, all-methods
/// *quality* threshold for master frames, sitting above the library's
/// per-method *correctness* floor (`stacking::MIN_FRAMES_FOR_REJECTION`, which
/// only overrides genuinely-unsound σ-rejection at N ≤ 4); the two compose —
/// a `< 8` master is already median here, so the library floor is a no-op.
/// Returns `None` if `paths` is empty.
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

    let config = if paths.len() < 8 {
        StackConfig {
            normalization: config.normalization,
            ..StackConfig::median()
        }
    } else {
        config
    };

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
    /// Create CalibrationMasters from pre-built CFA images.
    ///
    /// Generates defect map from the CFA dark if provided.
    /// `images.flat_dark` is a dark frame taken at the flat's exposure time — used
    /// instead of bias for flat normalization when provided.
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    pub fn from_images(
        images: CalibrationImages,
        sigma_threshold: f32,
        cancel: CancelToken,
    ) -> Self {
        let CalibrationImages {
            dark,
            flat,
            bias,
            flat_dark,
        } = images;

        // Hot pixels from the dark, cold/dead pixels from the flat — None if we have neither.
        // Both detections poll `cancel` per pixel (the defect-map scan dominates a cached-master
        // build, so a cancel must bail it mid-scan).
        let defect_map = (dark.is_some() || flat.is_some()).then(|| {
            let mut map = DefectMap::default();
            if let Some(dark) = dark.as_ref() {
                map = map.detect_hot(dark, sigma_threshold, &cancel);
            }
            if let Some(flat) = flat.as_ref() {
                map = map.detect_cold(flat, &cancel);
            }
            map
        });

        Self {
            master_dark: dark,
            master_flat: flat,
            master_bias: bias,
            master_flat_dark: flat_dark,
            defect_map,
        }
    }

    /// Create CalibrationMasters by stacking raw CFA files.
    ///
    /// Uses sigma-clipped mean (>= 8 frames) or median (< 8 frames)
    /// with the full stacking pipeline (rejection, normalization, chunked processing).
    /// Empty slices produce `None` for that master (see [`CalibrationFrames`]).
    ///
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// The roles are independent stacks, so they run **concurrently** to fill cores left idle by
    /// any one role's serial phases (first-frame decode, stats/reference selection). To stay
    /// memory-safe, the RAM budget is split evenly across the non-empty roles: each role's tiering
    /// and decode concurrency size against its share, so the concurrent loads provably can't
    /// collectively overcommit (the per-role peaks sum to at most the whole usable budget).
    pub fn from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationFrames<'_, P>,
        sigma_threshold: f32,
    ) -> Result<Self, Error> {
        let mut dark_cfg = StackConfig::dark();
        let mut flat_cfg = StackConfig::flat();
        let mut bias_cfg = StackConfig::bias();
        let mut flat_dark_cfg = StackConfig::dark();

        // Split the live RAM budget across the non-empty roles (empty ones do no work and return
        // `None`). Querying once and dividing — rather than letting each concurrent role query the
        // live free memory and each see the whole of it — is what keeps the split honest.
        let active = [frames.darks, frames.flats, frames.bias, frames.flat_darks]
            .iter()
            .filter(|paths| !paths.is_empty())
            .count();
        let budget = role_memory_budget(dark_cfg.cache.get_available_memory(), active);
        for cfg in [
            &mut dark_cfg,
            &mut flat_cfg,
            &mut bias_cfg,
            &mut flat_dark_cfg,
        ] {
            cfg.cache.available_memory = Some(budget);
        }

        // Independent stacks on the shared rayon pool — work-stealing interleaves their parallel
        // sections, filling the gaps a single sequential role would leave idle.
        let ((dark, flat), (bias, flat_dark)) = rayon::join(
            move || {
                rayon::join(
                    move || stack_cfa_master(frames.darks, dark_cfg, CancelToken::never()),
                    move || stack_cfa_master(frames.flats, flat_cfg, CancelToken::never()),
                )
            },
            move || {
                rayon::join(
                    move || stack_cfa_master(frames.bias, bias_cfg, CancelToken::never()),
                    move || {
                        stack_cfa_master(frames.flat_darks, flat_dark_cfg, CancelToken::never())
                    },
                )
            },
        );

        Ok(Self::from_images(
            CalibrationImages {
                dark: dark?,
                flat: flat?,
                bias: bias?,
                flat_dark: flat_dark?,
            },
            sigma_threshold,
            CancelToken::never(),
        ))
    }

    /// Calibrate a raw CFA light frame in place.
    ///
    /// Applies calibration on raw (un-demosaiced) data:
    /// 1. Dark subtraction (or bias if no dark)
    /// 2. Flat division with normalization
    /// 3. CFA-aware defect pixel correction
    pub fn calibrate(&self, image: &mut CfaImage) {
        // Double application would silently subtract the dark / divide the flat twice.
        assert!(
            !image.metadata.calibrated,
            "calibrate() called on an already-calibrated frame"
        );
        image.metadata.calibrated = true;

        // 1. Dark subtraction
        if let Some(ref dark) = self.master_dark {
            image.subtract(dark);
        } else if let Some(ref bias) = self.master_bias {
            image.subtract(bias);
        }

        // 2. Flat division (flat dark takes priority over bias for flat normalization)
        if let Some(ref flat) = self.master_flat {
            let flat_sub = self.master_flat_dark.as_ref().or(self.master_bias.as_ref());
            image.divide_by_normalized(flat, flat_sub);
        }

        // 3. CFA-aware defective pixel correction
        if let Some(ref defect_map) = self.defect_map {
            defect_map.correct(image);
        }
    }
}
