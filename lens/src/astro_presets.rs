//! Dropdown presets for the stacking node — small enums that map a
//! human-readable variant onto a lumos stage config (`StarDetectionConfig`
//! / `RegistrationConfig` / `StackConfig`). Each renders in the editor as a
//! `DataType::Enum` dropdown (via the Phase 0 enum editor); the node reads
//! the chosen variant back with `FromStr` and expands it with `config()`.

use std::str::FromStr;
use std::sync::LazyLock;

use lumos::{RegistrationConfig, StackConfig, StarDetectionConfig};
use scenarium::data::{DataType, EnumVariants};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// Sigma threshold baked into the rejection-based combine presets.
const COMBINE_SIGMA: f32 = 3.0;

/// Generate a preset enum plus its `EnumVariants` / `FromStr` glue and a
/// `DataType::Enum` handle. Each variant carries a stable string `label`
/// (the dropdown text + serialized value) and a `config` expression that
/// builds the lumos stage config.
macro_rules! preset_enum {
    (
        $(#[$meta:meta])*
        $enum:ident => $config:ty,
        datatype: $datatype:ident,
        type_id: $type_id:literal,
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

        pub static $datatype: LazyLock<DataType> =
            LazyLock::new(|| DataType::from_enum::<$enum>($type_id, $display));
    };
}

preset_enum! {
    /// Star-detection tuning preset.
    DetectionPreset => StarDetectionConfig,
    datatype: DETECTION_PRESET_DATATYPE,
    type_id: "70a45f76-9ff8-4cde-93ed-a23bdebb744f",
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
    datatype: REGISTRATION_PRESET_DATATYPE,
    type_id: "2724355e-82f6-4b20-b4dc-8df4ee3441b7",
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
    datatype: COMBINE_PRESET_DATATYPE,
    type_id: "a5ced174-4463-4395-99f2-59d66725574b",
    display: "CombinePreset",
    variants: {
        SigmaClipped = "sigma_clipped" => StackConfig::sigma_clipped(COMBINE_SIGMA),
        Winsorized = "winsorized" => StackConfig::winsorized(COMBINE_SIGMA),
        Median = "median" => StackConfig::median(),
        Mean = "mean" => StackConfig::mean(),
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
    fn combine_datatype_is_an_enum_with_the_listed_variants() {
        let DataType::Enum(def) = &*COMBINE_PRESET_DATATYPE else {
            panic!("expected an Enum data type");
        };
        assert_eq!(def.display_name, "CombinePreset");
        assert_eq!(
            def.variants,
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
}
