//! Calibration master frame creation and management.

pub(crate) mod calibration_component;
pub(crate) mod calibration_outcome;
pub(crate) mod calibration_set;
pub(crate) mod cosmic_ray;
pub(crate) mod defect_map;
pub(crate) mod error;
mod fits;
pub(crate) mod master_dark;
pub(crate) mod master_role;
pub(crate) mod master_subtraction;
pub(crate) mod prepared_flat;

use std::io;
use std::path::Path;

use common::CancelToken;

use crate::calibration_masters::defect_map::DefectMap;
use crate::calibration_masters::error::CalibrationError;
use crate::calibration_masters::master_subtraction::MasterSubtraction;
use crate::combine::cache::FrameCache;
use crate::combine::config::StackConfig;
use crate::combine::error::Error;
use crate::combine::stack::combine_cached;
use crate::ingest::frame_step::FrameStep;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::cfa::CfaImage;
use crate::io::image::sample_domain::{DomainMap, Pedestal};
use crate::math::size2us::Size2us;
use crate::progress::ProgressCallback;
use crate::stack_product::quality_planes::QualityPlanes;

use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::calibration_masters::calibration_set::CalibrationSet;
use crate::calibration_masters::master_dark::{DarkBias, MasterDark};
use crate::calibration_masters::master_role::MasterRole;
use crate::calibration_masters::prepared_flat::PreparedFlat;
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

/// The calibration a light receives: what to subtract, the flat to divide by, and the defects to
/// repair. Only what [`Self::calibrate`] reads is kept; the flat-dark is spent on the flat.
///
/// Construction removes the flat's additive part if its stack did not, detects cold pixels from
/// that unfloored response, then consumes it into a normalized, floored divisor. With a bias beside
/// the dark, the dark keeps its thermal signal alone, which scales with exposure. Calibration
/// operates on raw CFA data before demosaicing so defect correction can use same-color neighbors.
#[derive(Debug, Default)]
pub struct CalibrationMasters {
    bias: Option<CfaImage>,
    dark: Option<MasterDark>,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
}

/// Exposures within this share of each other count as one: capture software times frames to a few
/// milliseconds, far inside it, and a dark that matters, such as 120 s on 300 s lights, is far
/// outside.
const EXPOSURE_TOLERANCE: f64 = 0.01;

/// Sensor temperatures within this many degrees count as one: a regulated cooler holds its set
/// point to a few tenths of a degree, and dark current changes by about 12% per degree.
const TEMPERATURE_TOLERANCE: f64 = 1.0;

