//! [`DerivedFacts`]: what the pipeline knows of an image's samples, and how each step moves it.

use serde::{Deserialize, Serialize};

use crate::image::calibration_state::CalibrationState;
use crate::image::dark_comparison::DarkComparison;
use crate::image::image_provenance::{DemosaicProvenance, ImageProvenance, RowOrder};
use crate::image::mosaic_noise::MosaicNoise;
use crate::image::sample_domain::{SampleDomain, ScaleOrigin};

/// What the pipeline knows of an image's samples beyond the source's header: where they came from,
/// their domain, their noise facts and what calibration removed from them. A decoder states them;
/// a step that changes the samples moves them by one of the named transitions, which hold the
/// rules each fact follows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DerivedFacts {
    provenance: Option<ImageProvenance>,
    /// What one sample is worth in the source's own terms: the span its decoder divided by, the
    /// pedestal it still carries, and the unit.
    ///
    /// `None` for an image this crate synthesized rather than decoded, and for a preview raster
    /// that declared no domain. Two frames are commensurate when both answer and
    /// [`SampleDomain::conversion_to`] relates the answers; when either is `None` there is nothing
    /// to compare, which is not the same as agreeing. Calibration updates the pedestal when it
    /// subtracts a master.
    domain: Option<SampleDomain>,
    /// The uncertainty one quantization step adds to a sample, `step / √12`, in the samples' own
    /// units: the source's ADC step, a lower bound on any sample's noise before a flat divided it.
    ///
    /// Set by a decoder that knows the step — a RAW with a linear curve, an integer FITS — and by
    /// the combine for a master, which states the largest step of its inputs. Calibration leaves it
    /// as it is: subtracting a master adds no step to the light's own, and the flat's division
    /// scales it per pixel, which the noise model applies where it reads the flat. A demosaic, an
    /// interpolating warp and a drizzle clear it: each mixes samples, so the bound no longer
    /// describes one of them.
    quantization_sigma: Option<f32>,
    /// The white noise of the mosaic a demosaic made this frame from, which a measurement of the
    /// frame would understate. Set by the demosaic; `None` for any frame not demosaiced, and for a
    /// master, which the combine made.
    mosaic_noise: Option<MosaicNoise>,
    /// Whether every saturated pixel is flagged in the image's flags. A RAW decode and a FITS with
    /// a `DATAMAX` flag them, and calibration flags them before it moves the samples; anything else
    /// leaves a consumer to test the samples itself.
    saturation_flagged: bool,
    /// The parts of the signal calibration removed: set by `CalibrationMasters::calibrate` on a
    /// light, and by a calibration stack that subtracts a master from each frame. It guards
    /// against removing a part twice, and travels with the frame through demosaic.
    calibration: CalibrationState,
    /// What taking a master holding dark signal from this frame compared — a light's dark, a
    /// flat's flat-dark — or the union over every frame of the stack it is; `None` when none was
    /// taken.
    dark_comparison: Option<DarkComparison>,
}

/// The facts a decoder states of the samples it made, or the combine of a master: each unset one
/// is unknown, or absent.
#[derive(Debug, Clone, Default)]
pub(crate) struct DerivedFactsBuilder {
    facts: DerivedFacts,
}

impl DerivedFactsBuilder {
    pub(crate) fn provenance(mut self, provenance: ImageProvenance) -> Self {
        self.facts.provenance = Some(provenance);
        self
    }

    pub(crate) fn domain(mut self, domain: SampleDomain) -> Self {
        self.facts.domain = Some(domain);
        self
    }

    pub(crate) const fn quantization_sigma(mut self, sigma: Option<f32>) -> Self {
        self.facts.quantization_sigma = sigma;
        self
    }

    pub(crate) const fn mosaic_noise(mut self, noise: Option<MosaicNoise>) -> Self {
        self.facts.mosaic_noise = noise;
        self
    }

    pub(crate) const fn saturation_flagged(mut self, flagged: bool) -> Self {
        self.facts.saturation_flagged = flagged;
        self
    }

    pub(crate) const fn calibration(mut self, calibration: CalibrationState) -> Self {
        self.facts.calibration = calibration;
        self
    }

