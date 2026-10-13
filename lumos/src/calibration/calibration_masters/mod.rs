//! [`CalibrationMasters`]: the masters a light is calibrated by, built from a session's frames or
//! loaded from a bundle.

pub(super) mod fits;

use std::path::Path;

use common::CancelToken;

use crate::calibration::dark_match::{DarkBias, DarkMatch};
use crate::calibration::dark_scale::{DarkScale, DarkScaleSource, FitLight};
use crate::calibration::defect_map::DefectMap;
use crate::calibration::defect_map::defect_summary::DefectSummary;
use crate::calibration::error::{BundleError, CalibrationError, MasterBuildError};
use crate::calibration::light_calibration::{LightCalibration, Subtracted};
use crate::calibration::master_cache::{CachedMaster, MasterCache, MasterInputs, SubtractedFrames};
use crate::calibration::master_recipe::MasterRecipe;
use crate::calibration::master_stack::MasterStack;
use crate::calibration::subtractor::Subtractor;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::{Pedestal, SampleDomain};
use crate::ingest::ingest_config::IngestConfig;
use crate::io::error::FitsWriteError;
use crate::io::load_context::LoadContext;
use crate::math::size2us::Size2us;
use crate::progress::progress_monitor::ProgressMonitor;

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_outcome::CalibrationOutcome;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::master_role::MasterRole;
use crate::calibration::prepared_flat::PreparedFlat;

/// Default sigma threshold for defect detection.
///
/// A pixel is flagged as defective if it exceeds the per-color residual median by more than
/// `sigma_threshold × σ`, where robust bulk statistics and source resolution determine σ.
/// PixInsight uses 3.0; 5.0 is more conservative (fewer false positives).
pub const DEFAULT_SIGMA_THRESHOLD: f32 = 5.0;

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
    /// The dark, its record saying whether it lost the bias.
    dark: Option<CfaImage>,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
    /// What calibrating a light by these masters leaves it flagged, gathered once:
    /// [`QualityFlags::NO_DATA`] where a master the light is calibrated by holds no measurement — a
    /// fill or a saturated bound leave none in the difference or the quotient — and
    /// [`QualityFlags::FLAT_FLOOR`] where the divisor sits at its floor, so the light is corrected
    /// by less than its vignetting asks.
    imposed: Option<PixelFlags>,
}

