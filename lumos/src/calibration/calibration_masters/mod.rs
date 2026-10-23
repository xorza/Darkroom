//! [`CalibrationMasters`]: the masters a light is calibrated by, built from a session's frames or
//! loaded from a bundle.

use std::path::Path;

use arrayvec::ArrayVec;
use common::CancelToken;

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_outcome::CalibrationOutcome;
use crate::calibration::calibration_pass::{CalibrationPass, Saturation};
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::dark_scale::{DarkScale, DarkScaleSource, FitLight};
use crate::calibration::defect_map::DefectMap;
use crate::calibration::defect_map::defect_summary::DefectSummary;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::error::{CalibrationError, MasterBuildError};
use crate::calibration::imposed_flags;
use crate::calibration::master_cache::{CachedMaster, MasterCache, MasterInputs, SubtractedFrames};
use crate::calibration::master_recipe::MasterRecipe;
use crate::calibration::master_role::MasterRole;
use crate::calibration::master_stack::MasterStack;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::calibration::sensor::Sensor;
use crate::calibration::subtraction::{self, Subtraction};
use crate::calibration::subtractor::Subtractor;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::dark_comparison::DarkComparison;
use crate::image::pixel_flags::PixelFlags;
use crate::image::sample_domain::Pedestal;
use crate::ingest::ingest_config::IngestConfig;
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

/// What [`CalibrationMasters::build`] stacks: each role's frames, the sigma threshold of defect
/// detection ([`SigmaThreshold::DEFAULT`] by default), and the cache of masters it reads and
/// writes.
#[derive(Debug)]
pub struct MasterBuild<'a, P, C> {
    pub frames: CalibrationSet<&'a [P]>,
    pub sigma_threshold: SigmaThreshold,
    pub cache: &'a C,
}

/// Copied whatever its type parameters are, as it holds references alone: a derive would ask each
/// to be `Copy`.
impl<P, C> Clone for MasterBuild<'_, P, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P, C> Copy for MasterBuild<'_, P, C> {}

impl CalibrationMasters {
    /// Stack every role of the build's frames into its master under the role's preset, read as
    /// `ingest` says, and assemble the set ([`Self::from_images`]): the bias and the flat-dark
    /// first, then the flats, then the dark. Each flat loses its additive parts before the
    /// multiplicative normalization scales it, as PixInsight and Siril calibrate flats, by the rule
    /// a light loses them by: the bias as it stands, and the flat-dark's dark signal — the
    /// flat-dark without the bias, when there is one — scaled to the flat's own exposure. With no
    /// bias the flat-dark holds the bias too, and only a flat of its exposure can take it. A role
    /// with no frames has no master. The cache is read before a role is stacked, and written once
    /// it is under the key that read gave; [`NoMasterCache`](crate::NoMasterCache) stacks every
    /// one.
    ///
    /// # Errors
    ///
    /// A role's stack fails ([`StackError`](crate::StackError)), the masters do not make a set
    /// ([`CalibrationError`]), the cache fails, or the build is cancelled.
    pub fn build<P: AsRef<Path>, C: MasterCache<P>>(
        build: MasterBuild<'_, P, C>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<Self, MasterBuildError<C::Error>> {
        let MasterBuild {
            frames,
            sigma_threshold,
            cache,
        } = build;
        let stack = |role: MasterRole, subtract: &[Subtractor<'_>]| {
            let paths = *frames.get(role);
            if paths.is_empty() {
                return Ok(None);
            }
            if monitor.cancel.is_cancelled() {
                return Err(MasterBuildError::Cancelled);
            }
            let subtracted = subtract
                .iter()
                .map(|subtractor| SubtractedFrames {
                    role: subtractor.role,
                    frames: frames.get(subtractor.role),
                })
                .collect::<ArrayVec<_, 2>>();
            let inputs = MasterInputs {
                role,
                frames: paths,
                subtracted: &subtracted,
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
                subtract,
            }
            .run(monitor.clone())?
            .expect("a role with frames stacks a master");
            cache.store(key, &master).map_err(MasterBuildError::Cache)?;
            Ok(Some(master))
        };
        let bias = stack(MasterRole::Bias, &[])?;
        let flat_dark = stack(MasterRole::FlatDark, &[])?
            .map(|flat_dark| without_bias(flat_dark, bias.as_ref()))
            .transpose()?;
        let flat = {
            let subtract = [
                (MasterRole::Bias, bias.as_ref()),
                (MasterRole::FlatDark, flat_dark.as_ref()),
            ]
            .into_iter()
            .filter_map(|(role, master)| master.map(|master| Subtractor { role, master }))
            .collect::<ArrayVec<_, 2>>();
            stack(MasterRole::Flat, &subtract)?
        };
        let dark = stack(MasterRole::Dark, &[])?;
        let ProgressMonitor { cancel, .. } = monitor;
        Ok(Self::from_images(
            CalibrationSet {
                dark,
                flat,
                bias,
                flat_dark,
            },
            sigma_threshold,
            &cancel,
        )?)
    }

