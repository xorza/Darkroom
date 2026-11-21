//! [`CalibrationMasters`]: the masters a light is calibrated by, built from a session's frames or
//! loaded from a bundle.

pub(crate) mod additive;

use std::path::Path;

use arrayvec::ArrayVec;
use common::CancelToken;

use crate::calibration::bundle;
use crate::calibration::bundle::error::BundleError;
use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_masters::additive::Additive;
use crate::calibration::calibration_outcome::{CalibrationOutcome, DarkOutcome};
use crate::calibration::calibration_pass::{CalibrationPass, Saturation, Subtracted};
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::dark_scale::{DarkScale, DarkScaleSource, ExposureRatio, FitLight};
use crate::calibration::defect_map::DefectMap;
use crate::calibration::defect_map::defect_summary::DefectSummary;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::error::{CalibrationError, MasterBuildError};
use crate::calibration::imposed_flags;
use crate::calibration::master_cache::{MasterCache, MasterKey, MasterSlot};
use crate::calibration::master_recipe::MasterRecipe;
use crate::calibration::master_role::MasterRole;
use crate::calibration::master_stack::MasterStack;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::calibration::sensor::Sensor;
use crate::calibration::subtraction::{self, Subtraction, Term};
use crate::calibration::subtractor::Subtractor;
use crate::frame::capture_conditions::{SensorSetting, SettingsCompared};
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::metadata::dark_comparison::DarkComparison;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::sample_domain::Pedestal;
use crate::ingest::ingest_config::IngestConfig;
use crate::io::decode_config::DecodeConfig;
use crate::io::load_ctx::LoadCtx;
use crate::progress::progress_monitor::ProgressMonitor;

/// The calibration a light receives: what to subtract, the flat to divide by, and the defects to
/// repair. Only what [`Self::calibrate`] reads is kept; the flat-dark is spent on the flat.
///
/// Construction removes the flat's additive part if its stack did not, detects cold pixels from
/// that unfloored response, then consumes it into a normalized, floored divisor. With a bias beside
/// the dark, the dark keeps its thermal signal alone, which scales with exposure. Calibration
/// operates on raw CFA data before demosaicing so defect correction can use same-color neighbors.
#[derive(Debug, Default)]
pub struct CalibrationMasters {
    additive: Additive,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
    /// What calibrating a light by these masters leaves it flagged, gathered once:
    /// [`QualityFlags::NO_DATA`](crate::QualityFlags::NO_DATA) where a master the light is calibrated by holds no measurement — a
    /// fill or a saturated bound leave none in the difference or the quotient — and
    /// [`QualityFlags::FLAT_FLOOR`](crate::QualityFlags::FLAT_FLOOR) where the divisor sits at its floor, so the light is corrected
    /// by less than its vignetting asks.
    imposed: Option<PixelFlags>,
}

/// How [`CalibrationMasters::build`] builds: the σ of defect detection, and the cache of masters
/// it reads and writes, none stacking every master.
#[derive(Debug, Clone, Copy)]
pub struct MastersConfig<'a> {
    pub hot_sigma: SigmaThreshold,
    pub cache: Option<&'a dyn MasterCache>,
}

impl Default for MastersConfig<'_> {
    fn default() -> Self {
        Self {
            hot_sigma: SigmaThreshold::DEFAULT,
            cache: None,
        }
    }
}

impl CalibrationMasters {
    /// The masters a bundle at `path` holds, as [`Self::write_bundle`] wrote them, without
    /// rebuilding the prepared flat or the defect map: `cancel` reads as
    /// [`BundleError::Cancelled`]. The file is streamed, never read into RAM whole. A bundle is
    /// lumos's own format, so no load option applies: each master decodes as it was written.
    ///
    /// # Errors
    ///
    /// [`BundleError`], one variant for each way a file can fail to be a bundle of this version.
    pub fn read_bundle(path: impl AsRef<Path>, cancel: &CancelToken) -> Result<Self, BundleError> {
        bundle::load(
            path.as_ref(),
            &LoadCtx::new(DecodeConfig::default(), cancel.clone()),
        )
    }

