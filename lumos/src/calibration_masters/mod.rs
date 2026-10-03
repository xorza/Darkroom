//! Calibration master frame creation and management.

pub(crate) mod calibration_component;
pub(crate) mod calibration_set;
pub(crate) mod cosmic_ray;
pub(crate) mod defect_map;
pub(crate) mod error;
mod fits;
pub(crate) mod master_role;
mod prepared_flat;

use std::io;
use std::path::Path;

use common::CancelToken;

use crate::calibration_masters::defect_map::DefectMap;
use crate::calibration_masters::error::CalibrationError;
use crate::combine::cache::FrameCache;
use crate::combine::config::StackConfig;
use crate::combine::error::Error;
use crate::combine::stack::combine_cached;
use crate::io::image::cfa::CfaImage;
use crate::math::size2us::Size2us;
use crate::memory::run_memory::RunMemory;
use crate::progress::ProgressCallback;
use crate::stack_product::quality_planes::QualityPlanes;

use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::calibration_set::CalibrationSet;
use crate::calibration_masters::master_role::MasterRole;
/// Default sigma threshold for defect detection.
///
/// A pixel is flagged as defective if it exceeds the per-color residual median by more than
/// `sigma_threshold × σ`, where robust bulk statistics and source resolution determine σ.
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_SIGMA_THRESHOLD: f32 = 5.0;

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

/// Stack one calibration role's raw CFA frames into a single master, under `config` — the
/// role's preset is [`MasterRole::stack_config`]. Returns `None` if `paths` is empty.
///
/// The preset carries its own small-frame fallback (`StackConfig::small_n`): the combine engine
/// downgrades to the median below the preset's `min_frames` (e.g. `flat()` below 8), so no
/// frame-count special-casing is needed here. Stack each role this way, then assemble the set
/// with [`CalibrationMasters::from_images`].
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
    let memory = RunMemory::read(config.cache.memory_override);
    // A master is mosaic data for the calibration stage to consume, not a science product: the
    // ancillary planes would be allocated and written per pixel for nothing.
    let config = StackConfig {
        quality: QualityPlanes::IMAGE_ONLY,
        ..config
    };
    // `cancel` rides on the cache from construction, so the RAW-decode load loop
    // polls it too (not just the combine).
    let product = combine_cached(&config, paths.len(), "cfa paths", || {
        FrameCache::from_cfa_paths(paths, &config, memory, progress, cancel)
    })?;

    Ok(Some(product.into_cfa_master()))
}

impl CalibrationMasters {
    /// Components present in this bundle, in calibration order.
    pub fn components(&self) -> impl Iterator<Item = CalibrationComponent> {
        self.masters
            .iter()
            .filter(|(_, master)| master.is_some())
            .map(|(role, _)| CalibrationComponent::Master(role))
            .chain(
                self.defect_map
                    .as_ref()
                    .map(|_| CalibrationComponent::Defects),
            )
    }

