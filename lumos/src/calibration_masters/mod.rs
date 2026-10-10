//! Calibration master frame creation and management.

pub(crate) mod calibration_component;
pub(crate) mod calibration_outcome;
pub(crate) mod calibration_set;
pub(crate) mod cosmic_ray;
pub(crate) mod dark_match;
pub(crate) mod defect_map;
pub(crate) mod error;
mod fits;
pub(crate) mod master_role;
pub(crate) mod master_subtraction;
pub(crate) mod prepared_flat;

use std::io;
use std::path::Path;

use common::CancelToken;

use crate::calibration_masters::dark_match::DarkMatch;
use crate::calibration_masters::defect_map::DefectMap;
use crate::calibration_masters::error::CalibrationError;
use crate::calibration_masters::master_subtraction::{MasterSubtraction, Subtractor};
use crate::combine::cache::FrameCache;
use crate::combine::config::StackConfig;
use crate::combine::error::StackError;
use crate::combine::stack::combine_cached;
use crate::frame_store::capture_conditions::CaptureConditions;
use crate::ingest::frame_step::FrameStep;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::cfa::CfaImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::sample_domain::{DomainMap, Pedestal, SampleDomain};
use crate::io::image::unverified_conditions::UnverifiedConditions;
use crate::math::size2us::Size2us;
use crate::progress::progress_callback::ProgressCallback;
use crate::stack_product::quality_planes::QualityPlanes;

use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::calibration_masters::calibration_set::CalibrationSet;
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
    /// The dark, its record saying whether it lost the bias.
    dark: Option<CfaImage>,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
}

/// Stack one calibration role's raw CFA frames into a single master, under `config` — the
/// role's preset is [`MasterRole::stack_config`] — read as `ingest` says. Returns `None` if `paths`
/// is empty.
///
/// `subtract`, when given, is taken from every frame before its statistics and the combine, and
/// each frame's record of what calibration removed takes what the subtractor still held: flats take
/// their flat-dark or bias this way, so the multiplicative normalization scales each flat's own
/// signal. Scaling a flat that still holds its offset `b` and subtracting the offset from the
/// master afterwards leaves `b·(mean gain − 1)`, a vignetting residual of a few percent, as
/// PixInsight and Siril avoid by calibrating each flat first. A subtractor holding dark signal is
/// matched to each frame's exposure and temperature as a dark is to a light.
///
/// The master states the exposure and temperature its frames share. The frames of a dark or a
/// flat-dark have to share them: its signal is one exposure's at one temperature.
///
/// The preset carries its own small-frame fallback (`Combine::small_n`): the combine engine
/// downgrades to the median below the preset's `min_frames` (e.g. `flat()` below 8), so no
/// frame-count special-casing is needed here. Stack each role this way, then assemble the set
/// with [`CalibrationMasters::from_images`].
///
/// # Errors
///
/// A [`StackError`]: the subtractor does not fit the role, a frame does not fit the subtractor, a
/// dark's frames disagree on their exposure or temperature, or the stack itself fails.
pub fn stack_cfa_master(
    paths: &[impl AsRef<Path> + Sync],
    role: MasterRole,
    config: StackConfig,
    ingest: &IngestConfig,
    subtract: Option<Subtractor<'_>>,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<Option<CfaImage>, StackError> {
    // `None` rather than `StackError::NoFrames`: an absent calibration role is normal, and this is
    // the one thing `combine_cached` cannot decide for us.
    if paths.is_empty() {
        return Ok(None);
    }
    let subtraction = subtract
        .map(|subtractor| MasterSubtraction::new(role, subtractor))
        .transpose()?;
    let run = IngestRun::new(ingest, cancel);
    // A master is mosaic data for the calibration stage to consume, not a science product: the
    // ancillary planes would be allocated and written per pixel for nothing.
    let config = StackConfig {
        quality: QualityPlanes::IMAGE_ONLY,
        ..config
    };
    let step = subtraction
        .as_ref()
        .map(|step| step as &dyn FrameStep<CfaImage>);
    let product = combine_cached(&config, paths.len(), "cfa paths", || {
        let cache = FrameCache::from_cfa_paths(paths, &config, run, step, progress)?;
        if role.is_dark() {
            CaptureConditions::check_agreement(
                cache
                    .frames
                    .iter()
                    .map(|frame| frame.source_stats.facts.conditions),
            )
            .map_err(|source| StackError::MasterConditions { role, source })?;
        }
        Ok(cache)
    })?;

    Ok(Some(product.into_cfa_master()))
}