impl CalibrationMasters {
    /// Stack every role of `frames` into its master under the role's preset, read as `ingest`
    /// says, and assemble the set ([`Self::from_images`]): the bias and the flat-dark first, then
    /// the flats, each with its flat-dark or else the bias taken from every frame before the
    /// multiplicative normalization scales it, as PixInsight and Siril calibrate flats, then the
    /// dark. A role with no frames has no master. `cache` is read before a role is stacked, and
    /// written once it is under the key that read gave; [`NoMasterCache`](crate::NoMasterCache)
    /// stacks every one.
    ///
    /// # Errors
    ///
    /// A role's stack fails ([`StackError`](crate::StackError)), the masters do not make a set
    /// ([`CalibrationError`]), the cache fails, or the build is cancelled.
    pub fn build<P: AsRef<Path>, C: MasterCache<P>>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
        ingest: &IngestConfig,
        cache: &C,
        monitor: &ProgressMonitor,
    ) -> Result<Self, MasterBuildError<C::Error>> {
        let stack = |role: MasterRole, subtract: Option<(Subtractor<'_>, &[P])>| {
            let paths = *frames.get(role);
            if paths.is_empty() {
                return Ok(None);
            }
            if monitor.cancel.is_cancelled() {
                return Err(MasterBuildError::Cancelled);
            }
            let inputs = MasterInputs {
                role,
                frames: paths,
                subtracted: subtract.map(|(subtractor, frames)| SubtractedFrames {
                    role: subtractor.role,
                    frames,
                }),
                recipe: MasterRecipe::of(role, ingest),
            };
            let key = match cache.load(&inputs).map_err(MasterBuildError::Cache)? {
                CachedMaster::Kept(master) => return Ok(Some(*master)),
                CachedMaster::Missing(key) => key,
            };
            let master = MasterStack {
                paths,
                role,
                config: role.stack_config(),
                ingest,
                subtract: subtract.map(|(subtractor, _)| subtractor),
            }
            .run(monitor.clone())?
            .expect("a role with frames stacks a master");
            cache.store(key, &master).map_err(MasterBuildError::Cache)?;
            Ok(Some(master))
        };
        let bias = stack(MasterRole::Bias, None)?;
        let flat_dark = stack(MasterRole::FlatDark, None)?;
        let subtractor = [
            (MasterRole::FlatDark, flat_dark.as_ref()),
            (MasterRole::Bias, bias.as_ref()),
        ]
        .into_iter()
        .find_map(|(role, master)| {
            master.map(|master| (Subtractor { role, master }, *frames.get(role)))
        });
        let flat = stack(MasterRole::Flat, subtractor)?;
        let dark = stack(MasterRole::Dark, None)?;
        Ok(Self::from_images(
            CalibrationSet {
                dark,
                flat,
                bias,
                flat_dark,
            },
            sigma_threshold,
            &monitor.cancel,
        )?)
    }

    /// The present masters, with their roles, in calibration order.
    fn masters(&self) -> impl Iterator<Item = (MasterRole, &CfaImage)> {
        [
            (MasterRole::Dark, self.dark.as_ref()),
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
        let map = self.defect_map.as_ref()?;
        let (_, master) = self
            .masters()
            .next()
            .expect("a defect map comes with the master it was detected on");
        Some(map.summary(&master.cfa_type))
    }

    /// The conditions not compared when the flat-dark was taken from the flat, or from any frame
    /// the flat was stacked from: the flat is corrected for a dark signal nothing checked.
    pub fn unverified_flat_dark(&self) -> UnverifiedConditions {
        self.flat
            .as_ref()
            .map_or(UnverifiedConditions::NONE, |flat| {
                flat.divisor().metadata.unverified_dark
            })
    }

    /// Photosites the flat's floor raised: every light is corrected by less than its vignetting
    /// asks there.
    pub fn floored_flat_pixels(&self) -> usize {
        self.flat.as_ref().map_or(0, PreparedFlat::floored)
    }

    /// Resident RAM held by this bundle: the present master frames' pixel bytes, the defect map's
    /// index lists, and the flags the masters impose on a light.
    pub fn ram_bytes(&self) -> usize {
        let frame_bytes = self
            .masters()
            .map(|(_, master)| master.ram_bytes())
            .sum::<usize>();
        frame_bytes
            + self.defect_map.as_ref().map_or(0, DefectMap::ram_bytes)
            + self.imposed.as_ref().map_or(0, |flags| flags.bytes().len())
    }

    /// The masters as one coherent bundle, checked as [`Self::from_images`] checks its inputs, with
    /// the flags they impose on a light gathered.
    ///
    /// # Errors
    /// [`CalibrationError`] when the masters or the map do not describe one sensor, or a master
    /// lost more to calibration than its role can.
    pub(super) fn assemble(
        bias: Option<CfaImage>,
        dark: Option<CfaImage>,
        flat: Option<PreparedFlat>,
        defect_map: Option<DefectMap>,
    ) -> Result<Self, CalibrationError> {
        let mut masters = Self {
            bias,
            dark,
            flat,
            defect_map,
            imposed: None,
        };
        masters.validate_dimensions()?;
        masters.validate_records()?;
        masters.imposed = masters.imposed_flags();
        Ok(masters)
    }

    /// Whether the dark still holds the bias, which the bias is then not subtracted beside.
    fn dark_holds_bias(&self) -> bool {
        self.dark
            .as_ref()
            .is_some_and(|dark| !dark.metadata.calibration.bias)
    }

    /// The bias a light is calibrated by: none when the dark still holds it.
    fn used_bias(&self) -> Option<&CfaImage> {
        self.bias.as_ref().filter(|_| !self.dark_holds_bias())
    }

    /// See [`Self::imposed`](CalibrationMasters#structfield.imposed); `None` when they impose none.
    fn imposed_flags(&self) -> Option<PixelFlags> {
        let unmeasured: Vec<&PixelFlags> = [
            self.used_bias(),
            self.dark.as_ref(),
            self.flat.as_ref().map(PreparedFlat::divisor),
        ]
        .into_iter()
        .flatten()
        .filter_map(|master| master.flags.as_ref())
        .filter(|flags| flags.contains(QualityFlags::UNMEASURED))
        .collect();
        let floored = self.flat.as_ref().filter(|flat| flat.floored() > 0);
        if unmeasured.is_empty() && floored.is_none() {
            return None;
        }
        let (_, first) = self.masters().next()?;
        PixelFlags::from_fn(first.size(), |index| {
            let mut flags = QualityFlags::default();
            if unmeasured
                .iter()
                .any(|master| master.at(index).intersects(QualityFlags::UNMEASURED))
            {
                flags = flags.union(QualityFlags::NO_DATA);
            }
            if floored.is_some_and(|flat| flat.is_floored(index)) {
                flags = flags.union(QualityFlags::FLAT_FLOOR);
            }
            flags
        })
    }

    /// Save this coherent master bundle as a versioned, checksummed multi-extension FITS file.
    ///
    /// The flat is stored prepared and the dark with its bias state. Loading the bundle does not
    /// repeat flat preparation or defect detection.
    pub fn save(&self, path: &Path) -> Result<(), FitsWriteError> {
        fits::save(path, self)
    }

    /// Load a bundle written by [`Self::save`] without rebuilding its prepared flat or defect map,
    /// under `context`: its cancellation, which reads as [`BundleError::Cancelled`], and its
    /// memory limit on each master's decode. The file is streamed, never read into RAM whole.
    ///
    /// # Errors
    ///
    /// [`BundleError`], one variant for each way a file can fail to be a bundle of this version.
    pub fn load(path: &Path, context: &LoadContext) -> Result<Self, BundleError> {
        fits::load(path, context)
    }

    /// Create `CalibrationMasters` from pre-built CFA images.
    ///
    /// Each master's record of what calibration removed says what it still holds. With a bias in
    /// the set, the dark and the flat-dark lose their bias, so each keeps the dark signal alone,
    /// which scales with exposure. The flat then loses what it still holds of the two additive
    /// parts, matched to its exposure and temperature: the dark signal by the flat-dark, and the
    /// bias by the bias, or by a flat-dark that still holds it. A flat stacked with its subtractor
    /// taken from each frame ([`Self::build`]) already lost those parts, and does not lose them
    /// again, and what its stack's match did not compare stays on it beside what this one did not
    /// ([`Self::unverified_flat_dark`]). Cold pixels are detected on that flat, hot pixels on the
    /// dark as given. `sigma_threshold` controls defect detection sensitivity (see
    /// [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// # Errors
    ///
    /// A [`CalibrationError`] when the masters do not describe one sensor, a master lost more to
    /// calibration than its role can, the flat-dark does not match the flat or would remove a part
    /// the flat already lost, the flat has no positive mean to normalize by or holds an offset
    /// nothing removes, and [`CalibrationError::Cancelled`] if cancellation is requested before
    /// defect detection completes.
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
        for (role, master) in images.iter() {
            if let Some(master) = master {
                check_record(role, master)?;
            }
        }

        let CalibrationSet {
            dark,
            flat,
            bias,
            flat_dark,
        } = images;
        let without_bias = |master: Option<CfaImage>| {
            master
                .map(|mut master| -> Result<CfaImage, CalibrationError> {
                    if let Some(bias) = &bias
                        && !master.metadata.calibration.bias
                    {
                        Subtractor {
                            role: MasterRole::Bias,
                            master: bias,
                        }
                        .subtract_from(&mut master, None)?;
                    }
                    Ok(master)
                })
                .transpose()
        };
        let flat_dark = without_bias(flat_dark)?;
        let subtracted_flat = flat
            .map(|mut flat| -> Result<CfaImage, CalibrationError> {
                if let Some(flat_dark) = &flat_dark {
                    let holds = MasterRole::FlatDark
                        .signal()
                        .without(flat_dark.metadata.calibration);
                    let lost = flat.metadata.calibration;
                    if !lost.contains(holds) {
                        if holds.overlaps(lost) {
                            return Err(CalibrationError::UnusableFlatDark);
                        }
                        let matched = DarkMatch::new(
                            CaptureConditions::of(&flat.metadata),
                            CaptureConditions::of(&flat_dark.metadata),
                            DarkBias::held_in(holds),
                        )
                        .map_err(|source| {
                            CalibrationError::DarkMismatch {
                                component: MasterRole::FlatDark,
                                source,
                            }
                        })?;
                        Subtractor {
                            role: MasterRole::FlatDark,
                            master: flat_dark,
                        }
                        .subtract_from(&mut flat, Some(matched))?;
                    }
                }
                if !flat.metadata.calibration.bias {
                    if let Some(bias) = &bias {
                        Subtractor {
                            role: MasterRole::Bias,
                            master: bias,
                        }
                        .subtract_from(&mut flat, None)?;
                    } else if holds_offset(&flat) {
                        return Err(CalibrationError::FlatWithoutSubtractor);
                    }
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

        let dark = without_bias(dark)?;
        let flat = subtracted_flat.map(PreparedFlat::new).transpose()?;
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }

        Self::assemble(bias, dark, flat, defect_map)
    }

    /// Every present master lost no more to calibration than its role can: the check `from_images`
    /// runs on its inputs, for a bundle `fits::load` read. A flat is stored prepared, after it lost
    /// what it held of the additive parts.
    fn validate_records(&self) -> Result<(), CalibrationError> {
        self.masters()
            .try_for_each(|(role, master)| check_record(role, master))
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
    ///    within 1 °C. A bias-removed dark is scaled by the slope of the light's excess on its own
    ///    at the hot pixels ([`DarkScale`]), or, with too few of them, by the light's exposure over
    ///    its own; one that holds the bias and differs in exposure is refused. A temperature
    ///    outside the tolerance is a warning, not a refusal. A fact one side does not declare is
    ///    not compared, and the outcome says so.
    /// 2. Flat division with normalization
    /// 3. CFA-aware defect pixel correction
    ///
    /// The light's record of what calibration removed takes every part the masters removed.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError`] when the light already lost a part to calibration, the light
    /// or any stored master is missing CFA metadata, a master's pattern differs from the light,
    /// the dark does not match it, or a flat would divide a light that holds an offset nothing
    /// subtracts. Validation completes before the light is mutated.
    pub fn calibrate(&self, image: &mut CfaImage) -> Result<CalibrationOutcome, CalibrationError> {
        // A second pass would remove a part twice. The record comes from the file (`LUMCALB`,
        // `LUMCALD`, `LUMCALF`), so this is input to refuse, not an invariant to assert.
        if !image.metadata.calibration.is_none() {
            return Err(CalibrationError::AlreadyCalibrated);
        }
        self.validate_against_light(image)?;
        let mut outcome = CalibrationOutcome::default();
        let dark_holds_bias = self.dark_holds_bias();
        let bias = self.used_bias();
        if self.flat.is_some() && bias.is_none() && !dark_holds_bias && holds_offset(image) {
            return Err(CalibrationError::LightWithoutSubtractor);
        }

        // Each master's map is taken against the domain the light will have when that master is
        // applied: subtracting the bias moves the light's pedestal, which the dark's map has to
        // see, or the pedestal would be removed a second time.
        let mut domain = image.metadata.domain.clone();
        let bias = bias
            .map(|bias| -> Result<_, CalibrationError> {
                let bias = Subtractor {
                    role: MasterRole::Bias,
                    master: bias,
                };
                let map = bias.map_into(domain.as_ref())?;
                after_subtracting(&mut domain, bias.master);
                Ok((bias, map))
            })
            .transpose()?;
        let dark = self
            .dark
            .as_ref()
            .map(|dark| -> Result<_, CalibrationError> {
                let matched = DarkMatch::new(
                    CaptureConditions::of(&image.metadata),
                    CaptureConditions::of(&dark.metadata),
                    DarkBias::held_in(MasterRole::Dark.signal().without(dark.metadata.calibration)),
                )
                .map_err(|source| CalibrationError::DarkMismatch {
                    component: MasterRole::Dark,
                    source,
                })?;
                outcome.unverified = matched.unverified;
                outcome.dark_temperature = matched.temperature;
                outcome.dark_temperature_difference = matched.temperature_difference;
                let dark = Subtractor {
                    role: MasterRole::Dark,
                    master: dark,
                };
                let map = dark.map_into(domain.as_ref())?;
                after_subtracting(&mut domain, dark.master);
                Ok((dark, map, matched))
            })
            .transpose()?;

        let bias_term = bias.map(|(bias, map)| Subtracted {
            samples: bias.master.data.pixels(),
            map,
            scale: 1.0,
        });
        // A bias-removed dark is scaled, by the fit where its hot pixels allow; one that holds the
        // bias is subtracted as it is, the exposures having matched.
        outcome.dark_scale = dark
            .as_ref()
            .filter(|_| !dark_holds_bias)
            .map(|(_, map, matched)| match &self.defect_map {
                Some(defects) => DarkScale::fit(
                    defects.hot(),
                    FitLight {
                        light: image,
                        bias: bias_term,
                        imposed: self.imposed.as_ref(),
                        dark_gain: map.gain,
                    },
                    matched.factor(),
                ),
                None => DarkScale {
                    factor: matched.factor(),
                    source: DarkScaleSource::Exposure { fit_pixels: 0 },
                },
            });
        LightCalibration {
            bias: bias_term,
            dark: dark.map(|(dark, map, matched)| Subtracted {
                samples: dark.master.data.pixels(),
                map,
                scale: outcome
                    .dark_scale
                    .map_or_else(|| matched.factor(), |scale| scale.factor),
            }),
            flat: self.flat.as_ref().map(|flat| flat.divisor().data.pixels()),
            imposed: self.imposed.as_ref(),
        }
        .apply(image);
        if let Some((bias, _)) = bias {
            bias.record_removal(image, None);
        }
        if let Some((dark, _, matched)) = dark {
            dark.record_removal(image, Some(matched));
        }
        image.metadata.domain = domain;
        if let Some(flat) = &self.flat {
            flat.record(image);
        }
        if let Some(defect_map) = &self.defect_map {
            defect_map.correct(image);
        }
        image.repair_nulls();
        Ok(outcome)
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
        let light_instrument = image.metadata.instrument.as_deref().map(str::trim);

        for (role, master) in self.masters() {
            // Only when both state one: a synthesized master states none, and a FITS file may not.
            if let (Some(light), Some(master)) = (
                light_instrument,
                master.metadata.instrument.as_deref().map(str::trim),
            ) && light != master
            {
                return Err(CalibrationError::InstrumentMismatch {
                    component: role,
                    light: light.to_owned(),
                    master: master.to_owned(),
                });
            }
            if master.cfa_type != light {
                return Err(CalibrationError::CfaPatternMismatch {
                    component: role,
                    expected: light,
                    master: master.cfa_type,
                });
            }
            // The flat divides a normalized copy of itself, so its own scale cancels; only a
            // different stated unit disqualifies it. The subtracted master's scale is checked where
            // it is converted (`Subtractor::map_into`). Only when both declare a domain: a
            // synthesized master has none, and refusing on that would reject every in-memory
            // fixture.
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

/// `master` lost no more to calibration than a master in `role` can.
const fn check_record(role: MasterRole, master: &CfaImage) -> Result<(), CalibrationError> {
    if role.may_have_lost().contains(master.metadata.calibration) {
        Ok(())
    } else {
        Err(CalibrationError::OverCalibratedMaster { component: role })
    }
}

/// Move `domain`'s pedestal to where subtracting `master` leaves it, as
/// [`CfaImage::subtract`] does.
const fn after_subtracting(domain: &mut Option<SampleDomain>, master: &CfaImage) {
    if let (Some(domain), Some(master)) = (domain, &master.metadata.domain) {
        domain.pedestal = domain.after_subtracting(master);
    }
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::calibration::calibration_masters::CalibrationMasters;
    use crate::calibration::calibration_set::CalibrationSet;
    use crate::calibration::master_cache::NoMasterCache;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::progress::progress_monitor::ProgressMonitor;

    /// [`CalibrationMasters::build`] with no cache, on the default ingest, in a test that expects
    /// it to succeed.
    pub(crate) fn masters_from_files<P: AsRef<Path>>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
    ) -> CalibrationMasters {
        CalibrationMasters::build(
            frames,
            sigma_threshold,
            &IngestConfig::default(),
            &NoMasterCache,
            &ProgressMonitor::unattended(),
        )
        .expect("build the calibration masters")
    }
}

#[cfg(test)]
mod tests;
