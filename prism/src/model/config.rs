use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Config {
    pub current_path: Option<PathBuf>,
}

impl Config {
    pub fn load_or_default() -> Self {
        Config::load().unwrap_or_default()
    }

    pub fn save(&self) {
        let serialized = common::serde::serialize(self, common::SerdeFormat::Toml);
        std::fs::write("config.toml", serialized).ok();
    }

    fn load() -> anyhow::Result<Self> {
        let serialized = std::fs::read("config.toml")?;
        common::serde::deserialize(&serialized, common::SerdeFormat::Toml)
    }
}