    /// Write the masters to `path` as a versioned, checksummed multi-extension FITS bundle: the
    /// flat prepared and the dark with its bias state, so [`Self::read_bundle`] repeats neither the
    /// flat's preparation nor the defect detection.
    pub fn write_bundle(&self, path: impl AsRef<Path>) -> Result<(), BundleError> {
        bundle::save(path.as_ref(), self)
    }

    /// Stack every role of `frames` into its master under the role's preset, read as `ingest`
    /// says, and assemble the set ([`Self::from_images`]) at `config`'s defect σ: the bias and the
    /// flat-dark first, then the flats, then the dark. Each flat loses its additive parts before
    /// the multiplicative normalization scales it, as PixInsight and Siril calibrate flats, by the
    /// rule a light loses them by: the bias as it stands, and the flat-dark's dark signal — the
    /// flat-dark without the bias, when there is one — scaled to the flat's own exposure. With no
    /// bias the flat-dark holds the bias too, and only a flat of its exposure can take it. A role
    /// with no frames has no master. `config`'s cache is read before a role is stacked, and
    /// written once it is, under the [`MasterKey`] lumos composes of its frames.
    ///
    /// # Errors
    ///
    /// A role's stack fails ([`StackError`](crate::StackError)), the masters do not make a set
    /// ([`CalibrationError`]) — flat-darks with no flat among them, before any frame is read — or
    /// the build is cancelled.
    pub fn build<P: AsRef<Path>>(
        frames: CalibrationSet<&[P]>,
        config: &MastersConfig<'_>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<Self, MasterBuildError> {
        let frames = frames.map(|paths| paths.iter().map(AsRef::as_ref).collect::<Vec<&Path>>());
        Self::build_from(&frames, config, ingest, monitor)
    }

