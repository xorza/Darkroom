use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};

use crate::theme::ThemeChoice;

/// Config file name, resolved relative to the process working
/// directory. TOML so it's hand-editable and matches the theme
/// on-disk format.
const CONFIG_FILE: &str = "darkroom.config.toml";

/// Persisted session state: the theme preference to restore and the
/// document open when the app last closed. Reloaded on startup so
/// darkroom reopens where the user left off. Missing / unreadable
/// config falls back to `default()`. `#[serde(default)]` so a partial
/// config (TOML omits absent keys) still deserializes.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Theme preference to restore (`system` / `dark` / `light`).
    /// Written by the Theme menu; the default (`system`) follows the
    /// OS light/dark setting.
    pub theme: ThemeChoice,
    /// Document to reopen on launch. `None` starts with an empty doc.
    pub document_path: Option<PathBuf>,
}

impl AppConfig {
    fn path() -> PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(CONFIG_FILE)
    }

    /// Read the config from the working dir. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt config shouldn't brick the app.
    pub fn load() -> Self {
        let Ok(bytes) = std::fs::read(Self::path()) else {
            return Self::default();
        };
        deserialize(&bytes, SerdeFormat::Toml).unwrap_or_default()
    }

    /// Write the config to the working dir. Errors print to stderr —
    /// a failed persist shouldn't interrupt the user's session.
    pub fn save(&self) {
        let bytes = match serialize(self, SerdeFormat::Toml) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("config save failed: {err}");
                return;
            }
        };
        if let Err(err) = std::fs::write(Self::path(), &bytes) {
            eprintln!("config save failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cfg: &AppConfig) -> AppConfig {
        let bytes = serialize(cfg, SerdeFormat::Toml).expect("config TOML serializes");
        deserialize(&bytes, SerdeFormat::Toml).expect("config TOML round-trips")
    }

    #[test]
    fn populated_config_roundtrips() {
        let cfg = AppConfig {
            theme: ThemeChoice::Light,
            document_path: Some(PathBuf::from("/tmp/graph.rhai")),
        };
        let back = roundtrip(&cfg);
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.document_path, Some(PathBuf::from("/tmp/graph.rhai")));
    }

    #[test]
    fn default_config_roundtrips() {
        // TOML omits the `None` document path, so the default config
        // serializes to a minimal document; `#[serde(default)]` must
        // restore `theme` as `System` and the path as `None` rather than
        // erroring on the missing keys.
        let back = roundtrip(&AppConfig::default());
        assert_eq!(back.theme, ThemeChoice::System);
        assert_eq!(back.document_path, None);
    }
}
