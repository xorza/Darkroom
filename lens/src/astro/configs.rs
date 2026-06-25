//! Lens-side editable mirrors of lumos config types.
//!
//! The generic config-builder ([`crate::config_node`]) reflects fields via
//! [`common::Introspect`]. Rather than make lumos derive that, each lumos config
//! gets a thin mirror here that derives `Introspect` and converts to/from the
//! lumos type. `From<lumos::X>` also gives the mirror's `Default` (so the
//! builder's seeded defaults match lumos), and `From<Mirror> for lumos::X` is
//! compile-checked against the lumos struct — add a field there and the
//! conversion stops compiling until the mirror catches up.

use common::{Introspect, IntrospectEnum};
use lumos::{
    BackgroundMode, ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, RegistrationConfig,
    Scnr, StackConfig, StarDetectionConfig, Stretch, StretchMethod, Threshold,
};
use strum_macros::{Display, EnumString};

use crate::config_node::NodeConfig;

/// Editable mirror of [`lumos::BackgroundMode`]. `strum` gives the variant
/// string round-trip backing [`IntrospectEnum`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum BackgroundModeDef {
    #[default]
    Subtract,
    Divide,
}

impl From<BackgroundModeDef> for BackgroundMode {
    fn from(mode: BackgroundModeDef) -> Self {
        match mode {
            BackgroundModeDef::Subtract => BackgroundMode::Subtract,
            BackgroundModeDef::Divide => BackgroundMode::Divide,
        }
    }
}

impl From<BackgroundMode> for BackgroundModeDef {
    fn from(mode: BackgroundMode) -> Self {
        match mode {
            BackgroundMode::Subtract => BackgroundModeDef::Subtract,
            BackgroundMode::Divide => BackgroundModeDef::Divide,
        }
    }
}

/// Editable mirror of [`lumos::ExtractBackground`].
#[derive(Debug, Clone, Introspect)]
pub struct BackgroundConfigDef {
    pub tile_size: usize,
    pub degree: usize,
    pub mode: BackgroundModeDef,
    pub rejection_sigma: f32,
    pub iterations: usize,
    pub divide_floor: f32,
}

impl Default for BackgroundConfigDef {
    fn default() -> Self {
        ExtractBackground::default().into()
    }
}

impl From<ExtractBackground> for BackgroundConfigDef {
    fn from(config: ExtractBackground) -> Self {
        Self {
            tile_size: config.tile_size,
            degree: config.degree,
            mode: config.mode.into(),
            rejection_sigma: config.rejection_sigma,
            iterations: config.iterations,
            divide_floor: config.divide_floor,
        }
    }
}

impl From<BackgroundConfigDef> for ExtractBackground {
    fn from(config: BackgroundConfigDef) -> Self {
        Self {
            tile_size: config.tile_size,
            degree: config.degree,
            mode: config.mode.into(),
            rejection_sigma: config.rejection_sigma,
            iterations: config.iterations,
            divide_floor: config.divide_floor,
        }
    }
}

impl NodeConfig for BackgroundConfigDef {
    const TYPE_ID: &'static str = "47a71876-5db9-45f9-a21d-cc2ce40a80f2";
    const NAME: &'static str = "ExtractBackground";
}

/// The most-tuned [`lumos::StarDetectionConfig`] knobs. Other fields take the
/// lumos default; `From<StarDetectionConfig>` exposes that as the mirror default.
#[derive(Debug, Clone, Introspect)]
pub struct DetectionConfigDef {
    pub sigma_threshold: f32,
    pub expected_fwhm: f32,
    pub min_area: usize,
    pub max_area: usize,
    pub min_snr: f32,
    pub max_eccentricity: f32,
}

impl Default for DetectionConfigDef {
    fn default() -> Self {
        StarDetectionConfig::default().into()
    }
}

