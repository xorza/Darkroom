use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Config {}

impl Config {
    pub fn load_or_default() -> Self {
        Config::load().unwrap_or_default()
    }

    fn load() -> anyhow::Result<Self> {
        let serialized = std::fs::read("config.json")?;
        common::serde::deserialize(&serialized, common::FileFormat::Yaml)
    }
}