/// Stack one calibration role's raw CFA frames into a single master, under `config` — the
/// role's preset is [`MasterRole::stack_config`]. Returns `None` if `paths` is empty.
///
/// `subtract`, when given, is taken from every frame before its statistics and the combine, and the
/// master is marked calibrated: flats take their flat-dark or bias this way, so the multiplicative
/// normalization scales each flat's own signal. Scaling a flat that still holds its offset `b` and
/// subtracting the offset from the master afterwards leaves `b·(mean gain − 1)`, a vignetting
/// residual of a few percent, as PixInsight and Siril avoid by calibrating each flat first.
///
/// The preset carries its own small-frame fallback (`StackConfig::small_n`): the combine engine
/// downgrades to the median below the preset's `min_frames` (e.g. `flat()` below 8), so no
/// frame-count special-casing is needed here. Stack each role this way, then assemble the set
/// with [`CalibrationMasters::from_images`].
pub fn stack_cfa_master(
    paths: &[impl AsRef<Path> + Sync],
    config: StackConfig,
    subtract: Option<&CfaImage>,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<Option<CfaImage>, Error> {
    // `None` rather than `Error::NoFrames`: an absent calibration role is normal, and this is
    // the one thing `combine_cached` cannot decide for us.
    if paths.is_empty() {
        return Ok(None);
    }
    let run = IngestRun::new(&config.ingest, cancel);
    // A master is mosaic data for the calibration stage to consume, not a science product: the
    // ancillary planes would be allocated and written per pixel for nothing.
    let config = StackConfig {
        quality: QualityPlanes::IMAGE_ONLY,
        ..config
    };
    let subtraction = subtract.map(|master| MasterSubtraction { master });
    let step = subtraction
        .as_ref()
        .map(|step| step as &dyn FrameStep<CfaImage>);
    let product = combine_cached(&config, paths.len(), "cfa paths", || {
        FrameCache::from_cfa_paths(paths, &config, run, step, progress)
    })?;

    Ok(Some(product.into_cfa_master()))
}

impl CalibrationMasters {
    /// The present masters, with their roles, in calibration order.
    fn masters(&self) -> impl Iterator<Item = (MasterRole, &CfaImage)> {
        [
            (MasterRole::Dark, self.dark.as_ref().map(|dark| &dark.image)),
            (
                MasterRole::Flat,
                self.flat.as_ref().map(PreparedFlat::divisor),
            ),
            (MasterRole::Bias, self.bias.as_ref()),
        ]
        .into_iter()
        .filter_map(|(role, master)| master.map(|master| (role, master)))
    }

    /// Components present in this bundle, in calibration order.
    pub fn components(&self) -> impl Iterator<Item = CalibrationComponent> {
        self.masters()
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

    /// Photosites the flat's floor raised: every light is corrected by less than its vignetting
    /// asks there.
    pub fn floored_flat_pixels(&self) -> usize {
        self.flat.as_ref().map_or(0, PreparedFlat::floored)
    }

    /// Resident RAM held by this bundle: the present master frames' pixel bytes
    /// plus the defect map's index lists.
    pub fn ram_bytes(&self) -> usize {
        let frame_bytes = self
            .masters()
            .map(|(_, master)| master.ram_bytes())
            .sum::<usize>();
        frame_bytes + self.defect_map.as_ref().map_or(0, DefectMap::ram_bytes)
    }

    /// Save this coherent master bundle as a versioned, checksummed multi-extension FITS file.
    ///
    /// The flat is stored prepared and the dark with its bias state. Loading the bundle does not
    /// repeat flat preparation or defect detection.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        fits::save(path, self)
    }

    /// Load a bundle written by [`Self::save`] without rebuilding its prepared flat or defect map.
    pub fn load(path: &Path) -> io::Result<Self> {
        fits::load(path)
    }

    /// Create `CalibrationMasters` from pre-built CFA images.
    ///
    /// The flat's additive part is removed here only when its stack did not remove it per frame
    /// ([`stack_cfa_master`] with a subtractor marks it calibrated): by the flat-dark, else the
    /// bias. A flat that still holds an offset with neither is refused. Cold pixels are detected on
    /// that flat, hot pixels on the dark; with a bias, the dark keeps its thermal signal alone.
    /// `sigma_threshold` controls defect detection sensitivity (see [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// # Errors
    ///
    /// A [`CalibrationError`] when the masters do not describe one sensor, the flat has no positive
    /// mean to normalize by or holds an offset nothing removes, and [`CalibrationError::Cancelled`]
    /// if cancellation is requested before defect detection completes.
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
        let subtracted_flat = flat
            .map(|mut flat| {
                if flat.metadata.calibrated {
                    return Ok(flat);
                }
                let subtractor = match (&flat_dark, &bias) {
                    (Some(subtractor), _) => Some((subtractor, MasterRole::FlatDark)),
                    (None, Some(subtractor)) => Some((subtractor, MasterRole::Bias)),
                    (None, None) => None,
                };
                match subtractor {
                    Some((subtractor, role)) => {
                        let map = master_scale(&flat, subtractor, role)?;
                        flat.subtract(subtractor, map);
                    }
                    None if holds_offset(&flat) => {
                        return Err(CalibrationError::FlatWithoutSubtractor);
                    }
                    None => {}
                }
                Ok(flat)
            })
            .transpose()?;

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

        let dark = dark
            .map(|mut dark| -> Result<MasterDark, CalibrationError> {
                let Some(bias) = &bias else {
                    return Ok(MasterDark {
                        image: dark,
                        bias: DarkBias::Included,
                    });
                };
                let map = master_scale(&dark, bias, MasterRole::Bias)?;
                dark.subtract(bias, map);
                Ok(MasterDark {
                    image: dark,
                    bias: DarkBias::Removed,
                })
            })
            .transpose()?;
        let flat = subtracted_flat.map(PreparedFlat::new).transpose()?;
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }

        Ok(Self {
            bias,
            dark,
            flat,
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
        let mut masters = self.masters();
        let Some((_, first)) = masters.next() else {
            return Ok(());
        };
        let expected = first.size();
        for (role, master) in masters {
            if master.cfa_type != first.cfa_type {
                return Err(CalibrationError::CfaPatternMismatch {
                    component: role,
                    expected: first.cfa_type,
                    master: master.cfa_type,
                });
            }
            if master.size() != expected {
                return Err(CalibrationError::DimensionMismatch {
                    component: role.into(),
                    expected,
                    master: master.size(),
                });
            }
        }
        // The map's dimensions come from whichever master it was detected on, so within a bundle
        // built here it always agrees; a file can disagree.
        if let Some(defects) = self.defect_map.as_ref().map(DefectMap::dimensions)
            && defects != expected
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
    /// 1. The bias and the dark, the dark matched to the light: exposure within 1%, temperature
    ///    within 1 °C. A bias-removed dark of another exposure is scaled by the light's exposure
    ///    over its own; one that holds the bias is refused. A fact one side does not declare is
    ///    not compared, and the outcome says so.
    /// 2. Flat division with normalization
    /// 3. CFA-aware defect pixel correction
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError`] when the light or any stored master is missing CFA metadata, a
    /// master's pattern differs from the light, the dark does not match it, or a flat would divide
    /// a light that holds an offset nothing subtracts. Validation completes before the light is
    /// mutated.
    pub fn calibrate(&self, image: &mut CfaImage) -> Result<CalibrationOutcome, CalibrationError> {
        // Double application would subtract the dark and divide the flat twice. The flag comes
        // from the file (`LUMCAL`), so this is input to refuse, not an invariant to assert.
        if image.metadata.calibrated {
            return Err(CalibrationError::AlreadyCalibrated);
        }
        self.validate_against_light(image)?;
        let mut outcome = CalibrationOutcome::default();
        let dark = self
            .dark
            .as_ref()
            .map(|dark| -> Result<_, CalibrationError> {
                let scale = self.dark_scale(dark, image, &mut outcome)?;
                Ok((
                    &dark.image,
                    master_scale(image, &dark.image, MasterRole::Dark)?,
                    scale,
                ))
            })
            .transpose()?;
        // The bias is subtracted on its own unless the dark still holds it.
        let bias = self
            .bias
            .as_ref()
            .filter(|_| {
                self.dark
                    .as_ref()
                    .is_none_or(|dark| dark.bias == DarkBias::Removed)
            })
            .map(|bias| master_scale(image, bias, MasterRole::Bias).map(|map| (bias, map)))
            .transpose()?;
        if self.flat.is_some() && dark.is_none() && bias.is_none() && holds_offset(image) {
            return Err(CalibrationError::LightWithoutSubtractor);
        }

        image.metadata.calibrated = true;
        if let Some((bias, map)) = bias {
            image.subtract(bias, map);
        }
        if let Some((dark, map, scale)) = dark {
            image.subtract_scaled(dark, map, scale);
        }
        if let Some(flat) = &self.flat {
            flat.apply(image);
        }
        if let Some(defect_map) = &self.defect_map {
            defect_map.correct(image);
        }
        Ok(outcome)
    }

    /// The factor `dark` is subtracted from `light` by: 1 when their exposures match or one is
    /// undeclared, the ratio for a bias-removed dark of another exposure. Records in `outcome` what
    /// it could not compare.
    fn dark_scale(
        &self,
        dark: &MasterDark,
        light: &CfaImage,
        outcome: &mut CalibrationOutcome,
    ) -> Result<f64, CalibrationError> {
        match (light.metadata.ccd_temp, dark.temperature()) {
            (Some(light), Some(dark)) if (light - dark).abs() > TEMPERATURE_TOLERANCE => {
                return Err(CalibrationError::DarkTemperatureMismatch { light, dark });
            }
            (Some(_), Some(_)) => {}
            _ => outcome.unverified_temperature = true,
        }
        let (Some(light), Some(exposure)) = (light.metadata.exposure_time, dark.exposure()) else {
            outcome.unverified_exposure = true;
            return Ok(1.0);
        };
        if (light - exposure).abs() <= EXPOSURE_TOLERANCE * light.max(exposure) {
            return Ok(1.0);
        }
        match dark.bias {
            DarkBias::Removed => {
                let scale = light / exposure;
                outcome.dark_scale = Some(scale);
                Ok(scale)
            }
            DarkBias::Included => Err(CalibrationError::DarkExposureMismatch {
                light,
                dark: exposure,
            }),
        }
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
        let light_domain = image.metadata.domain.as_ref();

        for (role, master) in self.masters() {
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
                    (light_domain, &master.metadata.domain)
                && !master_domain.units_agree(light_domain)
            {
                return Err(CalibrationError::SampleDomainMismatch {
                    component: role,
                    frame: light_domain.clone(),
                    master: master_domain.clone(),
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

/// Whether `frame` may still hold an additive offset: its pedestal is kept or unknown. A frame with
/// no domain was synthesized, and states nothing to check.
fn holds_offset(frame: &CfaImage) -> bool {
    frame
        .metadata
        .domain
        .as_ref()
        .is_some_and(|domain| domain.pedestal != Pedestal::Removed)
}

/// The map that expresses `master`'s samples in `frame`'s domain, for the master in `role`.
///
/// The identity when either declares no domain: a synthesized frame has none, and there is nothing
/// to convert between. An error when the two cannot be related exactly — a different stated unit, a
/// span the decoder had to assume, or a known pedestal against an unknown one — because subtracting
/// across such a gap is silently wrong.
fn master_scale(
    frame: &CfaImage,
    master: &CfaImage,
    role: MasterRole,
) -> Result<DomainMap, CalibrationError> {
    let (Some(frame_domain), Some(master_domain)) =
        (&frame.metadata.domain, &master.metadata.domain)
    else {
        return Ok(DomainMap::IDENTITY);
    };
    master_domain.conversion_to(frame_domain).ok_or_else(|| {
        CalibrationError::SampleDomainMismatch {
            component: role,
            frame: frame_domain.clone(),
            master: master_domain.clone(),
        }
    })
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use common::CancelToken;

    use crate::calibration_masters::calibration_set::CalibrationSet;

    use crate::calibration_masters::master_role::MasterRole;

    use crate::calibration_masters::{CalibrationMasters, stack_cfa_master};
    use crate::io::image::cfa::CfaImage;
    use crate::progress::ProgressCallback;

    /// Every role stacked under its preset, the flats with their flat-dark or bias taken from each
    /// frame, then the set assembled — what a caller does role by role.
    pub(crate) fn masters_from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
    ) -> CalibrationMasters {
        let stack = |paths: &[P], role: MasterRole, subtract: Option<&CfaImage>| {
            stack_cfa_master(
                paths,
                role.stack_config(),
                subtract,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .expect("stack a calibration master")
        };
        let bias = stack(frames.bias, MasterRole::Bias, None);
        let flat_dark = stack(frames.flat_dark, MasterRole::FlatDark, None);
        let flat = stack(
            frames.flat,
            MasterRole::Flat,
            flat_dark.as_ref().or(bias.as_ref()),
        );
        CalibrationMasters::from_images(
            CalibrationSet {
                dark: stack(frames.dark, MasterRole::Dark, None),
                flat,
                bias,
                flat_dark,
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
