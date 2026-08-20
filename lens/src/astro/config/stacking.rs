//! The editable projections behind the star-detection, registration and
//! stacking builder nodes.
//!
//! Unlike the per-frame configs — which derive
//! [`Introspect`](common::Introspect) on the lumos type itself and need nothing
//! here but an identity — each of these fronts a *nested* config whose full
//! field set is far wider than a node's worth of ports. So each is a flat
//! projection: the handful of knobs the editor offers, expanded back over
//! `Default` on the way out. They deliberately do not track their lumos type
//! field-for-field, and the round-trip tests below pin the subset they do
//! carry.

use common::{Introspect, IntrospectEnum};
use lumos::{RegistrationConfig, SipConfig, StackConfig, StarDetectionConfig};

use crate::astro::config::preset::preset_enum;
use crate::config_node::NodeConfig;

const COMBINE_SIGMA: f32 = 3.0;

preset_enum! {
    DetectionPreset => StarDetectionConfig,
    display: "DetectionPreset",
    variants: {
        WideField = "wide_field" @ "Wide Field" => StarDetectionConfig::wide_field(),
        HighResolution = "high_resolution" @ "High Resolution" => StarDetectionConfig::high_resolution(),
        CrowdedField = "crowded_field" @ "Crowded Field" => StarDetectionConfig::crowded_field(),
        PreciseGround = "precise_ground" @ "Precise Ground" => StarDetectionConfig::precise_ground(),
    }
}

preset_enum! {
    RegistrationPreset => RegistrationConfig,
    display: "RegistrationPreset",
    variants: {
        Default = "default" @ "Default" => RegistrationConfig::default(),
        Fast = "fast" @ "Fast" => RegistrationConfig::fast(),
        Precise = "precise" @ "Precise" => RegistrationConfig::precise(),
        WideField = "wide_field" @ "Wide Field" => RegistrationConfig::wide_field(),
        Mosaic = "mosaic" @ "Mosaic" => RegistrationConfig::mosaic(),
    }
}

preset_enum! {
    CombinePreset => StackConfig,
    display: "CombinePreset",
    variants: {
        SigmaClipped = "sigma_clipped" @ "Sigma Clipped" => StackConfig::sigma_clipped(COMBINE_SIGMA),
        Winsorized = "winsorized" @ "Winsorized" => StackConfig::winsorized(COMBINE_SIGMA),
        Median = "median" @ "Median" => StackConfig::median(),
        Mean = "mean" @ "Mean" => StackConfig::mean(),
    }
}

/// The star-detection knobs the editor offers, drawn from
/// [`StarDetectionConfig`]'s `detection`, `fwhm` and `filter` sub-configs.
#[derive(Debug, Clone, Introspect)]
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
        StarDetectionConfig::default().into()
    }
}

impl From<StarDetectionConfig> for DetectionKnobs {
    fn from(config: StarDetectionConfig) -> Self {
        Self {
            sigma_threshold: config.detection.sigma_threshold,
            expected_fwhm: config.fwhm.expected,
            min_area: config.detection.min_area,
            max_area: config.detection.max_area,
            min_snr: config.filter.min_snr,
            max_eccentricity: config.filter.max_eccentricity,
        }
    }
}

impl From<DetectionKnobs> for StarDetectionConfig {
    fn from(knobs: DetectionKnobs) -> Self {
        let mut config = StarDetectionConfig::default();
        config.detection.sigma_threshold = knobs.sigma_threshold;
        config.fwhm.expected = knobs.expected_fwhm;
        config.detection.min_area = knobs.min_area;
        config.detection.max_area = knobs.max_area;
        config.filter.min_snr = knobs.min_snr;
        config.filter.max_eccentricity = knobs.max_eccentricity;
        config
    }
}

impl NodeConfig for DetectionKnobs {
    const TYPE_ID: &'static str = "4512544e-537c-4c1c-96ad-e596cc88d60d";
    const NAME: &'static str = "DetectionConfig";
}