    /// Defect statistics, or `None` when no dark or flat supplied a defect map.
    pub fn defect_summary(&self) -> Option<DefectSummary> {
        self.defect_map.as_ref().map(|map| DefectSummary {
            hot_pixels: map.hot_indices().len(),
            cold_pixels: map.cold_indices().len(),
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
    pub fn save(&self, path: &Path) -> io::Result<()> {
        fits::save(path, self)
    }

    /// Load a bundle written by [`Self::save`] without rebuilding its prepared flat or defect map.
    pub fn load(path: &Path) -> io::Result<Self> {
        fits::load(path)
    }

    /// Create `CalibrationMasters` from pre-built CFA images.
    ///
    /// Generates defect map from the CFA dark if provided.
    /// `images.flat_dark` is a dark frame taken at the flat's exposure time — used
    /// instead of bias for flat normalization when provided.
    /// `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// # Errors
    ///
    /// A [`CalibrationError`] when the masters do not describe one sensor or the flat has no
    /// positive mean to normalize by, and [`CalibrationError::Cancelled`] if cancellation is
    /// requested before defect detection completes.
    pub fn from_images(
        images: CalibrationSet<Option<CfaImage>>,
        sigma_threshold: f32,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }
        // Before any of the work below, all of which combines the masters by flat pixel index and
        // asserts on only the pair it touches.
        let dimensions = images.common_dimensions()?;

        let CalibrationSet {
            dark,
            flat,
            bias,
            flat_dark,
        } = images;
        let flat_subtractor = match (&flat, &flat_dark, &bias) {
            (Some(flat), Some(subtractor), _) => Some((
                subtractor,
                master_scale(flat, subtractor, MasterRole::FlatDark)?,
            )),
            (Some(flat), None, Some(subtractor)) => Some((
                subtractor,
                master_scale(flat, subtractor, MasterRole::Bias)?,
            )),
            _ => None,
        };
        let subtracted_flat = flat.map(|flat| prepared_flat::subtract(flat, flat_subtractor));

        // Hot pixels from the dark, cold/dead pixels from the subtracted flat — None if neither
        // exists. Detection must precede normalization's near-zero floor.
        let defect_map = if dark.is_some() || subtracted_flat.is_some() {
            let mut map =
                DefectMap::new(dimensions.expect("a present master gives the set its dimensions"));
            if let Some(dark) = dark.as_ref() {
                map = map.detect_hot(dark, sigma_threshold, cancel)?;
            }
            if let Some(flat) = subtracted_flat.as_ref() {
                map = map.detect_cold(flat, cancel)?;
            }
            Some(map)
        } else {
            None
        };

        let flat = subtracted_flat.map(prepared_flat::normalize).transpose()?;
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
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
            self.defect_map.as_ref().map(DefectMap::dimensions),
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
        // Double application would subtract the dark and divide the flat twice. The flag comes
        // from the file (`LUMCAL`), so this is input to refuse, not an invariant to assert.
        if image.metadata.calibrated {
            return Err(CalibrationError::AlreadyCalibrated);
        }
        self.validate_against_light(image)?;
        // 1. Dark subtraction (or bias), in the light's own domain.
        let subtracted = match (&self.masters.dark, &self.masters.bias) {
            (Some(dark), _) => Some((dark, MasterRole::Dark)),
            (None, Some(bias)) => Some((bias, MasterRole::Bias)),
            (None, None) => None,
        };
        let subtracted = subtracted
            .map(|(master, role)| master_scale(image, master, role).map(|scale| (master, scale)))
            .transpose()?;
        image.metadata.calibrated = true;
        if let Some((master, scale)) = subtracted {
            image.subtract(master, scale);
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
        let light = image.cfa_type;
        let light_size = Size2us::new(image.data.width(), image.data.height());
        let light_domain = image.metadata.sample_domain();

        for (role, master) in self
            .masters
            .iter()
            .filter_map(|(role, master)| master.as_ref().map(|master| (role, master)))
        {
            if master.cfa_type != light {
                return Err(CalibrationError::CfaPatternMismatch {
                    component: role,
                    expected: light,
                    master: master.cfa_type,
                });
            }
            // The flat divides a normalized copy of itself, so its own scale cancels; only a
            // different stated unit disqualifies it. The subtracted master's scale is checked where
            // it is converted (`master_scale`). Only when both declare a domain: a synthesized
            // master has none, and refusing on that would reject every in-memory fixture.
            if role == MasterRole::Flat
                && let (Some(light_domain), Some(master_domain)) =
                    (&light_domain, master.metadata.sample_domain())
                && !master_domain.units_agree(light_domain)
            {
                return Err(CalibrationError::SampleDomainMismatch {
                    component: role,
                    frame: light_domain.clone(),
                    master: master_domain,
                });
            }
            let master_size = Size2us::new(master.data.width(), master.data.height());
            if master_size != light_size {
                return Err(CalibrationError::DimensionMismatch {
                    component: role.into(),
                    expected: light_size,
                    master: master_size,
                });
            }
        }

        // The defect map indexes the light by flat index too, and it is the one component that
        // can be present without a master of its own to have been checked above.
        if let Some(defects) = self.defect_map.as_ref().map(DefectMap::dimensions)
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

/// The factor that expresses `master`'s samples in `frame`'s domain, for the master in `role`.
///
/// `1.0` when either declares no domain: a synthesized frame has none, and there is nothing to
/// convert between. An error when the two cannot be related exactly — a different stated unit, or
/// a span the decoder had to assume — because subtracting across such a gap silently does nothing.
fn master_scale(
    frame: &CfaImage,
    master: &CfaImage,
    role: MasterRole,
) -> Result<f32, CalibrationError> {
    let (Some(frame_domain), Some(master_domain)) = (
        frame.metadata.sample_domain(),
        master.metadata.sample_domain(),
    ) else {
        return Ok(1.0);
    };
    master_domain
        .conversion_to(&frame_domain)
        .ok_or(CalibrationError::SampleDomainMismatch {
            component: role,
            frame: frame_domain,
            master: master_domain,
        })
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use common::CancelToken;

    use crate::calibration_masters::calibration_set::CalibrationSet;

    use crate::calibration_masters::master_role::MasterRole;

    use crate::calibration_masters::{CalibrationMasters, stack_cfa_master};
    use crate::progress::ProgressCallback;

    /// Every role stacked under its preset, then the set assembled — what a caller does role by
    /// role.
    pub(crate) fn masters_from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
    ) -> CalibrationMasters {
        let stack = |paths: &[P], role: MasterRole| {
            stack_cfa_master(
                paths,
                role.stack_config(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .expect("stack a calibration master")
        };
        CalibrationMasters::from_images(
            CalibrationSet {
                dark: stack(frames.dark, MasterRole::Dark),
                flat: stack(frames.flat, MasterRole::Flat),
                bias: stack(frames.bias, MasterRole::Bias),
                flat_dark: stack(frames.flat_dark, MasterRole::FlatDark),
            },
            sigma_threshold,
            &CancelToken::never(),
        )
        .expect("assemble calibration masters")
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
