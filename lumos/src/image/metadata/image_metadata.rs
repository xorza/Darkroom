//! [`ImageMetadata`]: what every image product carries besides its pixels.

use serde::{Deserialize, Serialize};

use crate::image::metadata::derived_facts::DerivedFacts;
use crate::image::metadata::observation_header::ObservationHeader;

/// What every image product carries besides its pixels: the source's header, and the facts the
/// pipeline derived of the samples.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub header: ObservationHeader,
    pub derived: DerivedFacts,
}

impl ImageMetadata {
    /// Electrons per unit of these samples: the camera's electrons per ADU times
    /// [`DerivedFacts::adu_per_unit`].
    pub(crate) fn electrons_per_unit(&self) -> Option<f32> {
        let egain = self
            .header
            .egain
            .filter(|egain| egain.is_finite() && *egain > 0.0)?;
        Some((egain * self.derived.adu_per_unit()?) as f32)
    }
}

#[cfg(test)]
mod tests {
    use crate::image::metadata::derived_facts::DerivedFacts;
    use crate::image::metadata::image_metadata::ImageMetadata;
    use crate::image::metadata::observation_header::ObservationHeader;
    use crate::image::metadata::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};

    /// 1.5 e⁻/ADU over a declared scale of 65535 ADU per unit is 98302.5 electrons per unit. An
    /// assumed scale, a missing domain or a non-positive gain give none.
    #[test]
    fn electrons_per_unit_needs_a_gain_and_a_declared_scale() {
        let domain = |origin| SampleDomain {
            scale: 65_535.0,
            origin,
            pedestal: Pedestal::Removed,
            unit: None,
        };
        let metadata = |egain, domain| ImageMetadata {
            header: ObservationHeader {
                egain,
                ..Default::default()
            },
            derived: {
                let mut facts = DerivedFacts::default();
                facts.set_domain(domain);
                facts
            },
        };
        assert_eq!(
            metadata(Some(1.5), Some(domain(ScaleOrigin::Declared))).electrons_per_unit(),
            Some(98_302.5)
        );
        for (egain, domain) in [
            (Some(1.5), Some(domain(ScaleOrigin::Assumed))),
            (Some(1.5), None),
            (Some(0.0), Some(domain(ScaleOrigin::Declared))),
            (None, Some(domain(ScaleOrigin::Declared))),
        ] {
            assert_eq!(metadata(egain, domain).electrons_per_unit(), None);
        }
    }

    /// A default record states nothing: no object, no white balance, no provenance.
    #[test]
    fn a_default_record_states_nothing() {
        let meta = ImageMetadata::default();
        assert!(meta.header.object.is_none());
        assert!(meta.header.camera_white_balance.is_none());
        assert!(meta.derived.provenance().is_none());
    }
}