    /// The defect map, when the masters hold one.
    pub(crate) const fn defect_map(&self) -> Option<&DefectMap> {
        self.defect_map.as_ref()
    }

    /// The present masters, with their roles, in calibration order.
    pub(crate) fn masters(&self) -> impl Iterator<Item = (MasterRole, &CfaImage)> {
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
        self.dark.as_ref().map_or(0, CfaImage::ram_bytes)
            + self.flat.as_ref().map_or(0, PreparedFlat::ram_bytes)
            + self.bias.as_ref().map_or(0, CfaImage::ram_bytes)
            + self.defect_map.as_ref().map_or(0, DefectMap::ram_bytes)
            + self.imposed.as_ref().map_or(0, |flags| flags.bytes().len())
    }

    /// A set of masters built one master at a time; see [`MastersBuilder::build`].
    pub(crate) fn builder() -> MastersBuilder {
        MastersBuilder::default()
    }

    /// Whether the dark still holds the bias, which the bias is then not subtracted beside.
    fn dark_holds_bias(&self) -> bool {
        self.dark
            .as_ref()
            .is_some_and(|dark| !dark.metadata.derived.calibration().bias)
    }

    /// The bias a light is calibrated by: none when the dark still holds it.
    fn used_bias(&self) -> Option<&CfaImage> {
        self.bias.as_ref().filter(|_| !self.dark_holds_bias())
    }

