//! App preferences persisted between editor runs. Today just the
//! last-opened graph path; probably grows to include window
//! geometry, recent files, etc. Session is the only reader/writer.

use anyhow::Result;
use common::SerdeFormat;
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Config {
    pub current_path: Option<std::path::PathBuf>,
}

impl Config {
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_default()
    }

    pub fn save(&self) {
        let serialized = common::serde::serialize(self, SerdeFormat::Toml);
        std::fs::write(CONFIG_FILE, serialized).ok();
    }

    fn load() -> Result<Self> {
        let serialized = std::fs::read(CONFIG_FILE)?;
        common::serde::deserialize(&serialized, SerdeFormat::Toml)
    }
}
