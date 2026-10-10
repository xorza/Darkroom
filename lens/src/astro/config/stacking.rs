//! The editable projections behind the star-detection, registration and
//! stacking builder nodes.
//!
//! Unlike the per-frame configs — which derive
//! [`Introspect`] on the lumos type itself and need nothing
//! here but an identity — each of these fronts a *nested* config whose full
//! field set is far wider than a node's worth of ports. So each is a flat
//! projection: the handful of knobs the editor offers, expanded back over
//! `Default` on the way out. They deliberately do not track their lumos type
//! field-for-field, and the round-trip tests below pin the subset they do
//! carry.

use common::{Introspect, IntrospectEnum};
use lumos::detection::{self, FwhmMode};
use lumos::{Combine, Normalization, RegistrationConfig, SipConfig, StackConfig, Weighting};

use crate::astro::config::preset::Preset;

/// Lumos's star-detection presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "5f3563ab-f65d-4ed2-99ad-c63b9d3377ba")]
pub(crate) enum DetectionPreset {
    WideField,
    HighResolution,
    CrowdedField,
    PreciseGround,
}

impl Preset for DetectionPreset {
    type Knobs = DetectionKnobs;
    type Config = detection::Config;

    fn config(self) -> detection::Config {
        match self {
            Self::WideField => detection::Config::wide_field(),
            Self::HighResolution => detection::Config::high_resolution(),
            Self::CrowdedField => detection::Config::crowded_field(),
            Self::PreciseGround => detection::Config::precise_ground(),
        }
    }
}

/// Lumos's registration presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "7f6cfead-d076-4529-9a11-5f4da539168d")]
pub(crate) enum RegistrationPreset {
    Default,
    Fast,
    Precise,
    WideField,
    Mosaic,
}

impl Preset for RegistrationPreset {
    type Knobs = RegistrationKnobs;
    type Config = RegistrationConfig;

    fn config(self) -> RegistrationConfig {
        match self {
            Self::Default => RegistrationConfig::default(),
            Self::Fast => RegistrationConfig::fast(),
            Self::Precise => RegistrationConfig::precise(),
            Self::WideField => RegistrationConfig::wide_field(),
            Self::Mosaic => RegistrationConfig::mosaic(),
        }
    }
}

/// The star-detection knobs the editor offers, drawn from
/// [`detection::Config`]'s `detection`, `fwhm` and `filter` sub-configs.
#[derive(Debug, Clone, Introspect)]
#[config(
    type_id = "4512544e-537c-4c1c-96ad-e596cc88d60d",
    name = "DetectionConfig"
)]
pub(crate) struct DetectionKnobs {
    sigma_threshold: f32,
    expected_fwhm: f32,
    min_area: usize,
    max_area: usize,
    min_snr: f32,
    max_eccentricity: f32,
}

impl Default for DetectionKnobs {
    fn default() -> Self {
        detection::Config::default().into()
    }
}

impl From<detection::Config> for DetectionKnobs {
    fn from(config: detection::Config) -> Self {
        Self {
            sigma_threshold: config.detection.sigma_threshold,
            expected_fwhm: config
                .fwhm
                .mode
                .or(detection::Config::default().fwhm.mode)
                .expect("the default config runs a matched filter")
                .seed(),
            min_area: config.detection.min_area,
            max_area: config.detection.max_area,
            min_snr: config.filter.min_snr,
            max_eccentricity: config.filter.max_eccentricity,
        }
    }
}

impl From<DetectionKnobs> for detection::Config {
    fn from(knobs: DetectionKnobs) -> Self {
        let mut config = detection::Config::default();
        config.detection.sigma_threshold = knobs.sigma_threshold;
        config.fwhm.mode = Some(FwhmMode::Fixed(knobs.expected_fwhm));
        config.detection.min_area = knobs.min_area;
        config.detection.max_area = knobs.max_area;
        config.filter.min_snr = knobs.min_snr;
        config.filter.max_eccentricity = knobs.max_eccentricity;
        config
    }
}

/// The registration knobs the editor offers. `sip_enabled` stands in for
/// [`RegistrationConfig::sip`]'s whole `Option<SipConfig>`: on means the
/// default SIP fit, off means none.
#[derive(Debug, Clone, Introspect)]
#[config(
    type_id = "63cd4de9-b82f-4829-bea5-391da64e296f",
    name = "RegistrationConfig"
)]
pub(crate) struct RegistrationKnobs {
    max_stars: usize,
    min_matches: usize,
    ratio_tolerance: f64,
    ransac_iterations: usize,
    max_rms_error: f64,
    sip_enabled: bool,
}

