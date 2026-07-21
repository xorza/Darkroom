//! Shared preset input and resolution machinery for astro configuration families.

use scenarium::{DynamicValue, FuncInput, ValueVariant};

use crate::config_node::{ConfigValue, NodeConfig, config_data_type};

pub(crate) trait Preset: Sized {
    type Config;

    fn picker_variants() -> Vec<ValueVariant>;
    fn parse(value: &str) -> Option<Self>;
    fn config(self) -> Self::Config;
}

pub(crate) fn input<T, P>(name: &str) -> FuncInput
where
    T: NodeConfig,
    P: Preset,
{
    let variants = P::picker_variants();
    let default_value = variants.first().map(|variant| variant.value.clone());
    let mut input = FuncInput::required(name, config_data_type::<T>())
        .description("Preset quick-pick; wire a matching build config node to override it.")
        .variants(variants);
    input.default_value = default_value;
    input
}

pub(crate) fn resolve<T, P>(value: &DynamicValue) -> P::Config
where
    T: NodeConfig + Into<P::Config>,
    P: Preset,
{
    value
        .as_custom::<ConfigValue<T>>()
        .map(|config| config.0.clone().into())
        .or_else(|| value.as_enum().and_then(P::parse).map(Preset::config))
        .expect("config input type is validated at the compile boundary")
}

/// Generate a preset enum + its `EnumVariants` (the `value_variants` list) /
/// `FromStr` glue. Each variant carries a stable string `label` (the serialized
/// value), a human `display` label (the dropdown text), and a `config`
/// expression that builds the lumos stage config.
macro_rules! preset_enum {
    (
        $(#[$meta:meta])*
        $enum:ident => $config:ty,
        display: $display:literal,
        variants: { $($variant:ident = $label:literal @ $label_display:literal => $ctor:expr),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
        pub(crate) enum $enum {
            $($variant),+
        }

        impl $enum {
            pub(crate) fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label),+
                }
            }

            /// Human dropdown label for this preset (display-only).
            fn display_label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label_display),+
                }
            }

            /// The picker variants: each stores the raw `label` as its bound
            /// value but shows the friendly `display_label`.
            pub(crate) fn picker_variants() -> Vec<scenarium::ValueVariant> {
                <Self as strum::IntoEnumIterator>::iter()
                    .map(|v| {
                        scenarium::ValueVariant::new(
                            v.label(),
                            scenarium::StaticValue::Enum(v.label().to_string()),
                        )
                        .display(v.display_label())
                    })
                    .collect()
            }

            /// Expand this preset to its lumos stage config.
            pub(crate) fn config(self) -> $config {
                match self {
                    $(Self::$variant => $ctor),+
                }
            }
        }

        impl scenarium::EnumVariants for $enum {
            fn variant_names() -> Vec<String> {
                <Self as strum::IntoEnumIterator>::iter()
                    .map(|v| v.label().to_string())
                    .collect()
            }
        }

        impl std::str::FromStr for $enum {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, String> {
                <Self as strum::IntoEnumIterator>::iter()
                    .find(|v| v.label() == s)
                    .ok_or_else(|| format!("unknown {} preset: {s}", $display))
            }
        }

        impl crate::astro::config::preset::Preset for $enum {
            type Config = $config;

            fn picker_variants() -> Vec<scenarium::ValueVariant> {
                $enum::picker_variants()
            }

            fn parse(value: &str) -> Option<Self> {
                <Self as std::str::FromStr>::from_str(value).ok()
            }

            fn config(self) -> Self::Config {
                $enum::config(self)
            }
        }
    };
}

pub(crate) use preset_enum;

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use lumos::{
        BackgroundMode, ExtractBackground, RegistrationConfig, Scnr, Stretch, StretchMethod,
    };
    use scenarium::{DynamicValue, EnumVariants, StaticValue};
    use strum::IntoEnumIterator;

    use crate::astro::config::preset::resolve;
    use crate::astro::config::processing::{
        BackgroundModeKind, ScnrKind, StretchConfigDef, StretchPreset,
    };
    use crate::astro::config::stacking::{CombinePreset, DetectionPreset, RegistrationPreset};
    use crate::config_node::ConfigValue;

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
        let background: ExtractBackground = BackgroundModeKind::Divide.config();
        assert_eq!(background.mode, BackgroundMode::Divide);
        let _scnr: Scnr = ScnrKind::AdditiveMask.config();
    }

    #[test]
    fn resolver_accepts_presets_and_wired_configs() {
        let preset = resolve::<StretchConfigDef, StretchPreset>(&DynamicValue::from(
            StaticValue::Enum("auto_stf".to_string()),
        ));
        assert!(matches!(preset.method, StretchMethod::AutoStf { .. }));

        let configured = resolve::<StretchConfigDef, StretchPreset>(&DynamicValue::from_custom(
            ConfigValue(StretchConfigDef::default()),
        ));
        assert!(matches!(configured.method, StretchMethod::AutoAsinh { .. }));
    }

    #[test]
    #[should_panic(expected = "config input type is validated")]
    fn resolver_rejects_incompatible_values() {
        resolve::<StretchConfigDef, StretchPreset>(&DynamicValue::from(1.0));
    }
}