/// The registration knobs the editor offers. `sip_enabled` stands in for
/// [`RegistrationConfig::sip`]'s whole `Option<SipConfig>`: on means the
/// default SIP fit, off means none.
#[derive(Debug, Clone, Introspect)]
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

impl NodeConfig for RegistrationKnobs {
    const TYPE_ID: &'static str = "63cd4de9-b82f-4829-bea5-391da64e296f";
    const NAME: &'static str = "RegistrationConfig";
}

/// Which combination [`CombineKnobs`] builds. A [`StackConfig`] carries each
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

/// The frame-combination knobs the editor offers. `sigma` is read only by the
/// two rejecting methods.
#[derive(Debug, Clone, Introspect)]
pub(crate) struct CombineKnobs {
    method: CombineMethodChoice,
    sigma: f32,
}

impl Default for CombineKnobs {
    fn default() -> Self {
        Self {
            method: CombineMethodChoice::SigmaClipped,
            sigma: COMBINE_SIGMA,
        }
    }
}

impl From<CombineKnobs> for StackConfig {
    fn from(knobs: CombineKnobs) -> Self {
        match knobs.method {
            CombineMethodChoice::SigmaClipped => StackConfig::sigma_clipped(knobs.sigma),
            CombineMethodChoice::Winsorized => StackConfig::winsorized(knobs.sigma),
            CombineMethodChoice::Median => StackConfig::median(),
            CombineMethodChoice::Mean => StackConfig::mean(),
        }
    }
}

impl NodeConfig for CombineKnobs {
    const TYPE_ID: &'static str = "843bff16-61ec-47db-9a86-64bb53c9c1cc";
    const NAME: &'static str = "CombineConfig";
}

#[cfg(test)]
mod tests {
    use lumos::{RegistrationConfig, SipConfig, StackConfig, StarDetectionConfig};

    use crate::astro::config::stacking::{
        CombineKnobs, CombineMethodChoice, DetectionKnobs, RegistrationKnobs,
    };

    /// A projection is only safe if every knob writes back to the field it read
    /// from. Round-tripping a config whose knobs all hold *distinct* values
    /// proves that pairing for all of them at once: a knob wired to the wrong
    /// field carries the wrong number back and the assertion names it.
    #[test]
    fn detection_knobs_write_back_the_fields_they_read() {
        let mut config = StarDetectionConfig::default();
        config.detection.sigma_threshold = 4.5;
        config.detection.min_area = 7;
        config.detection.max_area = 900;
        config.fwhm.expected = 3.25;
        config.filter.min_snr = 12.5;
        config.filter.max_eccentricity = 0.75;

        let restored: StarDetectionConfig = DetectionKnobs::from(config.clone()).into();
        assert_eq!(restored.detection.sigma_threshold, 4.5);
        assert_eq!(restored.detection.min_area, 7);
        assert_eq!(restored.detection.max_area, 900);
        assert_eq!(restored.fwhm.expected, 3.25);
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

    /// Each choice builds the combination it names, and `sigma` reaches the two
    /// methods that reject on it.
    ///
    /// Compared on `method` alone: the rest of a [`StackConfig`] is cache and
    /// quality settings the projection never touches, and its default cache
    /// directory is unique per instance.
    #[test]
    fn each_combine_choice_builds_its_combine_method() {
        let built = |method| StackConfig::from(CombineKnobs { method, sigma: 2.5 }).method;
        assert_eq!(
            built(CombineMethodChoice::SigmaClipped),
            StackConfig::sigma_clipped(2.5).method
        );
        assert_eq!(
            built(CombineMethodChoice::Winsorized),
            StackConfig::winsorized(2.5).method
        );
        assert_eq!(
            built(CombineMethodChoice::Median),
            StackConfig::median().method
        );
        assert_eq!(built(CombineMethodChoice::Mean), StackConfig::mean().method);
        assert_ne!(
            built(CombineMethodChoice::SigmaClipped),
            StackConfig::sigma_clipped(3.5).method,
            "sigma has to reach the rejecting methods"
        );
    }
}
