//! Editable configuration mirrors for star detection, registration, and stacking.

use common::Introspect;
use common::IntrospectEnum;
use lumos::{RegistrationConfig, SipConfig, StackConfig, StarDetectionConfig};
use strum_macros::{Display, EnumString};

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

#[derive(Debug, Clone, Introspect)]
pub(crate) struct DetectionConfigDef {
    sigma_threshold: f32,
    expected_fwhm: f32,
    min_area: usize,
    max_area: usize,
    min_snr: f32,
    max_eccentricity: f32,
}

impl Default for DetectionConfigDef {
    fn default() -> Self {
        StarDetectionConfig::default().into()
    }
}

impl From<StarDetectionConfig> for DetectionConfigDef {
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

impl From<DetectionConfigDef> for StarDetectionConfig {
    fn from(mirror: DetectionConfigDef) -> Self {
        let mut config = StarDetectionConfig::default();
        config.detection.sigma_threshold = mirror.sigma_threshold;
        config.fwhm.expected = mirror.expected_fwhm;
        config.detection.min_area = mirror.min_area;
        config.detection.max_area = mirror.max_area;
        config.filter.min_snr = mirror.min_snr;
        config.filter.max_eccentricity = mirror.max_eccentricity;
        config
    }
}

impl NodeConfig for DetectionConfigDef {
    const TYPE_ID: &'static str = "4512544e-537c-4c1c-96ad-e596cc88d60d";
    const NAME: &'static str = "DetectionConfig";
}

#[derive(Debug, Clone, Introspect)]
pub(crate) struct RegistrationConfigDef {
    max_stars: usize,
    min_matches: usize,
    ratio_tolerance: f64,
    ransac_iterations: usize,
    max_rms_error: f64,
    sip_enabled: bool,
}

impl Default for RegistrationConfigDef {
    fn default() -> Self {
        RegistrationConfig::default().into()
    }
}

impl From<RegistrationConfig> for RegistrationConfigDef {
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

impl From<RegistrationConfigDef> for RegistrationConfig {
    fn from(mirror: RegistrationConfigDef) -> Self {
        let mut config = RegistrationConfig::default();
        config.matching.max_stars = mirror.max_stars;
        config.matching.min_matches = mirror.min_matches;
        config.matching.triangle.ratio_tolerance = mirror.ratio_tolerance;
        config.ransac.max_iterations = mirror.ransac_iterations;
        config.max_rms_error = mirror.max_rms_error;
        config.sip = mirror.sip_enabled.then(SipConfig::default);
        config
    }
}

impl NodeConfig for RegistrationConfigDef {
    const TYPE_ID: &'static str = "63cd4de9-b82f-4829-bea5-391da64e296f";
    const NAME: &'static str = "RegistrationConfig";
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "0ac16ec1-4a1e-48e9-aff5-17df1ff645bc")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum CombineMethodDef {
    #[default]
    SigmaClipped,
    Winsorized,
    Median,
    Mean,
}

#[derive(Debug, Clone, Introspect)]
pub(crate) struct CombineConfigDef {
    method: CombineMethodDef,
    sigma: f32,
}

impl Default for CombineConfigDef {
    fn default() -> Self {
        Self {
            method: CombineMethodDef::SigmaClipped,
            sigma: 3.0,
        }
    }
}

impl From<CombineConfigDef> for StackConfig {
    fn from(mirror: CombineConfigDef) -> Self {
        match mirror.method {
            CombineMethodDef::SigmaClipped => StackConfig::sigma_clipped(mirror.sigma),
            CombineMethodDef::Winsorized => StackConfig::winsorized(mirror.sigma),
            CombineMethodDef::Median => StackConfig::median(),
            CombineMethodDef::Mean => StackConfig::mean(),
        }
    }
}

impl NodeConfig for CombineConfigDef {
    const TYPE_ID: &'static str = "843bff16-61ec-47db-9a86-64bb53c9c1cc";
    const NAME: &'static str = "CombineConfig";
}