impl Default for RegistrationKnobs {
    fn default() -> Self {
        RegistrationConfig::default().into()
    }
}

impl From<RegistrationConfig> for RegistrationKnobs {
    fn from(config: RegistrationConfig) -> Self {
        Self {
            max_stars: config.matching.max_stars,
            min_matches: config.matching.min_matches,
            ratio_tolerance: config.matching.triangle.ratio_tolerance,
            ransac_iterations: config.ransac.max_iterations,
            max_rms_error: config.max_rms_error,
            sip_enabled: config.sip.is_some(),
        }
    }
}

impl From<RegistrationKnobs> for RegistrationConfig {
    fn from(knobs: RegistrationKnobs) -> Self {
        let mut config = RegistrationConfig::default();
        config.matching.max_stars = knobs.max_stars;
        config.matching.min_matches = knobs.min_matches;
        config.matching.triangle.ratio_tolerance = knobs.ratio_tolerance;
        config.ransac.max_iterations = knobs.ransac_iterations;
        config.max_rms_error = knobs.max_rms_error;
        config.sip = knobs.sip_enabled.then(SipConfig::default);
        config
    }
}

/// Which combination [`CombineKnobs`] builds. A [`Combine`] carries each
/// method's parameters in its own shape, so the editor picks the method here
/// and supplies the one shared parameter — `sigma` — as its own field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "0ac16ec1-4a1e-48e9-aff5-17df1ff645bc")]
pub(crate) enum CombineMethodChoice {
    SigmaClipped,
    Winsorized,
    Median,
    Mean,
}

impl Preset for CombineMethodChoice {
    type Knobs = CombineKnobs;
    type Config = StackConfig;

    fn config(self) -> StackConfig {
        CombineKnobs {
            method: self,
            ..Default::default()
        }
        .into()
    }
}

/// How the lights are put on one scale before they combine: lumos's
/// [`Normalization`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "a784ccc5-0139-4a2b-93ec-902d6212539d")]
pub(crate) enum NormalizationChoice {
    Global,
    Multiplicative,
    None,
}

impl From<NormalizationChoice> for Normalization {
    fn from(choice: NormalizationChoice) -> Self {
        match choice {
            NormalizationChoice::Global => Self::Global,
            NormalizationChoice::Multiplicative => Self::Multiplicative,
            NormalizationChoice::None => Self::None,
        }
    }
}

/// How much each light counts in the combine: lumos's [`Weighting`], less the
/// manual weights a graph has no frame list to give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "a1f9be81-a6f8-44a6-b9c4-7a558e98d1df")]
pub(crate) enum WeightingChoice {
    Noise,
    Equal,
}

impl From<WeightingChoice> for Weighting {
    fn from(choice: WeightingChoice) -> Self {
        match choice {
            WeightingChoice::Noise => Self::Noise,
            WeightingChoice::Equal => Self::Equal,
        }
    }
}

/// The frame-combination knobs the editor offers. `sigma` is read only by the
/// two rejecting methods; `normalization` and `weighting` are the lights'
/// policy, whatever the method.
#[derive(Debug, Clone, Introspect)]
#[config(
    type_id = "843bff16-61ec-47db-9a86-64bb53c9c1cc",
    name = "CombineConfig"
)]
pub(crate) struct CombineKnobs {
    method: CombineMethodChoice,
    sigma: f32,
    normalization: NormalizationChoice,
    weighting: WeightingChoice,
}

impl Default for CombineKnobs {
    /// Sigma-clipped at 3σ, should a rejecting method be picked, with the
    /// policy of [`StackConfig::light`]: global normalization, noise weights.
    fn default() -> Self {
        Self {
            method: CombineMethodChoice::SigmaClipped,
            sigma: 3.0,
            normalization: NormalizationChoice::Global,
            weighting: WeightingChoice::Noise,
        }
    }
}

impl From<CombineKnobs> for StackConfig {
    fn from(knobs: CombineKnobs) -> Self {
        let combine = match knobs.method {
            CombineMethodChoice::SigmaClipped => Combine::sigma_clipped(knobs.sigma),
            CombineMethodChoice::Winsorized => Combine::winsorized(knobs.sigma),
            CombineMethodChoice::Median => Combine::median(),
            CombineMethodChoice::Mean => Combine::mean(),
        };
        Self {
            combine,
            normalization: knobs.normalization.into(),
            weighting: knobs.weighting.into(),
            ..Self::light()
        }
    }
}

#[cfg(test)]
mod tests {
    use lumos::detection::{self, FwhmMode};
    use lumos::{Combine, Normalization, RegistrationConfig, SipConfig, StackConfig, Weighting};

