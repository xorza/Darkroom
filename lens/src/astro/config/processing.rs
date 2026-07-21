//! Editable configuration mirrors for per-frame astronomical processing.

use common::{Introspect, IntrospectEnum};
use lumos::{
    BackgroundMode, ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, Scnr, Stretch,
    StretchMethod, Threshold,
};
use strum_macros::{Display, EnumString};

use crate::astro::config::preset::preset_enum;
use crate::config_node::NodeConfig;

const SCNR_ADDITIVE_AMOUNT: f32 = 0.5;

preset_enum! {
    StretchPreset => Stretch,
    display: "StretchPreset",
    variants: {
        AutoAsinh = "auto_asinh" @ "Auto Asinh" => Stretch::auto_asinh(),
        AutoStf = "auto_stf" @ "Auto STF" => Stretch::auto_stf(),
    }
}

preset_enum! {
    BackgroundModeKind => ExtractBackground,
    display: "BackgroundMode",
    variants: {
        Subtract = "subtract" @ "Subtract" => ExtractBackground {
            mode: BackgroundMode::Subtract,
            ..Default::default()
        },
        Divide = "divide" @ "Divide" => ExtractBackground {
            mode: BackgroundMode::Divide,
            ..Default::default()
        },
    }
}

preset_enum! {
    ScnrKind => Scnr,
    display: "Scnr",
    variants: {
        AverageNeutral = "average_neutral" @ "Average Neutral" => Scnr::average_neutral(),
        AdditiveMask = "additive_mask" @ "Additive Mask" => Scnr::additive_mask(SCNR_ADDITIVE_AMOUNT),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "ed416b2d-378b-4eb1-9029-bc7a80a509aa")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum BackgroundModeDef {
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

#[derive(Debug, Clone, Introspect)]
pub(crate) struct BackgroundConfigDef {
    tile_size: usize,
    degree: usize,
    mode: BackgroundModeDef,
    rejection_sigma: f32,
    iterations: usize,
    divide_floor: f32,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "542a0fa0-25ff-4839-b309-acbe65d93a84")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ThresholdDef {
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

#[derive(Debug, Clone, Introspect)]
pub(crate) struct DenoiseConfigDef {
    scales: usize,
    k: f32,
    threshold: ThresholdDef,
    strength: f32,
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

#[derive(Debug, Clone, Introspect)]
pub(crate) struct HdrConfigDef {
    scales: usize,
    amount: f32,
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

#[derive(Debug, Clone, Introspect)]
pub(crate) struct LocalContrastConfigDef {
    tiles: usize,
    clip_limit: f32,
    strength: f32,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "662e2432-b685-4b5b-bf05-0041814dc908")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ScnrMethodDef {
    #[default]
    AverageNeutral,
    AdditiveMask,
}

#[derive(Debug, Clone, Introspect)]
pub(crate) struct ScnrConfigDef {
    method: ScnrMethodDef,
    amount: f32,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "ca1be21f-2096-410c-8bc2-33e96d9b12be")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ColorModeDef {
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumString, Display, IntrospectEnum)]
#[config(type_id = "722f7047-a6fc-4538-abd7-8af5fd1ee0ff")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum StretchMethodKindDef {
    #[default]
    AutoAsinh,
    AutoStf,
}

#[derive(Debug, Clone, Introspect)]
pub(crate) struct StretchConfigDef {
    method: StretchMethodKindDef,
    target_background: f32,
    shadow_sigmas: f32,
    color: ColorModeDef,
}

impl Default for StretchConfigDef {
    fn default() -> Self {
        let config = Stretch::default();
        let StretchMethod::AutoAsinh { target_background } = config.method else {
            panic!("lumos Stretch::default() must remain auto-asinh");
        };
        Self {
            method: StretchMethodKindDef::AutoAsinh,
            target_background,
            shadow_sigmas: 1.5,
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

#[cfg(test)]
mod tests {
    use lumos::{ColorMode, Stretch, StretchMethod};

    use crate::astro::config::processing::{ColorModeDef, StretchConfigDef, StretchMethodKindDef};

    #[test]
    fn stretch_default_and_supported_methods_convert_exactly() {
        let default = StretchConfigDef::default();
        assert_eq!(default.method, StretchMethodKindDef::AutoAsinh);
        assert_eq!(default.color, ColorModeDef::ColorPreserving);

        let stretch: Stretch = StretchConfigDef {
            method: StretchMethodKindDef::AutoStf,
            target_background: 0.25,
            shadow_sigmas: 2.0,
            color: ColorModeDef::PerChannel,
        }
        .into();
        let StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } = stretch.method
        else {
            panic!("expected auto-STF");
        };
        assert_eq!(shadow_sigmas, 2.0);
        assert_eq!(target_background, 0.25);
        assert_eq!(stretch.color, ColorMode::PerChannel);
    }
}
