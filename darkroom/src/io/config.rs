use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};

pub use crate::theme::ThemePreset;

/// Config file name, resolved relative to the process working
/// directory. TOML so it's hand-editable and matches the theme
/// on-disk format.
const CONFIG_FILE: &str = "darkroom.config.toml";

/// Persisted session state: the built-in theme preset to restore and
/// the document open when the app last closed. Reloaded on startup
/// so darkroom reopens where the user left off. Missing / unreadable
/// config falls back to `default()`. `#[serde(default)]` so an
/// all-`None` config (TOML omits `None` keys, yielding an empty doc)
/// still deserializes.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Built-in preset to restore (`dark` / `light`). Written by the
    /// Theme → Toggle Light/Dark command; `None` (the default) uses
    /// the built-in default theme.
    pub theme_preset: Option<ThemePreset>,
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
        let bytes = serialize(self, SerdeFormat::Toml);
        if let Err(err) = std::fs::write(Self::path(), &bytes) {
            eprintln!("config save failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cfg: &AppConfig) -> AppConfig {
        let bytes = serialize(cfg, SerdeFormat::Toml);
        deserialize(&bytes, SerdeFormat::Toml).expect("config TOML round-trips")
    }

    #[test]
    fn populated_config_roundtrips() {
        let cfg = AppConfig {
            theme_preset: Some(ThemePreset::Light),
            document_path: Some(PathBuf::from("/tmp/graph.rhai")),
        };
        let back = roundtrip(&cfg);
        assert_eq!(back.theme_preset, Some(ThemePreset::Light));
        assert_eq!(back.document_path, Some(PathBuf::from("/tmp/graph.rhai")));
    }

    #[test]
    fn all_none_config_roundtrips() {
        // TOML omits `None` keys, so the default config serializes to an
        // (effectively) empty document; `#[serde(default)]` must restore
        // both fields as `None` rather than erroring on the missing keys.
        let back = roundtrip(&AppConfig::default());
        assert_eq!(back.theme_preset, None);
        assert_eq!(back.document_path, None);
    }
}
