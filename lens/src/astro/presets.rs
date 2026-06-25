//! Dropdown presets for the astro nodes — small enums that map a
//! human-readable variant onto a lumos config (`StarDetectionConfig` /
//! `RegistrationConfig` / `StackConfig` / `Stretch` / `Scnr`). Every
//! preset-consuming node offers these as a `value_variants` quick-pick on a
//! config-typed input (a `build_*_config` node overrides it); the node reads the
//! chosen variant back with `FromStr` and expands it with `config()`.

use std::str::FromStr;

use lumos::{BackgroundMode, RegistrationConfig, Scnr, StackConfig, StarDetectionConfig, Stretch};
use scenarium::data::EnumVariants;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// Sigma threshold baked into the rejection-based combine presets.
const COMBINE_SIGMA: f32 = 3.0;

/// Generate a preset enum + its `EnumVariants` (the `value_variants` list) /
/// `FromStr` glue. Each variant carries a stable string `label` (the dropdown
/// text + serialized value) and a `config` expression that builds the lumos
/// stage config.
macro_rules! preset_enum {
    (
        $(#[$meta:meta])*
        $enum:ident => $config:ty,
        display: $display:literal,
        variants: { $($variant:ident = $label:literal => $ctor:expr),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
        pub enum $enum {
            $($variant),+
        }

        impl $enum {
            fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label),+
                }
            }

            /// Expand this preset to its lumos stage config.
            pub fn config(self) -> $config {
                match self {
                    $(Self::$variant => $ctor),+
                }
            }
        }

        impl EnumVariants for $enum {
            fn variant_names() -> Vec<String> {
                Self::iter().map(|v| v.label().to_string()).collect()
            }
        }

        impl FromStr for $enum {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, String> {
                Self::iter()
                    .find(|v| v.label() == s)
                    .ok_or_else(|| format!("unknown {} preset: {s}", $display))
            }
        }
    };
}

preset_enum! {
    /// Star-detection tuning preset.
    DetectionPreset => StarDetectionConfig,
    display: "DetectionPreset",
    variants: {
        WideField = "wide_field" => StarDetectionConfig::wide_field(),
        HighResolution = "high_resolution" => StarDetectionConfig::high_resolution(),
        CrowdedField = "crowded_field" => StarDetectionConfig::crowded_field(),
        PreciseGround = "precise_ground" => StarDetectionConfig::precise_ground(),
    }
}

preset_enum! {
    /// Registration (alignment) tuning preset.
    RegistrationPreset => RegistrationConfig,
    display: "RegistrationPreset",
    variants: {
        Default = "default" => RegistrationConfig::default(),
        Fast = "fast" => RegistrationConfig::fast(),
        Precise = "precise" => RegistrationConfig::precise(),
        WideField = "wide_field" => RegistrationConfig::wide_field(),
        Mosaic = "mosaic" => RegistrationConfig::mosaic(),
    }
}

preset_enum! {
    /// Frame-combination preset.
    CombinePreset => StackConfig,
    display: "CombinePreset",
    variants: {
        SigmaClipped = "sigma_clipped" => StackConfig::sigma_clipped(COMBINE_SIGMA),
        Winsorized = "winsorized" => StackConfig::winsorized(COMBINE_SIGMA),
        Median = "median" => StackConfig::median(),
        Mean = "mean" => StackConfig::mean(),
    }
}

preset_enum! {
    /// Auto-stretch method preset (display-domain tone curve).
    StretchPreset => Stretch,
    display: "StretchPreset",
    variants: {
        AutoAsinh = "auto_asinh" => Stretch::auto_asinh(),
        AutoStf = "auto_stf" => Stretch::auto_stf(),
    }
}

preset_enum! {
    /// Background-removal mode for `extract_background`.
    BackgroundModeKind => BackgroundMode,
    display: "BackgroundMode",
    variants: {
        Subtract = "subtract" => BackgroundMode::Subtract,
        Divide = "divide" => BackgroundMode::Divide,
    }
}

/// Blend amount for the SCNR additive-mask method.
const SCNR_ADDITIVE_AMOUNT: f32 = 0.5;

preset_enum! {
    /// SCNR (green-cast removal) method.
    ScnrKind => Scnr,
    display: "Scnr",
    variants: {
        AverageNeutral = "average_neutral" => Scnr::average_neutral(),
        AdditiveMask = "additive_mask" => Scnr::additive_mask(SCNR_ADDITIVE_AMOUNT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_labels_round_trip_through_from_str() {
        assert_eq!(
            DetectionPreset::variant_names(),
            [
                "wide_field",
                "high_resolution",
                "crowded_field",
                "precise_ground"
            ]
        );
        for v in DetectionPreset::iter() {
            assert_eq!(DetectionPreset::from_str(v.label()).unwrap(), v);
        }
        assert!(DetectionPreset::from_str("nope").is_err());
    }

    #[test]
    fn combine_preset_lists_its_variants() {
        assert_eq!(
            CombinePreset::variant_names(),
            ["sigma_clipped", "winsorized", "median", "mean"]
        );
    }

    #[test]
    fn registration_default_label_maps_to_default_config() {
        // The `Default` variant resolves and produces a config (smoke test
        // that the macro wired the trait `default()` ctor correctly).
        assert_eq!(
            RegistrationPreset::from_str("default").unwrap(),
            RegistrationPreset::Default
        );
        let _cfg: RegistrationConfig = RegistrationPreset::Default.config();
    }

    #[test]
    fn stretch_preset_round_trips_and_maps_to_config() {
        assert_eq!(StretchPreset::variant_names(), ["auto_asinh", "auto_stf"]);
        for v in StretchPreset::iter() {
            assert_eq!(StretchPreset::from_str(v.label()).unwrap(), v);
        }
        let _cfg: Stretch = StretchPreset::AutoStf.config();
    }

    #[test]
    fn processing_enums_have_expected_variants() {
        assert_eq!(BackgroundModeKind::variant_names(), ["subtract", "divide"]);
        assert_eq!(
            ScnrKind::variant_names(),
            ["average_neutral", "additive_mask"]
        );
        let _bg: BackgroundMode = BackgroundModeKind::Divide.config();
        let _scnr: Scnr = ScnrKind::AdditiveMask.config();
    }
}