impl CalibrationMasters {
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
        self.defect_map.as_ref().map(|map| DefectSummary {
            hot_pixels: map.hot_indices().len(),
            cold_pixels: map.cold_indices().len(),
            percentage: map.percentage(),
        })
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

    /// Load a bundle written by [`Self::save`] without rebuilding its prepared flat or defect map,
    /// under `context`: its cancellation, which reads as [`io::ErrorKind::Interrupted`], and its
    /// memory limit on each master's decode.
    pub fn load(path: &Path, context: &LoadContext) -> io::Result<Self> {
        fits::load(path, context)
    }

    /// Create `CalibrationMasters` from pre-built CFA images.
    ///
    /// Each master's record of what calibration removed says what it still holds. With a bias in
    /// the set, the dark and the flat-dark lose their bias, so each keeps the dark signal alone,
    /// which scales with exposure. The flat then loses what it still holds of the two additive
    /// parts, matched to its exposure and temperature: the dark signal by the flat-dark, and the
    /// bias by the bias, or by a flat-dark that still holds it. A flat stacked with its subtractor
    /// taken from each frame ([`stack_cfa_master`]) already lost those parts, and does not lose
    /// them again, and what its stack's match did not compare stays on it beside what this one did
    /// not ([`Self::unverified_flat_dark`]). Cold pixels are detected on that flat, hot pixels on the
    /// dark as given.
    /// `sigma_threshold` controls defect detection sensitivity (see [`DEFAULT_SIGMA_THRESHOLD`]).
    ///
    /// # Errors
    ///
    /// A [`CalibrationError`] when the masters do not describe one sensor, a master lost more to
    /// calibration than its role can, the flat-dark does not match the flat or would remove a
    /// part the flat already lost, the flat has no positive mean to normalize by or holds an offset
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
                        subtract_master(&mut master, bias, MasterRole::Bias, None)?;
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
                            holds.bias,
                        )
                        .map_err(|source| {
                            CalibrationError::DarkMismatch {
                                component: MasterRole::FlatDark,
                                source,
                            }
                        })?;
                        subtract_master(&mut flat, flat_dark, MasterRole::FlatDark, Some(matched))?;
                    }
                }
                if !flat.metadata.calibration.bias {
                    if let Some(bias) = &bias {
                        subtract_master(&mut flat, bias, MasterRole::Bias, None)?;
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

        Ok(Self {
            bias,
            dark,
            flat,
            defect_map,
        })
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
    ///    within 1 °C. A bias-removed dark of another exposure is scaled by the light's exposure
    ///    over its own; one that holds the bias is refused. A fact one side does not declare is
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
        let dark_holds_bias = self
            .dark
            .as_ref()
            .is_some_and(|dark| !dark.metadata.calibration.bias);
        // The bias is subtracted on its own unless the dark still holds it.
        let bias = self.bias.as_ref().filter(|_| !dark_holds_bias);
        if self.flat.is_some() && bias.is_none() && !dark_holds_bias && holds_offset(image) {
            return Err(CalibrationError::LightWithoutSubtractor);
        }

        // Each master's map is taken against the domain the light will have when that master is
        // applied: subtracting the bias moves the light's pedestal, which the dark's map has to
        // see, or the pedestal would be removed a second time.
        let mut domain = image.metadata.domain.clone();
        let bias = bias
            .map(|bias| -> Result<_, CalibrationError> {
                let map = master_map(domain.as_ref(), bias, MasterRole::Bias)?;
                after_subtracting(&mut domain, bias);
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
                    dark_holds_bias,
                )
                .map_err(|source| CalibrationError::DarkMismatch {
                    component: MasterRole::Dark,
                    source,
                })?;
                outcome.unverified = matched.unverified;
                outcome.dark_scale = matched.scale;
                let map = master_map(domain.as_ref(), dark, MasterRole::Dark)?;
                after_subtracting(&mut domain, dark);
                Ok((dark, map, matched))
            })
            .transpose()?;

        image.record_saturation();
        if let Some((bias, map)) = bias {
            remove_master(image, bias, MasterRole::Bias, map, None);
        }
        if let Some((dark, map, matched)) = dark {
            remove_master(image, dark, MasterRole::Dark, map, Some(matched));
        }
        debug_assert_eq!(image.metadata.domain, domain);
        if let Some(flat) = &self.flat {
            flat.apply(image);
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
            // it is converted (`master_map`). Only when both declare a domain: a synthesized
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

/// `master` lost no more to calibration than a master in `role` can.
const fn check_record(role: MasterRole, master: &CfaImage) -> Result<(), CalibrationError> {
    if role.may_have_lost().contains(master.metadata.calibration) {
        Ok(())
    } else {
        Err(CalibrationError::OverCalibratedMaster { component: role })
    }
}

/// The map that expresses `master`'s samples in the domain `frame` states, for the master in
/// `role`.
///
/// The identity when either declares no domain: a synthesized frame has none, and there is nothing
/// to convert between. An error when the two cannot be related exactly — a different stated unit, a
/// span the decoder had to assume, or a known pedestal against an unknown one — because subtracting
/// across such a gap is silently wrong.
fn master_map(
    frame: Option<&SampleDomain>,
    master: &CfaImage,
    role: MasterRole,
) -> Result<DomainMap, CalibrationError> {
    let (Some(frame), Some(master_domain)) = (frame, &master.metadata.domain) else {
        return Ok(DomainMap::IDENTITY);
    };
    master_domain
        .conversion_to(frame)
        .ok_or_else(|| CalibrationError::SampleDomainMismatch {
            component: role,
            frame: frame.clone(),
            master: master_domain.clone(),
        })
}

/// Move `domain`'s pedestal to where subtracting `master` leaves it, as
/// [`CfaImage::subtract`] does.
const fn after_subtracting(domain: &mut Option<SampleDomain>, master: &CfaImage) {
    if let (Some(domain), Some(master)) = (domain, &master.metadata.domain) {
        domain.pedestal = domain.after_subtracting(master);
    }
}

/// [`remove_master`] through the map from `master`'s domain to `frame`'s current one.
fn subtract_master(
    frame: &mut CfaImage,
    master: &CfaImage,
    role: MasterRole,
    matched: Option<DarkMatch>,
) -> Result<(), CalibrationError> {
    let map = master_map(frame.metadata.domain.as_ref(), master, role)?;
    remove_master(frame, master, role, map, matched);
    Ok(())
}

/// Subtract `master`, in `role`, from `frame` through `map`, its dark signal scaled as `matched`
/// says, and record in `frame` what it held and what the match did not compare. Every removal of
/// a master goes through here, so neither record can be left behind.
fn remove_master(
    frame: &mut CfaImage,
    master: &CfaImage,
    role: MasterRole,
    map: DomainMap,
    matched: Option<DarkMatch>,
) {
    let removes = role.signal().without(master.metadata.calibration);
    debug_assert!(
        !frame.metadata.calibration.overlaps(removes),
        "{role} would remove {removes:?} from a frame that lost {:?}",
        frame.metadata.calibration
    );
    debug_assert_eq!(
        matched.is_some(),
        removes.thermal,
        "{role}: a master is matched to the frame exactly when it holds dark signal"
    );
    frame.subtract_scaled(master, map, matched.map_or(1.0, DarkMatch::factor));
    frame.metadata.calibration = frame.metadata.calibration.union(removes);
    if let Some(matched) = matched {
        frame.metadata.unverified_dark = frame.metadata.unverified_dark.union(matched.unverified);
    }
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use common::CancelToken;

    use crate::calibration_masters::calibration_set::CalibrationSet;

    use crate::calibration_masters::master_role::MasterRole;
    use crate::calibration_masters::master_subtraction::Subtractor;
    use crate::calibration_masters::{CalibrationMasters, stack_cfa_master};
    use crate::ingest::ingest_config::IngestConfig;
    use crate::progress::progress_callback::ProgressCallback;

    /// Every role stacked under its preset, the flats with their flat-dark or bias taken from each
    /// frame, then the set assembled — what a caller does role by role.
    pub(crate) fn masters_from_files<P: AsRef<Path> + Sync>(
        frames: CalibrationSet<&[P]>,
        sigma_threshold: f32,
    ) -> CalibrationMasters {
        let stack = |paths: &[P], role: MasterRole, subtract: Option<Subtractor<'_>>| {
            stack_cfa_master(
                paths,
                role,
                role.stack_config(),
                &IngestConfig::default(),
                subtract,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .expect("stack a calibration master")
        };
        let bias = stack(frames.bias, MasterRole::Bias, None);
        let flat_dark = stack(frames.flat_dark, MasterRole::FlatDark, None);
        let subtractor = match (&flat_dark, &bias) {
            (Some(master), _) => Some(Subtractor {
                role: MasterRole::FlatDark,
                master,
            }),
            (None, Some(master)) => Some(Subtractor {
                role: MasterRole::Bias,
                master,
            }),
            (None, None) => None,
        };
        let flat = stack(frames.flat, MasterRole::Flat, subtractor);
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
