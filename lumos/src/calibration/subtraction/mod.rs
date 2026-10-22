//! [`Subtraction`]: the additive masters taken from one frame, resolved against it, then removed
//! in one pass.

use arrayvec::ArrayVec;

use crate::calibration::calibration_pass::{CalibrationPass, Saturation, Subtracted};
use crate::calibration::dark_match::{DarkBias, DarkMatch};
use crate::calibration::error::SubtractionFault;
use crate::calibration::master_role::MasterRole;
use crate::calibration::subtractor::Subtractor;
use crate::frame::capture_conditions::CaptureConditions;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::{DomainMap, SampleDomain};

/// The additive masters taken from one frame, in order, resolved against it: each one's map into
/// the frame's domain as it stands when that master applies — a bias moves the pedestal the next
/// one's map sees — and, for one that holds dark signal, its match to the frame.
///
/// A light, a frame of a calibration stack, and a master the set is built from lose their
/// additive parts through this one step: the bias as it stands, then a bias-removed dark scaled to
/// the frame.
#[derive(Debug)]
pub(crate) struct Subtraction<'a> {
    terms: ArrayVec<Term<'a>, 2>,
    /// The frame's domain once every term is removed.
    domain: Option<SampleDomain>,
}

/// One master of a [`Subtraction`], resolved against the frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Term<'a> {
    pub(crate) subtractor: Subtractor<'a>,
    /// The master's samples in the frame's domain, as the frame stands when it applies.
    pub(crate) map: DomainMap,
    /// The master's match to the frame, when it holds dark signal.
    pub(crate) matched: Option<DarkMatch>,
}

impl<'a> Term<'a> {
    /// The master's term in the pass, its signal scaled by `scale`.
    pub(crate) fn subtracted(&self, scale: f64) -> Subtracted<'a> {
        Subtracted {
            samples: self.subtractor.master.data().pixels(),
            map: self.map,
            scale,
        }
    }

    /// The scale its match gives the master's signal: the frame's exposure over the master's, 1
    /// for a master of no dark signal or one taken at the frame's exposure.
    pub(super) fn matched_scale(&self) -> f64 {
        self.matched.map_or(1.0, DarkMatch::factor)
    }
}

impl<'a> Subtraction<'a> {
    /// `subtractors`, in order, resolved against `frame`.
    ///
    /// # Errors
    ///
    /// A [`SubtractionFault`]: a master that holds dark signal was taken under other conditions
    /// than the frame, or a master's domain cannot be expressed in the frame's.
    pub(crate) fn resolve(
        frame: &CfaImage,
        subtractors: impl IntoIterator<Item = Subtractor<'a>>,
    ) -> Result<Self, SubtractionFault> {
        let conditions = CaptureConditions::of(&frame.metadata);
        let mut domain = frame.metadata.derived.domain().cloned();
        let mut terms = ArrayVec::new();
        for subtractor in subtractors {
            let master = subtractor.master;
            let removes = subtractor.removes();
            let matched = removes
                .thermal
                .then(|| {
                    DarkMatch::new(
                        conditions,
                        CaptureConditions::of(&master.metadata),
                        DarkBias::held_in(removes),
                    )
                })
                .transpose()
                .map_err(|source| SubtractionFault::Conditions {
                    role: subtractor.role,
                    source,
                })?;
            let map = subtractor.map_into(domain.as_ref())?;
            if let (Some(domain), Some(master)) = (&mut domain, master.metadata.derived.domain()) {
                domain.pedestal = domain.after_subtracting(master);
            }
            terms.push(Term {
                subtractor,
                map,
                matched,
            });
        }
        Ok(Self { terms, domain })
    }

    /// The term of the master of `role`, when the subtraction takes one.
    pub(crate) fn term(&self, role: MasterRole) -> Option<&Term<'a>> {
        self.terms.iter().find(|term| term.subtractor.role == role)
    }

    /// Remove every term from `frame` in one pass, each signal scaled by its match, flagged
    /// `NO_DATA` where `imposed` says, its saturation as `saturation` says; then
    /// [record](Self::record) it.
    pub(crate) fn apply(
        self,
        frame: &mut CfaImage,
        imposed: Option<&PixelFlags>,
        saturation: Saturation,
    ) {
        CalibrationPass {
            subtracted: self
                .terms
                .iter()
                .map(|term| term.subtracted(term.matched_scale()))
                .collect(),
            flat: None,
            imposed,
            saturation,
        }
        .apply(frame);
        self.record(frame);
    }

    /// Record in `frame`, once its samples lost every term, what each master held and what its
    /// match did not compare, and the domain the frame now has.
    pub(crate) fn record(self, frame: &mut CfaImage) {
        for term in &self.terms {
            term.subtractor.record_removal(frame, term.matched);
        }
        frame.metadata.derived.set_domain(self.domain);
    }
}

/// `NO_DATA` wherever one of `subtractors` holds no measurement — a fill, or a saturated bound —
/// as a pass imposes it: the value subtracted there is not one either. `None` when none holds
/// such a pixel.
pub(crate) fn unmeasured(subtractors: &[Subtractor<'_>]) -> Option<PixelFlags> {
    let flags: ArrayVec<&PixelFlags, 2> = subtractors
        .iter()
        .filter_map(|subtractor| subtractor.master.flags())
        .filter(|flags| flags.contains(QualityFlags::UNMEASURED))
        .collect();
    let size = flags.first()?.size();
    PixelFlags::from_fn(size, |idx| {
        if flags
            .iter()
            .any(|flags| flags.at(idx).intersects(QualityFlags::UNMEASURED))
        {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    })
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::calibration::calibration_pass::Saturation;
    use crate::calibration::master_role::MasterRole;
    use crate::calibration::subtraction::{self, Subtraction};
    use crate::calibration::subtractor::Subtractor;
    use crate::image::cfa::cfa_image::CfaImage;

    /// Take `bias` from `master`, a master, as a set takes its bias: through the one step.
    pub(crate) fn subtract_bias(master: &mut CfaImage, bias: &CfaImage) {
        let subtractors = [Subtractor {
            role: MasterRole::Bias,
            master: bias,
        }];
        Subtraction::resolve(master, subtractors).unwrap().apply(
            master,
            subtraction::unmeasured(&subtractors).as_ref(),
            Saturation::Kept,
        );
    }
}

#[cfg(test)]
mod tests;
