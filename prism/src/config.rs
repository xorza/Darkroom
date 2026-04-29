//! App preferences persisted between editor runs. Today just the
//! last-opened graph path; probably grows to include window
//! geometry, recent files, etc. Session is the only reader/writer.

use anyhow::Result;
use common::SerdeFormat;
use serde::{Deserialize, Serialize};

use crate::script::TcpScriptConfig;

const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Config {
    pub current_path: Option<std::path::PathBuf>,
    /// Reopen `current_path` at launch. CLI `--load-last` ORs into
    /// this for the run without persisting back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub load_last: bool,
    /// Auto-start the TCP script listener at launch. `None` leaves it
    /// off; mirrors the CLI's `--script-tcp` family for GUI runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_tcp: Option<TcpScriptConfig>,
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
