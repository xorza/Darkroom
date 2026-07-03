use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};

use crate::core::theme_pref::ThemeChoice;

/// Preferences file name, resolved relative to the process working
/// directory. TOML so it's hand-editable and matches the theme
/// on-disk format.
const PREFERENCES_FILE: &str = "darkroom.preferences.toml";

/// Persisted session state: the theme preference to restore, the
/// document open when the app last closed, and the ML model paths.
/// Reloaded on startup so darkroom reopens where the user left off.
/// Missing / unreadable preferences fall back to `default()`.
/// `#[serde(default)]` so a partial preferences file (TOML omits absent keys)
/// still deserializes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Theme preference to restore (`system` / `dark` / `light`).
    /// Written by the Theme menu; the default (`system`) follows the
    /// OS light/dark setting.
    pub theme: ThemeChoice,
    /// Document to reopen on launch. `None` starts with an empty doc.
    pub document_path: Option<PathBuf>,
    /// Reopen `document_path` on launch. When `false`, launch starts with
    /// an empty document (the path is still remembered, just not opened).
    /// Defaults to `true` — the historical reopen-where-you-left-off behavior.
    pub load_last_document: bool,
    /// Prompt to save unsaved changes before quitting (window close, ⌘Q,
    /// File ▸ Quit). When `false`, quitting discards unsaved changes without
    /// asking. The exit dialog's "Don't ask again" checkbox clears it; the
    /// Preferences tab can restore it. Defaults to `true`.
    pub confirm_unsaved_on_exit: bool,
    /// ONNX model paths for lens's ML nodes (`ml_denoise` / `remove_stars`).
    /// A TOML `[ml_models]` table — must stay the **last** field, as TOML
    /// tables follow all scalar keys at the same level.
    pub ml_models: MlModelPreferences,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            document_path: None,
            load_last_document: true,
            confirm_unsaved_on_exit: true,
            ml_models: MlModelPreferences::default(),
        }
    }
}

/// ONNX model paths for lens's ML nodes, persisted so the caller-supplied models survive restarts.
/// Mirrors [`lens::MlModelPaths`] (which is not serde) and is pushed into lens's runtime global at
/// startup via [`Preferences::apply_ml_model_paths`]. The defaults track [`lens::MlModelPaths::default`]
/// so the bare filenames have a single source of truth.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MlModelPreferences {
    /// ONNX denoiser model (DeepSNR), used by the `ml_denoise` node.
    pub denoise: PathBuf,
    /// StarNet-style star-removal ONNX model, used by the `remove_stars` node.
    pub star_removal: PathBuf,
}

impl Default for MlModelPreferences {
    fn default() -> Self {
        let defaults = lens::MlModelPaths::default();
        Self {
            denoise: defaults.denoise,
            star_removal: defaults.star_removal,
        }
    }
}

impl From<&MlModelPreferences> for lens::MlModelPaths {
    fn from(preferences: &MlModelPreferences) -> Self {
        lens::MlModelPaths {
            denoise: preferences.denoise.clone(),
            star_removal: preferences.star_removal.clone(),
        }
    }
}

impl Preferences {
    fn path() -> PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(PREFERENCES_FILE)
    }

    /// Publish the ML model paths into lens's runtime global so the `ml_denoise` / `remove_stars`
    /// nodes resolve the caller-supplied models. Called from [`load`] at startup and again whenever
    /// the Preferences tab edits a path, so a change takes effect on the next node run.
    ///
    /// [`load`]: Preferences::load
    pub(crate) fn apply_ml_model_paths(&self) {
        lens::set_ml_model_paths((&self.ml_models).into());
    }

    /// Read the preferences from the working dir. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt preferences file shouldn't brick the app.
    /// Also publishes the loaded ML model paths into lens's runtime global.
    pub fn load() -> Self {
        let preferences = match std::fs::read(Self::path()) {
            Ok(bytes) => deserialize(&bytes, SerdeFormat::Toml).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        preferences.apply_ml_model_paths();
        preferences
    }

    /// Write the preferences to the working dir. Errors print to stderr —
    /// a failed persist shouldn't interrupt the user's session.
    pub fn save(&self) {
        let bytes = match serialize(self, SerdeFormat::Toml) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("preferences save failed: {err}");
                return;
            }
        };
        if let Err(err) = std::fs::write(Self::path(), &bytes) {
            eprintln!("preferences save failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cfg: &Preferences) -> Preferences {
        let bytes = serialize(cfg, SerdeFormat::Toml).expect("preferences TOML serializes");
        deserialize(&bytes, SerdeFormat::Toml).expect("preferences TOML round-trips")
    }

    #[test]
    fn populated_preferences_roundtrips() {
        let cfg = Preferences {
            theme: ThemeChoice::Light,
            document_path: Some(PathBuf::from("/tmp/graph.rhai")),
            // Non-defaults (defaults are `true`) so the round-trip is meaningful.
            load_last_document: false,
            confirm_unsaved_on_exit: false,
            ml_models: MlModelPreferences {
                denoise: PathBuf::from("/models/d.onnx"),
                star_removal: PathBuf::from("/models/s.onnx"),
            },
        };
        let back = roundtrip(&cfg);
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.document_path, Some(PathBuf::from("/tmp/graph.rhai")));
        assert!(!back.load_last_document);
        assert!(!back.confirm_unsaved_on_exit);
        assert_eq!(back.ml_models.denoise, PathBuf::from("/models/d.onnx"));
        assert_eq!(back.ml_models.star_removal, PathBuf::from("/models/s.onnx"));
    }

    #[test]
    fn default_preferences_roundtrips() {
        // TOML omits the `None` document path, so the default preferences
        // serializes to a minimal document; `#[serde(default)]` must
        // restore `theme` as `System` and the path as `None` rather than
        // erroring on the missing keys.
        let back = roundtrip(&Preferences::default());
        assert_eq!(back.theme, ThemeChoice::System);
        assert_eq!(back.document_path, None);
        // Defaults to reopening the last document (historical behavior).
        assert!(back.load_last_document);
        // Defaults to prompting before quitting with unsaved changes.
        assert!(back.confirm_unsaved_on_exit);
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
    fn partial_preferences_fills_ml_models_from_default() {
        // A pre-existing preferences file written before ml_models existed (only `theme`)
        // must still load — `#[serde(default)]` fills the missing table from
        // `Preferences::default()`, i.e. lens's canonical model filenames.
        let toml = b"theme = \"dark\"\n";
        let cfg: Preferences =
            deserialize(toml, SerdeFormat::Toml).expect("partial preferences deserializes");
        assert_eq!(cfg.theme, ThemeChoice::Dark);
        assert_eq!(cfg.document_path, None);
        // A preferences file predating this key still defaults to reopening the document.
        assert!(cfg.load_last_document);
        assert_eq!(cfg.ml_models.denoise, lens::MlModelPaths::default().denoise);
    }
}
