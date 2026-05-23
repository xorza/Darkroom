use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};

/// Config file name, resolved relative to the process working
/// directory. Rhai so it's hand-editable and matches the document /
/// theme on-disk format.
const CONFIG_FILE: &str = "darkroom.config.rhai";

/// Persisted session state: which theme is active (by name, resolved
/// to `<cwd>/<name>.toml`) and the document open when the app last
/// closed. Reloaded on startup so darkroom reopens where the user
/// left off. Missing / unreadable config falls back to `default()`.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    /// Stem of the active theme file in the working dir. `None` uses
    /// the built-in default theme.
    pub theme_name: Option<String>,
    /// Document to reopen on launch. `None` starts with an empty doc.
    pub document_path: Option<PathBuf>,
}

impl AppConfig {
    fn path() -> PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(CONFIG_FILE)
    }

    /// Resolve a theme name to its on-disk path in the working dir.
    /// Themes serialize as TOML.
    pub fn theme_path(name: &str) -> PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(format!("{name}.toml"))
    }

    /// Read the config from the working dir. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt config shouldn't brick the app.
    pub fn load() -> Self {
        let Ok(bytes) = std::fs::read(Self::path()) else {
            return Self::default();
        };
        deserialize(&bytes, SerdeFormat::Rhai).unwrap_or_default()
    }

    /// Write the config to the working dir. Errors print to stderr —
    /// a failed persist shouldn't interrupt the user's session.
    pub fn save(&self) {
        let bytes = serialize(self, SerdeFormat::Rhai);
        if let Err(err) = std::fs::write(Self::path(), &bytes) {
            eprintln!("config save failed: {err}");
        }
    }
}
