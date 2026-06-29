use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};

use crate::core::theme_pref::ThemeChoice;

/// Config file name, resolved relative to the process working
/// directory. TOML so it's hand-editable and matches the theme
/// on-disk format.
const CONFIG_FILE: &str = "darkroom.config.toml";

/// Persisted session state: the theme preference to restore, the
/// document open when the app last closed, and the ML model paths.
/// Reloaded on startup so darkroom reopens where the user left off.
/// Missing / unreadable config falls back to `default()`.
/// `#[serde(default)]` so a partial config (TOML omits absent keys)
/// still deserializes.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Theme preference to restore (`system` / `dark` / `light`).
    /// Written by the Theme menu; the default (`system`) follows the
    /// OS light/dark setting.
    pub theme: ThemeChoice,
    /// Document to reopen on launch. `None` starts with an empty doc.
    pub document_path: Option<PathBuf>,
    /// ONNX model paths for lens's ML nodes (`ml_denoise` / `remove_stars`).
    /// A TOML `[ml_models]` table — must stay the **last** field, as TOML
    /// tables follow all scalar keys at the same level.
    pub ml_models: MlModelConfig,
}

/// ONNX model paths for lens's ML nodes, persisted so the caller-supplied models survive restarts.
/// Mirrors [`lens::MlModelPaths`] (which is not serde) and is pushed into lens's runtime global at
/// startup via [`AppConfig::apply_ml_model_paths`]. The defaults track [`lens::MlModelPaths::default`]
/// so the bare filenames have a single source of truth.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MlModelConfig {
    /// ONNX denoiser model (DeepSNR), used by the `ml_denoise` node.
    pub denoise: PathBuf,
    /// StarNet-style star-removal ONNX model, used by the `remove_stars` node.
    pub star_removal: PathBuf,
}

impl Default for MlModelConfig {
    fn default() -> Self {
        let defaults = lens::MlModelPaths::default();
        Self {
            denoise: defaults.denoise,
            star_removal: defaults.star_removal,
        }
    }
}

impl From<&MlModelConfig> for lens::MlModelPaths {
    fn from(config: &MlModelConfig) -> Self {
        lens::MlModelPaths {
            denoise: config.denoise.clone(),
            star_removal: config.star_removal.clone(),
        }
    }
}

impl AppConfig {
    fn path() -> PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(CONFIG_FILE)
    }

    /// Publish the ML model paths into lens's runtime global so the `ml_denoise` / `remove_stars`
    /// nodes resolve the caller-supplied models. Called from [`load`] at startup and again whenever
    /// the Config tab edits a path, so a change takes effect on the next node run.
    ///
    /// [`load`]: AppConfig::load
    pub(crate) fn apply_ml_model_paths(&self) {
        lens::set_ml_model_paths((&self.ml_models).into());
    }

    /// Read the config from the working dir. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt config shouldn't brick the app.
    /// Also publishes the loaded ML model paths into lens's runtime global.
    pub fn load() -> Self {
        let config = match std::fs::read(Self::path()) {
            Ok(bytes) => deserialize(&bytes, SerdeFormat::Toml).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        config.apply_ml_model_paths();
        config
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
            ml_models: MlModelConfig {
                denoise: PathBuf::from("/models/d.onnx"),
                star_removal: PathBuf::from("/models/s.onnx"),
            },
        };
        let back = roundtrip(&cfg);
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.document_path, Some(PathBuf::from("/tmp/graph.rhai")));
        assert_eq!(back.ml_models.denoise, PathBuf::from("/models/d.onnx"));
        assert_eq!(back.ml_models.star_removal, PathBuf::from("/models/s.onnx"));
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
        // ML model paths default to lens's canonical bare filenames.
        assert_eq!(
            back.ml_models.denoise,
            lens::MlModelPaths::default().denoise
        );
        assert_eq!(
            back.ml_models.star_removal,
            lens::MlModelPaths::default().star_removal
        );
    }

    #[test]
    fn partial_config_fills_ml_models_from_default() {
        // A pre-existing config written before ml_models existed (only `theme`)
        // must still load — `#[serde(default)]` fills the missing table from
        // `AppConfig::default()`, i.e. lens's canonical model filenames.
        let toml = b"theme = \"dark\"\n";
        let cfg: AppConfig =
            deserialize(toml, SerdeFormat::Toml).expect("partial config deserializes");
        assert_eq!(cfg.theme, ThemeChoice::Dark);
        assert_eq!(cfg.document_path, None);
        assert_eq!(cfg.ml_models.denoise, lens::MlModelPaths::default().denoise);
    }
}
