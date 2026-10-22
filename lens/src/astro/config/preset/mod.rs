//! The preset a processing node offers on its config input.
//!
//! The input carries a detailed config's wire type and a picker over a
//! fieldless enum: unwired, it holds a pick; wired, the config. One input, so
//! a run and the editor read it the same way.

use std::fmt;

use common::{Introspect, IntrospectEnum};
use scenarium::ValueVariant;
use scenarium::{ConstValue, DynamicValue, FuncInput};

use crate::config_node::{ConfigValue, config_data_type};

/// A config family offered as presets. A pick expands into [`Self::Config`],
/// what the node runs with; a wired detailed config holds [`Self::Knobs`],
/// which converts into the same.
pub(crate) trait Preset: IntrospectEnum + Copy + 'static {
    /// What a wired detailed config carries.
    type Knobs: Introspect + Clone + fmt::Debug + Send + Sync + 'static + Into<Self::Config>;
    /// What the node runs with.
    type Config;

    /// The config this pick stands for.
    fn config(self) -> Self::Config;

    /// The input: a detailed config's wire type, with the picks as its
    /// constants and the first as its default.
    fn input(name: &str) -> FuncInput {
        let variants = Self::VARIANTS
            .iter()
            .zip(Self::LABELS)
            .map(|(&variant, &label)| {
                ValueVariant::new(variant, ConstValue::Enum(variant.to_owned())).display(label)
            })
            .collect();
        FuncInput::required(name, config_data_type::<Self::Knobs>())
            .description("A preset, or a detailed config wired in.")
            .variants(variants)
            .default(ConstValue::Enum(Self::VARIANTS[0].to_owned()))
    }

    /// The config a node runs with: the wired detailed config, else the pick.
    fn resolve(value: &DynamicValue) -> Self::Config {
        match value.as_custom::<ConfigValue<Self::Knobs>>() {
            Some(config) => config.0.clone().into(),
            None => Self::from_variant(value.required_enum())
                .expect("the load and the edit gate admitted the pick among the picker's variants")
                .config(),
        }
    }
}

#[cfg(test)]
mod tests;