    /// See [`Self::imposed`](CalibrationMasters#structfield.imposed); `None` when they impose none.
    fn imposed_flags(&self) -> Option<PixelFlags> {
        imposed_flags::of(
            [
                self.used_bias(),
                self.dark.as_ref(),
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
        let dimensions = Sensor::shared(
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

        let flat = subtracted_flat.map(PreparedFlat::new).transpose()?;
        if cancel.is_cancelled() {
            return Err(CalibrationError::Cancelled);
        }

        Self::builder()
            .bias(bias)
            .dark(dark)
            .flat(flat)
            .defect_map(defect_map)
            .build()
    }

    /// Every present master lost no more to calibration than its role can: the check `from_images`
    /// runs on its inputs, for a bundle `fits::load` read. A flat is stored prepared, after it lost
    /// what it held of the additive parts.
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
        let dark_holds_bias = self.dark_holds_bias();
        let bias = self.used_bias();
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
            self.dark.as_ref().map(|master| Subtractor {
                role: MasterRole::Dark,
                master,
            }),
        ];
        let subtraction = Subtraction::resolve(image, subtractors.into_iter().flatten())?;
        let bias = subtraction.term(MasterRole::Bias);
        let dark = subtraction.term(MasterRole::Dark);
        outcome.dark = dark
            .and_then(|dark| dark.matched)
            .map(|matched| matched.comparison);
        let bias_term = bias.map(|bias| bias.subtracted(1.0));
        // A bias-removed dark is scaled, by the fit where its hot pixels allow; one that holds the
        // bias is subtracted as it is, the exposures having matched.
        outcome.dark_scale = dark
            .filter(|_| !dark_holds_bias)
            .map(|dark| match &self.defect_map {
                Some(defects) => DarkScale::fit(
                    defects.hot(),
                    FitLight {
                        light: image,
                        bias: bias_term,
                        imposed: self.imposed.as_ref(),
                        dark_gain: dark.map.gain,
                    },
                    dark.matched_scale(),
                ),
                None => DarkScale {
                    factor: dark.matched_scale(),
                    source: DarkScaleSource::Exposure { fit_pixels: 0 },
                },
            });
        let dark_term = dark.map(|dark| {
            dark.subtracted(
                outcome
                    .dark_scale
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
            // The flat divides a normalized copy of itself, so its own scale cancels; only a
            // different stated unit disqualifies it. The subtracted master's scale is checked where
            // it is converted (`Subtractor::map_into`). Only when both declare a domain: a
            // synthesized master has none, and refusing on that would reject every in-memory
            // fixture.
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

/// The masters [`CalibrationMasters::builder`] gathers, each optional, until [`Self::build`]
/// checks them as one set.
#[derive(Debug, Default)]
pub(crate) struct MastersBuilder {
    bias: Option<CfaImage>,
    dark: Option<CfaImage>,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
}

impl MastersBuilder {
    #[must_use]
    pub(crate) fn bias(self, bias: impl Into<Option<CfaImage>>) -> Self {
        Self {
            bias: bias.into(),
            ..self
        }
    }

    #[must_use]
    pub(crate) fn dark(self, dark: impl Into<Option<CfaImage>>) -> Self {
        Self {
            dark: dark.into(),
            ..self
        }
    }

    #[must_use]
    pub(crate) fn flat(self, flat: impl Into<Option<PreparedFlat>>) -> Self {
        Self {
            flat: flat.into(),
            ..self
        }
    }

    #[must_use]
    pub(crate) fn defect_map(self, defect_map: impl Into<Option<DefectMap>>) -> Self {
        Self {
            defect_map: defect_map.into(),
            ..self
        }
    }

    /// The masters as one coherent bundle, checked as [`CalibrationMasters::from_images`] checks
    /// its inputs, with the flags they impose on a light gathered.
    ///
    /// # Errors
    /// [`CalibrationError`] when the masters or the map do not describe one sensor, or a master
    /// lost more to calibration than its role can.
    pub(crate) fn build(self) -> Result<CalibrationMasters, CalibrationError> {
        let Self {
            bias,
            dark,
            flat,
            defect_map,
        } = self;
        let mut masters = CalibrationMasters {
            bias,
            dark,
            flat,
            defect_map,
            imposed: None,
        };
        // `from_images` checks its inputs before it touches them, so this re-checks a set built
        // some other way, a bundle read as independent HDUs: no `CalibrationMasters` spans two
        // sensors, so no bundle written is one a read refuses.
        Sensor::shared(masters.sensors())?;
        masters.validate_records()?;
        masters.imposed = masters.imposed_flags();
        Ok(masters)
    }
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

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::calibration::calibration_masters::{CalibrationMasters, MasterBuild};
    use crate::calibration::calibration_set::CalibrationSet;
    use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
    use crate::calibration::master_cache::NoMasterCache;
    use crate::ingest::ingest_config::IngestConfig;
    use crate::progress::progress_monitor::ProgressMonitor;

    /// [`CalibrationMasters::build`] with no cache, on the default ingest, in a test that expects
    /// it to succeed.
    pub(crate) fn masters_from_files<P: AsRef<Path>>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: SigmaThreshold,
    ) -> CalibrationMasters {
        CalibrationMasters::build(
            MasterBuild {
                frames,
                sigma_threshold,
                cache: &NoMasterCache,
            },
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .expect("build the calibration masters")
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

#[cfg(test)]
mod tests;