    pub(crate) const fn dark_comparison(mut self, comparison: Option<DarkComparison>) -> Self {
        self.facts.dark_comparison = comparison;
        self
    }

    pub(crate) fn build(self) -> DerivedFacts {
        self.facts
    }
}

impl DerivedFacts {
    /// The facts of samples nothing is known of yet, to state one by one.
    pub(crate) fn builder() -> DerivedFactsBuilder {
        DerivedFactsBuilder::default()
    }

    /// These facts, to restate some of: the combine's master starts from its reference frame's.
    pub(crate) const fn rebuild(self) -> DerivedFactsBuilder {
        DerivedFactsBuilder { facts: self }
    }

    pub const fn provenance(&self) -> Option<&ImageProvenance> {
        self.provenance.as_ref()
    }

    /// The provenance, to restate what a step made of the samples: their color, their demosaic,
    /// whether a load clipped them.
    pub(crate) const fn provenance_mut(&mut self) -> Option<&mut ImageProvenance> {
        self.provenance.as_mut()
    }

    pub const fn domain(&self) -> Option<&SampleDomain> {
        self.domain.as_ref()
    }

    /// Calibration's domain once it subtracted its masters, which it plans before it moves the
    /// samples.
    pub(crate) fn set_domain(&mut self, domain: Option<SampleDomain>) {
        self.domain = domain;
    }

    pub const fn quantization_sigma(&self) -> Option<f32> {
        self.quantization_sigma
    }

    pub const fn mosaic_noise(&self) -> Option<MosaicNoise> {
        self.mosaic_noise
    }

    pub const fn saturation_flagged(&self) -> bool {
        self.saturation_flagged
    }

    /// Once calibration flagged every saturated pixel, before it moved the samples.
    pub(crate) const fn mark_saturation_flagged(&mut self) {
        self.saturation_flagged = true;
    }

    pub const fn calibration(&self) -> CalibrationState {
        self.calibration
    }

    /// Once calibration removed the parts `removes` names, beside those it removed before.
    pub(crate) const fn record_removal(&mut self, removes: CalibrationState) {
        self.calibration = self.calibration.union(removes);
    }

    pub const fn dark_comparison(&self) -> Option<DarkComparison> {
        self.dark_comparison
    }

    /// Once a master holding dark signal was taken from the frame, compared as `comparison`.
    pub(crate) fn record_dark_comparison(&mut self, comparison: DarkComparison) {
        self.dark_comparison = Some(
            self.dark_comparison
                .map_or(comparison, |recorded| recorded.union(comparison)),
        );
    }

    /// After an edit of the samples in place: no noise fact describes the edited samples.
    pub(super) const fn after_edit(&mut self) {
        self.quantization_sigma = None;
        self.mosaic_noise = None;
    }

    /// After a resample that mixes source samples into each one, as an interpolating warp and a
    /// drizzle do: one sample's quantization step no longer bounds its noise. The mosaic's noise
    /// describes the source mosaic, which a resample leaves as it was.
    pub(crate) const fn after_resample(&mut self) {
        self.quantization_sigma = None;
    }

    /// After an interpolation across a mosaic's photosites, from `mosaic_noise`, the noise the
    /// mosaic was measured to hold where it was: each sample mixes neighbours, so one step's σ no
    /// longer bounds it, and the correlation hides part of their noise from a later measurement.
    pub(super) const fn after_interpolation(&mut self, mosaic_noise: Option<MosaicNoise>) {
        self.quantization_sigma = None;
        self.mosaic_noise = mosaic_noise;
    }

    /// Which end of the image the first stored row belongs to, or `None` for an image this crate
    /// synthesized rather than decoded.
    ///
    /// Two frames are the same view only when both answer and the answers match; `None` is "cannot
    /// tell", which is not the same as agreeing.
    pub fn row_order(&self) -> Option<RowOrder> {
        self.provenance
            .as_ref()
            .map(|provenance| provenance.row_order)
    }