    fn build_from(
        frames: &CalibrationSet<Vec<&Path>>,
        config: &MastersConfig<'_>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<Self, MasterBuildError> {
        // A flat-dark calibrates the flats alone: without them it would be stacked and dropped.
        if frames.flat.is_empty() && !frames.flat_dark.is_empty() {
            return Err(MasterBuildError::Calibration(
                CalibrationError::FlatDarksWithoutFlats {
                    count: frames.flat_dark.len(),
                },
            ));
        }
        // Each role's master and its key, which a master taken from its frames' keys by.
        let stack = |role: MasterRole,
                     subtract: &[(Subtractor<'_>, Option<MasterKey>)]|
         -> Result<Option<(CfaImage, Option<MasterKey>)>, MasterBuildError> {
            let paths = frames.get(role).as_slice();
            if paths.is_empty() {
                return Ok(None);
            }
            if monitor.cancel.is_cancelled() {
                return Err(MasterBuildError::Cancelled);
            }
            let key = subtract
                .iter()
                .map(|(_, key)| *key)
                .collect::<Option<ArrayVec<MasterKey, 2>>>()
                .and_then(|subtracted| {
                    MasterKey::of(paths, MasterRecipe::of(role, ingest), &subtracted)
                });
            let slot = key.map(|key| MasterSlot {
                role,
                frames: paths,
                key,
            });
            let cache = config.cache.zip(slot);
            if let Some(master) = cache.and_then(|(cache, slot)| cache.load(&slot)) {
                return Ok(Some((master, key)));
            }
            let subtractors: ArrayVec<Subtractor<'_>, 2> =
                subtract.iter().map(|(subtractor, _)| *subtractor).collect();
            let master = MasterStack {
                paths,
                role,
                config: role.stack_config(),
                ingest,
                subtract: &subtractors,
            }
            .run(monitor.clone())?
            .expect("a role with frames stacks a master");
            if let Some((cache, slot)) = cache {
                cache.store(&slot, &master);
            }
            Ok(Some((master, key)))
        };
        let bias = stack(MasterRole::Bias, &[])?;
        let flat_dark = stack(MasterRole::FlatDark, &[])?
            .map(|(flat_dark, key)| {
                without_bias(flat_dark, bias.as_ref().map(|(bias, _)| bias))
                    .map(|flat_dark| (flat_dark, key))
            })
            .transpose()?;
        let flat = {
            let subtract = [
                (MasterRole::Bias, bias.as_ref()),
                (MasterRole::FlatDark, flat_dark.as_ref()),
            ]
            .into_iter()
            .filter_map(|(role, master)| {
                master.map(|(master, key)| (Subtractor { role, master }, *key))
            })
            .collect::<ArrayVec<_, 2>>();
            stack(MasterRole::Flat, &subtract)?
        };
        let dark = stack(MasterRole::Dark, &[])?;
        let ProgressMonitor { cancel, .. } = monitor;
        let master =
            |master: Option<(CfaImage, Option<MasterKey>)>| master.map(|(master, _)| master);
        Ok(Self::from_images(
            CalibrationSet {
                dark: master(dark),
                flat: master(flat),
                bias: master(bias),
                flat_dark: master(flat_dark),
            },
            config.hot_sigma,
            &cancel,
        )?)
    }

    /// The defect map, when the masters hold one.
    pub(crate) const fn defect_map(&self) -> Option<&DefectMap> {
        self.defect_map.as_ref()
    }

    /// The present masters, with their roles, in bundle order.
    pub(crate) fn masters(&self) -> impl Iterator<Item = (MasterRole, &CfaImage)> {
        [
            (MasterRole::Dark, self.additive.dark()),
            (
                MasterRole::Flat,
                self.flat.as_ref().map(PreparedFlat::divisor),
            ),
            (MasterRole::Bias, self.additive.bias()),
        ]
        .into_iter()
        .filter_map(|(role, master)| master.map(|master| (role, master)))
    }

    /// Components present in this bundle, in bundle order.
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
        Some(map.summary(&master.cfa_type()))
    }

    /// What the flat-dark was compared to the flat on, or the union over every frame the flat was
    /// stacked from, as a light's [`CalibrationOutcome::dark`] reports its dark; `None` without a
    /// flat or a flat-dark.
    pub fn flat_dark(&self) -> Option<DarkComparison> {
        self.flat
            .as_ref()?
            .divisor()
            .metadata
            .derived
            .dark_comparison()
    }

    /// Photosites the flat's floor raised: every light is corrected by less than its vignetting
    /// asks there.
    pub fn floored_flat_pixels(&self) -> usize {
        self.flat.as_ref().map_or(0, PreparedFlat::floored)
    }

    /// Resident RAM held by this bundle: the present masters' planes and flags, the flat's gain,
    /// the defect map's index lists, and the flags the masters impose on a light.
    pub fn ram_bytes(&self) -> usize {
        self.masters()
            .filter(|&(role, _)| role != MasterRole::Flat)
            .map(|(_, master)| master.ram_bytes())
            .sum::<usize>()
            + self.flat.as_ref().map_or(0, PreparedFlat::ram_bytes)
            + self.defect_map.as_ref().map_or(0, DefectMap::ram_bytes)
            + self.imposed.as_ref().map_or(0, |flags| flags.bytes().len())
    }

    /// `parts` as one coherent set, checked as [`Self::from_images`] checks its inputs, with the
    /// flags they impose on a light gathered: one sensor, every master's record one its role can
    /// hold, and a defect map only beside the dark or the flat it was detected on.
    ///
    /// # Errors
    ///
    /// [`CalibrationError`] when the parts do not make such a set.
    pub(crate) fn assemble(parts: MasterParts) -> Result<Self, CalibrationError> {
        let MasterParts {
            additive,
            flat,
            defect_map,
        } = parts;
        if defect_map.is_some() && additive.dark().is_none() && flat.is_none() {
            return Err(CalibrationError::DefectsWithoutMaster);
        }
        let mut masters = Self {
            additive,
            flat,
            defect_map,
            imposed: None,
        };
        Sensor::shared(masters.sensors())?;
        masters.validate_records()?;
        masters.imposed = masters.imposed_flags();
        Ok(masters)
    }

    /// See [`Self::imposed`](CalibrationMasters#structfield.imposed); `None` when they impose none.
    fn imposed_flags(&self) -> Option<PixelFlags> {
        imposed_flags::of(
            [
                self.additive.bias(),
                self.additive.dark(),
                self.flat.as_ref().map(PreparedFlat::divisor),
            ]
            .into_iter()
            .flatten(),
            self.flat.as_ref(),
        )
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
    /// ([`Self::flat_dark`]). Cold pixels are detected on that flat, hot pixels on the
    /// dark without its bias, the dark the lights are calibrated with. `sigma_threshold` controls
    /// defect detection sensitivity.
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
        sigma_threshold: SigmaThreshold,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }
        // Before any of the work below, all of which combines the masters by flat pixel index and
        // asserts on only the pair it touches.
        let size = Sensor::shared(
            images
                .iter()
                .filter_map(|(role, master)| Some((role.into(), Sensor::of(master.as_ref()?)))),
        )?
        .map(Sensor::size);
        for (role, master) in images.iter() {
            if let Some(master) = master {
                check_record(role, master)?;
            }
        }
        // A flat-dark calibrates the flat alone, as `build` refuses its frames without flats.
        if images.flat.is_none() && images.flat_dark.is_some() {
            return Err(CalibrationError::FlatDarksWithoutFlats { count: 1 });
        }

        let CalibrationSet {
            dark,
            flat,
            bias,
            flat_dark,
        } = images;
        let flat_dark = flat_dark
            .map(|flat_dark| without_bias(flat_dark, bias.as_ref()))
            .transpose()?;
        let subtracted_flat = flat
            .map(|mut flat| -> Result<CfaImage, CalibrationError> {
                let lost = flat.metadata.derived.calibration();
                // The flat-dark when the flat still holds what it removes: a flat stacked with
                // its subtractor taken from each frame lost it already.
                let flat_dark = flat_dark
                    .as_ref()
                    .map(|master| Subtractor {
                        role: MasterRole::FlatDark,
                        master,
                    })
                    .filter(|flat_dark| !lost.contains(flat_dark.removes()));
                if flat_dark.is_some_and(|flat_dark| flat_dark.removes().overlaps(lost)) {
                    return Err(CalibrationError::UnusableFlatDark);
                }
                // The bias when the flat holds it and the flat-dark does not remove it beside its
                // dark signal.
                let bias =
                    if lost.bias || flat_dark.is_some_and(|flat_dark| flat_dark.removes().bias) {
                        None
                    } else if let Some(master) = &bias {
                        Some(Subtractor {
                            role: MasterRole::Bias,
                            master,
                        })
                    } else if holds_offset(&flat) {
                        return Err(CalibrationError::FlatWithoutSubtractor);
                    } else {
                        None
                    };
                let subtractors: ArrayVec<Subtractor<'_>, 2> =
                    [bias, flat_dark].into_iter().flatten().collect();
                Subtraction::resolve(&flat, subtractors.iter().copied())?.apply(
                    &mut flat,
                    subtraction::unmeasured(&subtractors).as_ref(),
                    Saturation::Kept,
                );
                Ok(flat)
            })
            .transpose()?;

        // The dark the lights are calibrated with: without the bias, which each light loses on its
        // own. Its hot pixels are measured on it, so the excess a light's dark scale is fitted
        // against carries no bias pattern, and a pixel the bias alone sets apart is no defect.
        let dark = dark
            .map(|dark| without_bias(dark, bias.as_ref()))
            .transpose()?;
        // Hot pixels from that dark, cold/dead pixels from the subtracted flat — None if neither
        // exists. Detection must precede normalization's near-zero floor.
        let defect_map = if dark.is_some() || subtracted_flat.is_some() {
            let mut map =
                DefectMap::new(size.expect("a present master gives the set its dimensions"));
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

        let flat = subtracted_flat.map(PreparedFlat::new).transpose()?;
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }

        Self::assemble(MasterParts {
            additive: Additive::of(bias, dark)?,
            flat,
            defect_map,
        })
    }

