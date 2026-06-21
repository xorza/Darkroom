//! Lens-side editable mirrors of lumos config types.
//!
//! The generic config-builder ([`crate::config_node`]) reflects fields via
//! `serde` + [`schemars::JsonSchema`]. Rather than make lumos derive those for
//! the editor's sake, each lumos config gets a thin mirror here that derives
//! them and converts to/from the lumos type. `From<lumos::X>` also gives the
//! mirror's `Default` (so the builder's seeded defaults match lumos), and
//! `From<Mirror> for lumos::X` is compile-checked against the lumos struct — add
//! a field there and the conversion stops compiling until the mirror catches up.

use lumos::{BackgroundConfig, BackgroundMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config_node::NodeConfig;

/// Editable mirror of [`lumos::BackgroundMode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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

/// Editable mirror of [`lumos::BackgroundConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
        BackgroundConfig::default().into()
    }
}

impl From<BackgroundConfig> for BackgroundConfigDef {
    fn from(config: BackgroundConfig) -> Self {
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

impl From<BackgroundConfigDef> for BackgroundConfig {
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
    const NAME: &'static str = "BackgroundConfig";
}