    use crate::astro::config::stacking::{
        CombineKnobs, CombineMethodChoice, DetectionKnobs, NormalizationChoice, RegistrationKnobs,
        WeightingChoice,
    };

    /// A projection is only safe if every knob writes back to the field it read
    /// from. Round-tripping a config whose knobs all hold *distinct* values
    /// proves that pairing for all of them at once: a knob wired to the wrong
    /// field carries the wrong number back and the assertion names it.
    #[test]
    fn detection_knobs_write_back_the_fields_they_read() {
        let mut config = detection::Config::default();
        config.detection.sigma_threshold = 4.5;
        config.detection.min_area = 7;
        config.detection.max_area = 900;
        config.fwhm.mode = Some(FwhmMode::Fixed(3.25));
        config.filter.min_snr = 12.5;
        config.filter.max_eccentricity = 0.75;

        let restored: detection::Config = DetectionKnobs::from(config.clone()).into();
        assert_eq!(restored.detection.sigma_threshold, 4.5);
        assert_eq!(restored.detection.min_area, 7);
        assert_eq!(restored.detection.max_area, 900);
        assert_eq!(restored.fwhm.mode, Some(FwhmMode::Fixed(3.25)));
        assert_eq!(restored.filter.min_snr, 12.5);
        assert_eq!(restored.filter.max_eccentricity, 0.75);
    }

    #[test]
    fn registration_knobs_write_back_the_fields_they_read() {
        let mut config = RegistrationConfig::default();
        config.matching.max_stars = 250;
        config.matching.min_matches = 9;
        config.matching.triangle.ratio_tolerance = 0.125;
        config.ransac.max_iterations = 1500;
        config.max_rms_error = 0.75;

        let restored: RegistrationConfig = RegistrationKnobs::from(config.clone()).into();
        assert_eq!(restored.matching.max_stars, 250);
        assert_eq!(restored.matching.min_matches, 9);
        assert_eq!(restored.matching.triangle.ratio_tolerance, 0.125);
        assert_eq!(restored.ransac.max_iterations, 1500);
        assert_eq!(restored.max_rms_error, 0.75);
    }

    /// The one knob that stands for a whole sub-config rather than a field:
    /// on restores the default SIP fit, off leaves none.
    #[test]
    fn registration_sip_flag_stands_for_the_whole_sub_config() {
        let without = RegistrationConfig {
            sip: None,
            ..RegistrationConfig::default()
        };
        let off: RegistrationConfig = RegistrationKnobs::from(without).into();
        assert!(off.sip.is_none());

        let with = RegistrationConfig {
            sip: Some(SipConfig::default()),
            ..RegistrationConfig::default()
        };
        let on: RegistrationConfig = RegistrationKnobs::from(with).into();
        assert!(on.sip.is_some());
    }

    /// Each choice builds the combination it names, `sigma` reaches the two
    /// methods that reject on it, and the policy knobs reach the lights'
    /// normalization and weighting, which default to [`StackConfig::light`]'s.
    #[test]
    fn each_combine_knob_reaches_its_field() {
        let knobs = |method| CombineKnobs {
            method,
            sigma: 2.5,
            ..CombineKnobs::default()
        };
        for (method, combine) in [
            (
                CombineMethodChoice::SigmaClipped,
                Combine::sigma_clipped(2.5),
            ),
            (CombineMethodChoice::Winsorized, Combine::winsorized(2.5)),
            (CombineMethodChoice::Median, Combine::median()),
            (CombineMethodChoice::Mean, Combine::mean()),
        ] {
            assert_eq!(
                StackConfig::from(knobs(method)),
                StackConfig {
                    combine,
                    ..StackConfig::light()
                },
                "{method:?}"
            );
        }
        assert_ne!(
            StackConfig::from(knobs(CombineMethodChoice::SigmaClipped)).combine,
            Combine::sigma_clipped(3.5),
            "sigma has to reach the rejecting methods"
        );

        let unscaled = StackConfig::from(CombineKnobs {
            normalization: NormalizationChoice::None,
            weighting: WeightingChoice::Equal,
            ..CombineKnobs::default()
        });
        assert_eq!(
            (unscaled.normalization, unscaled.weighting),
            (Normalization::None, Weighting::Equal)
        );
        let flat_scaled = StackConfig::from(CombineKnobs {
            normalization: NormalizationChoice::Multiplicative,
            ..CombineKnobs::default()
        });
        assert_eq!(flat_scaled.normalization, Normalization::Multiplicative);
    }
}