    /// Every present master lost no more to calibration than its role can: the check `from_images`
    /// runs on its inputs, for a bundle read from a file.
    fn validate_records(&self) -> Result<(), CalibrationError> {
        self.masters()
            .try_for_each(|(role, master)| check_record(role, master))
    }

    /// Every present master, then the defect map, with the sensor it is of.
    fn sensors(&self) -> impl Iterator<Item = (CalibrationComponent, Sensor)> {
        self.masters()
            .map(|(role, master)| (role.into(), Sensor::of(master)))
            .chain(
                self.defect_map
                    .as_ref()
                    .map(|map| (CalibrationComponent::Defects, Sensor::of_defects(map))),
            )
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
        if !image.metadata.derived.calibration().is_none() {
            return Err(CalibrationError::AlreadyCalibrated);
        }
        self.validate_against_light(image)?;
        let mut outcome = CalibrationOutcome::default();
        let dark_holds_bias = self.additive.dark_holds_bias();
        let bias = self.additive.bias();
        if self.flat.is_some() && bias.is_none() && !dark_holds_bias && holds_offset(image) {
            return Err(CalibrationError::LightWithoutSubtractor);
        }

        // Each master's map is taken against the domain the light will have when that master is
        // applied: subtracting the bias moves the light's pedestal, which the dark's map has to
        // see, or the pedestal would be removed a second time.
        let subtractors = [
            bias.map(|master| Subtractor {
                role: MasterRole::Bias,
                master,
            }),
            self.additive.dark().map(|master| Subtractor {
                role: MasterRole::Dark,
                master,
            }),
        ];
        let subtraction = Subtraction::resolve(image, subtractors.into_iter().flatten())?;
        outcome.settings_compared = subtraction.settings == SettingsCompared::All;
        outcome.filter_differs = self.flat.as_ref().is_some_and(|flat| {
            let filter = |metadata: &ImageMetadata| {
                metadata
                    .header
                    .filter
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_owned)
            };
            matches!(
                (filter(&image.metadata), filter(&flat.divisor().metadata)),
                (Some(light), Some(flat)) if light != flat
            )
        });
        let bias = subtraction.term(MasterRole::Bias);
        let dark = subtraction.term(MasterRole::Dark);
        let bias_term = bias.map(|bias| bias.subtracted(1.0));
        // A bias-removed dark is scaled, by the fit where its hot pixels allow; one that holds the
        // bias is subtracted as it is, the exposures having matched.
        outcome.dark = match dark {
            None => DarkOutcome::NoDark,
            Some(dark) => {
                let matched = dark.matched.expect("a dark is matched to the light");
                if dark_holds_bias {
                    DarkOutcome::AsIs(matched.comparison)
                } else {
                    DarkOutcome::Scaled {
                        comparison: matched.comparison,
                        scale: self.dark_scale(image, bias_term, dark),
                    }
                }
            }
        };
        let dark_term = dark.map(|dark| {
            dark.subtracted(
                outcome
                    .dark
                    .scale()
                    .map_or_else(|| dark.matched_scale(), |scale| scale.factor),
            )
        });
        CalibrationPass {
            subtracted: [bias_term, dark_term].into_iter().flatten().collect(),
            flat: self
                .flat
                .as_ref()
                .map(|flat| flat.divisor().data().pixels()),
            imposed: self.imposed.as_ref(),
            saturation: Saturation::Detect,
        }
        .apply(image);
        subtraction.record(image);
        if let Some(flat) = &self.flat {
            flat.record(image);
        }
        // One mask of the pixels no repair may read serves both repairs: the defects join it as
        // they are repaired.
        let mut unmeasured = image.unmeasured_mask();
        if let Some(defect_map) = &self.defect_map {
            defect_map.correct(image, unmeasured.as_mut());
        }
        if let Some(unmeasured) = &unmeasured {
            image.repair_nulls_within(unmeasured);
        }
        Ok(outcome)
    }

    /// The scale of the bias-removed `dark` for `image`, less `bias`: fitted on the defect map's
    /// hot pixels where they beat the exposure ratio, else the ratio.
    fn dark_scale(
        &self,
        image: &CfaImage,
        bias: Option<Subtracted<'_>>,
        dark: &Term<'_>,
    ) -> DarkScale {
        let matched = dark.matched.expect("a dark is matched to the light");
        match &self.defect_map {
            Some(defects) => DarkScale::fit(
                defects.hot(),
                FitLight {
                    light: image,
                    bias,
                    imposed: self.imposed.as_ref(),
                    dark_gain: dark.map.gain,
                },
                ExposureRatio::of(matched),
            ),
            None => DarkScale {
                factor: matched.factor(),
                source: DarkScaleSource::Exposure { fit_pixels: 0 },
            },
        }
    }

    /// Every master must describe the same sensor as `image`, in pattern and extent, and the
    /// same instrument and unit where both state one.
    fn validate_against_light(&self, image: &CfaImage) -> Result<(), CalibrationError> {
        Sensor::of(image).admit(self.sensors())?;
        let light_domain = image.metadata.derived.domain();
        let light_instrument = image.metadata.header.instrument.as_deref().map(str::trim);

        for (role, master) in self.masters() {
            // Only when both state one: a synthesized master states none, and a FITS file may not.
            if let (Some(light), Some(master)) = (
                light_instrument,
                master.metadata.header.instrument.as_deref().map(str::trim),
            ) && light != master
            {
                return Err(CalibrationError::InstrumentMismatch {
                    component: role,
                    light: light.to_owned(),
                    master: master.to_owned(),
                });
            }
            // A flat of another binning sums other photosites than the light's.
            if role == MasterRole::Flat {
                SensorSetting::of(&image.metadata)
                    .compare_binning(SensorSetting::of(&master.metadata))
                    .map_err(|source| CalibrationError::SettingMismatch {
                        component: role,
                        source,
                    })?;
            }
            // The flat divides a normalized copy of itself, so its scale cancels; only a different
            // stated unit disqualifies it. A subtracted master's scale is checked in
            // `Subtractor::map_into`. Only when both declare a domain: a synthesized one has none.
            if role == MasterRole::Flat
                && let (Some(light_domain), Some(master_domain)) =
                    (light_domain, master.metadata.derived.domain())
                && !master_domain.units_agree(light_domain)
            {
                return Err(CalibrationError::SampleDomainMismatch {
                    component: role,
                    frame: light_domain.clone(),
                    master: master_domain.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The parts [`CalibrationMasters::assemble`] checks as one set.
#[derive(Debug)]
pub(crate) struct MasterParts {
    pub(crate) additive: Additive,
    pub(crate) flat: Option<PreparedFlat>,
    pub(crate) defect_map: Option<DefectMap>,
}

/// Whether `frame` may still hold an additive offset: its pedestal is kept or unknown. A frame with
/// no domain was synthesized, and states nothing to check.
fn holds_offset(frame: &CfaImage) -> bool {
    frame
        .metadata
        .derived
        .domain()
        .is_some_and(|domain| domain.pedestal != Pedestal::Removed)
}

/// `master` lost no more to calibration than a master in `role` can.
const fn check_record(role: MasterRole, master: &CfaImage) -> Result<(), CalibrationError> {
    if role
        .may_have_lost()
        .contains(master.metadata.derived.calibration())
    {
        Ok(())
    } else {
        Err(CalibrationError::OverCalibratedMaster { component: role })
    }
}

/// `master` without `bias`, when there is one and the master still holds it: a dark or a flat-dark
/// keeps only the dark signal, which scales with exposure.
fn without_bias(
    mut master: CfaImage,
    bias: Option<&CfaImage>,
) -> Result<CfaImage, CalibrationError> {
    if let Some(bias) = bias
        && !master.metadata.derived.calibration().bias
    {
        let subtractors = [Subtractor {
            role: MasterRole::Bias,
            master: bias,
        }];
        Subtraction::resolve(&master, subtractors)?.apply(
            &mut master,
            subtraction::unmeasured(&subtractors).as_ref(),
            Saturation::Kept,
        );
    }
    Ok(master)
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::calibration::calibration_masters::{CalibrationMasters, MastersConfig};
    use crate::calibration::calibration_set::CalibrationSet;
    use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::progress::progress_monitor::ProgressMonitor;

    /// [`CalibrationMasters::build`] with no cache, on the default ingest, in a test that expects
    /// it to succeed.
    pub(crate) fn masters_from_files<P: AsRef<Path>>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: SigmaThreshold,
    ) -> CalibrationMasters {
        CalibrationMasters::build(
            frames,
            &MastersConfig {
                hot_sigma: sigma_threshold,
                cache: None,
            },
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .expect("build the calibration masters")
    }
}

#[cfg(test)]
mod tests;