    /// The ADU one unit of these samples is worth. Only a declared scale says it; an assumed one
    /// is a guess no gain can be applied through.
    pub(crate) fn adu_per_unit(&self) -> Option<f64> {
        self.domain
            .as_ref()
            .filter(|domain| domain.origin == ScaleOrigin::Declared)
            .map(|domain| domain.scale)
    }

    /// Whether these samples came out of a demosaic, and so carry its interpolation artifacts. A
    /// monochrome sensor's frame is copied straight through, a CFA drizzle interpolates no color
    /// from another, and a green proxy fills its other photosites from green alone.
    pub(crate) fn is_demosaiced(&self) -> bool {
        self.provenance
            .as_ref()
            .is_some_and(|provenance| match provenance.demosaic {
                DemosaicProvenance::LumosRcd
                | DemosaicProvenance::LumosMarkesteijn { .. }
                | DemosaicProvenance::LibRaw => true,
                DemosaicProvenance::None
                | DemosaicProvenance::CfaDrizzle
                | DemosaicProvenance::GreenProxy => false,
            })
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::image::calibration_state::CalibrationState;
    use crate::image::dark_comparison::DarkComparison;
    use crate::image::derived_facts::DerivedFacts;
    use crate::image::image_provenance::ImageProvenance;
    use crate::image::mosaic_noise::MosaicNoise;

    /// Each fact set directly, for a test that builds the state it starts from.
    impl DerivedFacts {
        pub(crate) fn set_provenance(&mut self, provenance: Option<ImageProvenance>) {
            self.provenance = provenance;
        }

        pub(crate) const fn set_quantization_sigma(&mut self, sigma: Option<f32>) {
            self.quantization_sigma = sigma;
        }

        pub(crate) const fn set_mosaic_noise(&mut self, noise: Option<MosaicNoise>) {
            self.mosaic_noise = noise;
        }

        pub(crate) const fn set_saturation_flagged(&mut self, flagged: bool) {
            self.saturation_flagged = flagged;
        }

        pub(crate) const fn set_calibration(&mut self, calibration: CalibrationState) {
            self.calibration = calibration;
        }

        pub(crate) const fn set_dark_comparison(&mut self, comparison: Option<DarkComparison>) {
            self.dark_comparison = comparison;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::image::bg_split::BgSplit;
    use crate::image::calibration_state::CalibrationState;
    use crate::image::derived_facts::DerivedFacts;
    use crate::image::mosaic_noise::MosaicNoise;

    /// An edit drops both noise facts, a resample the step alone, and an interpolation the step
    /// with the mosaic's noise taken from its argument. Each leaves the facts no noise rule names.
    #[test]
    fn each_transition_moves_only_the_noise_facts_its_rule_names() {
        let noise = |sigma: f32| MosaicNoise {
            bg: [BgSplit {
                variance: sigma * sigma,
                read_share: 0.5,
            }; 3],
            sky: [0.1; 3],
            quantization_sigma: Some(1e-5),
        };
        let decoded = DerivedFacts {
            quantization_sigma: Some(2e-5),
            mosaic_noise: Some(noise(0.01)),
            saturation_flagged: true,
            calibration: CalibrationState {
                bias: true,
                thermal: false,
                flat: true,
            },
            ..DerivedFacts::default()
        };
        let moved = |transition: &dyn Fn(&mut DerivedFacts)| {
            let mut facts = decoded.clone();
            transition(&mut facts);
            facts
        };

        let edited = moved(&DerivedFacts::after_edit);
        assert_eq!(edited.quantization_sigma, None);
        assert_eq!(edited.mosaic_noise, None);

        let resampled = moved(&DerivedFacts::after_resample);
        assert_eq!(resampled.quantization_sigma, None);
        assert_eq!(resampled.mosaic_noise, Some(noise(0.01)));

        for measured in [Some(noise(0.02)), None] {
            let interpolated = moved(&|facts| facts.after_interpolation(measured));
            assert_eq!(interpolated.quantization_sigma, None);
            assert_eq!(interpolated.mosaic_noise, measured);
        }

        for facts in [edited, resampled] {
            assert!(facts.saturation_flagged);
            assert_eq!(facts.calibration, decoded.calibration);
        }
    }
}
