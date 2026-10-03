//! The quick pick a processing node offers beside its detailed `Config` port.
//!
//! The pick is a const-only picker over a fieldless enum. The `Config` port
//! declares that it overrides the picker, so while a config is wired the
//! compiler sets the pick aside and the editor marks it so.

use std::fmt;

use common::{FieldKind, Introspect, IntrospectEnum};
use scenarium::ValueVariant;
use scenarium::{ConstValue, DataType, DynamicValue, FuncInput, Library, TypeId};

use crate::config_node;
use crate::config_node::ConfigValue;

/// A config family offered as a picker. A pick expands into [`Self::Config`],
/// what the node runs with; a wired detailed config holds [`Self::Knobs`],
/// which converts into the same.
pub(crate) trait Preset: IntrospectEnum + Copy + 'static {
    /// What a detailed `Config` port carries.
    type Knobs: Introspect + Clone + fmt::Debug + Send + Sync + 'static + Into<Self::Config>;
    /// What the node runs with.
    type Config;

    /// The config this pick stands for.
    fn config(self) -> Self::Config;

    /// The picker's enum type.
    fn data_type() -> DataType {
        DataType::Enum(TypeId::literal(Self::TYPE_ID))
    }

    /// Register the picker's enum type, as an enum field of the knobs does.
    fn register(library: &mut Library) {
        config_node::register_field_enum(
            library,
            &FieldKind::Enum {
                type_id: Self::TYPE_ID,
                display_name: Self::DISPLAY_NAME,
                variants: Self::VARIANTS,
            },
        );
    }

    /// The picker: const-only, so a wired config is the one way to override
    /// it, and defaulting to the first variant.
    fn picker(name: &str) -> FuncInput {
        let variants = Self::VARIANTS
            .iter()
            .zip(Self::LABELS)
            .map(|(&variant, &label)| {
                ValueVariant::new(variant, ConstValue::Enum(variant.to_owned())).display(label)
            })
            .collect();
        FuncInput::required(name, Self::data_type())
            .const_only()
            .description("Preset; a wired config overrides it.")
            .variants(variants)
            .default(ConstValue::Enum(Self::VARIANTS[0].to_owned()))
    }

    /// The optional detailed config that overrides the picker at input `picker`.
    fn config_input(name: &str, picker: usize) -> FuncInput {
        ConfigValue::<Self::Knobs>::input(name, picker)
    }

    /// The config a node runs with: the wired detailed config, else the pick.
    fn resolve(pick: &DynamicValue, config: &DynamicValue) -> Self::Config {
        match config.as_custom::<ConfigValue<Self::Knobs>>() {
            Some(config) => config.0.clone().into(),
            None => Self::from_variant(pick.required_enum())
                .expect("the compiler checked the pick against the picker's variants")
                .config(),
        }
    }
}

#[cfg(test)]
mod tests;