impl From<StarDetectionConfig> for DetectionConfigDef {
    fn from(config: StarDetectionConfig) -> Self {
        Self {
            sigma_threshold: config.sigma_threshold,
            expected_fwhm: config.expected_fwhm,
            min_area: config.min_area,
            max_area: config.max_area,
            min_snr: config.min_snr,
            max_eccentricity: config.max_eccentricity,
        }
    }
}

impl From<DetectionConfigDef> for StarDetectionConfig {
    fn from(mirror: DetectionConfigDef) -> Self {
        StarDetectionConfig {
            sigma_threshold: mirror.sigma_threshold,
            expected_fwhm: mirror.expected_fwhm,
            min_area: mirror.min_area,
            max_area: mirror.max_area,
            min_snr: mirror.min_snr,
            max_eccentricity: mirror.max_eccentricity,
            ..Default::default()
        }
    }
}

impl NodeConfig for DetectionConfigDef {
    const TYPE_ID: &'static str = "4512544e-537c-4c1c-96ad-e596cc88d60d";
    const NAME: &'static str = "DetectionConfig";
}

/// The most-tuned [`lumos::RegistrationConfig`] knobs.
#[derive(Debug, Clone, Introspect)]
pub struct RegistrationConfigDef {
    pub max_stars: usize,
    pub min_matches: usize,
    pub ratio_tolerance: f64,
    pub ransac_iterations: usize,
    pub max_rms_error: f64,
    pub sip_enabled: bool,
}

impl Default for RegistrationConfigDef {
    fn default() -> Self {
        RegistrationConfig::default().into()
    }
}

impl From<RegistrationConfig> for RegistrationConfigDef {
    fn from(config: RegistrationConfig) -> Self {
        Self {
            max_stars: config.max_stars,
            min_matches: config.min_matches,
            ratio_tolerance: config.ratio_tolerance,
            ransac_iterations: config.ransac_iterations,
            max_rms_error: config.max_rms_error,
            sip_enabled: config.sip_enabled,
        }
    }
}

impl From<RegistrationConfigDef> for RegistrationConfig {
    fn from(mirror: RegistrationConfigDef) -> Self {
        RegistrationConfig {
            max_stars: mirror.max_stars,
            min_matches: mirror.min_matches,
            ratio_tolerance: mirror.ratio_tolerance,
            ransac_iterations: mirror.ransac_iterations,
            max_rms_error: mirror.max_rms_error,
            sip_enabled: mirror.sip_enabled,
            ..Default::default()
        }
    }
}

impl NodeConfig for RegistrationConfigDef {
    const TYPE_ID: &'static str = "63cd4de9-b82f-4829-bea5-391da64e296f";
    const NAME: &'static str = "RegistrationConfig";
}

/// Frame-combination method (mirrors the rejection/median/mean choice of
/// [`lumos::StackConfig`]; `sigma` applies to the rejection methods).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum CombineMethodDef {
    #[default]
    SigmaClipped,
    Winsorized,
    Median,
    Mean,
}

/// A combine config: a [`CombineMethodDef`] + the rejection `sigma`. Builds the
/// matching [`lumos::StackConfig`] preset (sigma ignored by median/mean).
#[derive(Debug, Clone, Introspect)]
pub struct CombineConfigDef {
    pub method: CombineMethodDef,
    pub sigma: f32,
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

/// Editable mirror of [`lumos::Threshold`] — wavelet coefficient thresholding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum ThresholdDef {
    Hard,
    #[default]
    Soft,
}

impl From<ThresholdDef> for Threshold {
    fn from(threshold: ThresholdDef) -> Self {
        match threshold {
            ThresholdDef::Hard => Threshold::Hard,
            ThresholdDef::Soft => Threshold::Soft,
        }
    }
}

impl From<Threshold> for ThresholdDef {
    fn from(threshold: Threshold) -> Self {
        match threshold {
            Threshold::Hard => ThresholdDef::Hard,
            Threshold::Soft => ThresholdDef::Soft,
        }
    }
}

