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
                            scenarium::ConstValue::Enum(v.label().to_string()),
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
mod tests;