/// Editable mirror of [`lumos::Denoise`] (full wavelet-denoise knobs;
/// `denoise`'s inline `strength` covers the common case).
#[derive(Debug, Clone, Introspect)]
pub struct DenoiseConfigDef {
    pub scales: usize,
    pub k: f32,
    pub threshold: ThresholdDef,
    pub strength: f32,
}

impl Default for DenoiseConfigDef {
    fn default() -> Self {
        Denoise::default().into()
    }
}

impl From<Denoise> for DenoiseConfigDef {
    fn from(config: Denoise) -> Self {
        Self {
            scales: config.scales,
            k: config.k,
            threshold: config.threshold.into(),
            strength: config.strength,
        }
    }
}

impl From<DenoiseConfigDef> for Denoise {
    fn from(mirror: DenoiseConfigDef) -> Self {
        Self {
            scales: mirror.scales,
            k: mirror.k,
            threshold: mirror.threshold.into(),
            strength: mirror.strength,
        }
    }
}

impl NodeConfig for DenoiseConfigDef {
    const TYPE_ID: &'static str = "ab942729-dc49-4518-aae4-9008bd33cea1";
    const NAME: &'static str = "Denoise";
}

/// Editable mirror of [`lumos::Hdr`] (`hdr_compress`'s inline `amount`
/// covers the common case).
#[derive(Debug, Clone, Introspect)]
pub struct HdrConfigDef {
    pub scales: usize,
    pub amount: f32,
}

impl Default for HdrConfigDef {
    fn default() -> Self {
        Hdr::default().into()
    }
}

impl From<Hdr> for HdrConfigDef {
    fn from(config: Hdr) -> Self {
        Self {
            scales: config.scales,
            amount: config.amount,
        }
    }
}

impl From<HdrConfigDef> for Hdr {
    fn from(mirror: HdrConfigDef) -> Self {
        Self {
            scales: mirror.scales,
            amount: mirror.amount,
        }
    }
}

impl NodeConfig for HdrConfigDef {
    const TYPE_ID: &'static str = "36babf1d-0fda-4d5d-b4c6-ed4c13ebff6b";
    const NAME: &'static str = "Hdr";
}

/// Editable mirror of [`lumos::LocalContrast`] (`local_contrast`'s inline
/// `strength` covers the common case).
#[derive(Debug, Clone, Introspect)]
pub struct LocalContrastConfigDef {
    pub tiles: usize,
    pub clip_limit: f32,
    pub strength: f32,
}

impl Default for LocalContrastConfigDef {
    fn default() -> Self {
        LocalContrast::default().into()
    }
}

impl From<LocalContrast> for LocalContrastConfigDef {
    fn from(config: LocalContrast) -> Self {
        Self {
            tiles: config.tiles,
            clip_limit: config.clip_limit,
            strength: config.strength,
        }
    }
}

impl From<LocalContrastConfigDef> for LocalContrast {
    fn from(mirror: LocalContrastConfigDef) -> Self {
        Self {
            tiles: mirror.tiles,
            clip_limit: mirror.clip_limit,
            strength: mirror.strength,
        }
    }
}

impl NodeConfig for LocalContrastConfigDef {
    const TYPE_ID: &'static str = "eb0062ca-cef9-4fef-a52b-cf3e8e0fce3c";
    const NAME: &'static str = "LocalContrast";
}

/// SCNR method (mirrors [`lumos::Scnr`]'s variant choice; `amount` applies
/// to `additive_mask`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum ScnrMethodDef {
    #[default]
    AverageNeutral,
    AdditiveMask,
}

/// A SCNR config: method + the `additive_mask` blend `amount` (ignored by
/// average-neutral). Builds the matching [`lumos::Scnr`].
#[derive(Debug, Clone, Introspect)]
pub struct ScnrConfigDef {
    pub method: ScnrMethodDef,
    pub amount: f32,
}

impl Default for ScnrConfigDef {
    fn default() -> Self {
        Self {
            method: ScnrMethodDef::AverageNeutral,
            amount: 0.5,
        }
    }
}

impl From<ScnrConfigDef> for Scnr {
    fn from(mirror: ScnrConfigDef) -> Self {
        match mirror.method {
            ScnrMethodDef::AverageNeutral => Scnr::average_neutral(),
            ScnrMethodDef::AdditiveMask => Scnr::additive_mask(mirror.amount),
        }
    }
}

impl NodeConfig for ScnrConfigDef {
    const TYPE_ID: &'static str = "cb80e688-a5ed-42fd-9087-6a9639a8b056";
    const NAME: &'static str = "ScnrConfig";
}

/// Mirror of [`lumos::ColorMode`] (how a stretch curve maps across channels).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum ColorModeDef {
    #[default]
    ColorPreserving,
    PerChannel,
}

impl From<ColorModeDef> for ColorMode {
    fn from(mode: ColorModeDef) -> Self {
        match mode {
            ColorModeDef::ColorPreserving => ColorMode::ColorPreserving,
            ColorModeDef::PerChannel => ColorMode::PerChannel,
        }
    }
}

impl From<ColorMode> for ColorModeDef {
    fn from(mode: ColorMode) -> Self {
        match mode {
            ColorMode::ColorPreserving => ColorModeDef::ColorPreserving,
            ColorMode::PerChannel => ColorModeDef::PerChannel,
        }
    }
}

/// The auto-stretch method choice (mirrors the two automatic
/// [`lumos::StretchMethod`] variants the presets use).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[strum(serialize_all = "snake_case")]
pub enum StretchMethodKindDef {
    #[default]
    AutoAsinh,
    AutoStf,
}

/// Curated mirror of [`lumos::Stretch`] for the two auto methods (what the
/// presets expose): `target_background` applies to both, `shadow_sigmas` only to
/// `auto_stf`. Explicit asinh / GHS curves aren't surfaced as nodes.
#[derive(Debug, Clone, Introspect)]
pub struct StretchConfigDef {
    pub method: StretchMethodKindDef,
    pub target_background: f32,
    pub shadow_sigmas: f32,
    pub color: ColorModeDef,
}

impl Default for StretchConfigDef {
    fn default() -> Self {
        Stretch::default().into()
    }
}

impl From<Stretch> for StretchConfigDef {
    fn from(config: Stretch) -> Self {
        let (method, target_background, shadow_sigmas) = match config.method {
            StretchMethod::AutoAsinh { target_background } => {
                (StretchMethodKindDef::AutoAsinh, target_background, 1.5)
            }
            StretchMethod::AutoStf {
                shadow_sigmas,
                target_background,
            } => (
                StretchMethodKindDef::AutoStf,
                target_background,
                shadow_sigmas,
            ),
            // Explicit asinh / GHS aren't mirrored — fall back to auto-asinh defaults.
            StretchMethod::Asinh { .. } | StretchMethod::Ghs { .. } => {
                (StretchMethodKindDef::AutoAsinh, 0.2, 1.5)
            }
        };
        Self {
            method,
            target_background,
            shadow_sigmas,
            color: config.color.into(),
        }
    }
}

impl From<StretchConfigDef> for Stretch {
    fn from(mirror: StretchConfigDef) -> Self {
        let method = match mirror.method {
            StretchMethodKindDef::AutoAsinh => StretchMethod::AutoAsinh {
                target_background: mirror.target_background,
            },
            StretchMethodKindDef::AutoStf => StretchMethod::AutoStf {
                shadow_sigmas: mirror.shadow_sigmas,
                target_background: mirror.target_background,
            },
        };
        Stretch {
            method,
            color: mirror.color.into(),
        }
    }
}

impl NodeConfig for StretchConfigDef {
    const TYPE_ID: &'static str = "b08bb9a1-db12-43d4-aa57-fe3e3732e917";
    const NAME: &'static str = "Stretch";
}
