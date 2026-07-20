use std::path::PathBuf;

use aperture::ImageFilter;
use common::{SerdeFormat, deserialize, file_utils, serialize};
use glam::{IVec2, UVec2};

use crate::core::io::cwd_file;
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
pub(crate) struct Preferences {
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
    /// Main window geometry from the last session, restored at launch so
    /// the editor reopens at the same size / position. `None` on first run
    /// (platform picks). A TOML `[window]` table — a table field, so it
    /// sits with the other tables after every scalar key.
    pub window: Option<WindowState>,
    /// Image-viewer toolbar choices (backdrop + magnification sampling),
    /// shared by all viewer tabs: a toolbar click in any viewer edits this
    /// in place and persists. A TOML `[viewer]` table.
    pub viewer: ViewerPreferences,
    /// ONNX model paths for lens's ML nodes (`ml_denoise` / `remove_stars`).
    /// A TOML `[ml_models]` table — must stay the **last** field, as TOML
    /// tables follow all scalar keys at the same level.
    pub ml_models: MlModelPreferences,
}

/// Backdrop behind (and around) a viewer's image, as offered by the
/// viewer toolbar's swatch row. Frontend-agnostic persisted choice,
/// like [`ThemeChoice`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ViewerBackground {
    /// The editor's canvas fill — the resting default.
    #[default]
    Theme,
    Black,
    White,
    /// Neutral gray checkerboard — the transparency reference.
    Checker,
}

/// Persisted image-viewer toolbar state. One global setting (not
/// per-tab): every viewer pane reads and edits the same choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ViewerPreferences {
    pub background: ViewerBackground,
    /// Magnification sampling for the shown image. Defaults to `Nearest`
    /// for pixel peeping; zoomed-out minification always stays linear.
    pub mag_filter: ImageFilter,
}

impl Default for ViewerPreferences {
    fn default() -> Self {
        Self {
            background: ViewerBackground::default(),
            mag_filter: ImageFilter::Nearest,
        }
    }
}

/// Persisted main-window geometry. `size` is logical pixels (DPI-independent,
/// so it restores to the same apparent size on a differently-scaled
/// monitor); `position` is physical pixels, absent when the platform
/// doesn't report it (Wayland). `maximized` restores the maximized state
/// while `size` remains what to return to when un-maximized. glam vecs
/// serialize as `[x, y]` arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct WindowState {
    /// Logical inner size `[w, h]`.
    pub size: UVec2,
    pub maximized: bool,
    /// Physical outer position `[x, y]`; `None` on Wayland.
    pub position: Option<IVec2>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            document_path: None,
            load_last_document: true,
            confirm_unsaved_on_exit: true,
            window: None,
            viewer: ViewerPreferences::default(),
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
        cwd_file(PREFERENCES_FILE)
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
    pub(crate) fn load() -> Self {
        let preferences = match std::fs::read(Self::path()) {
            Ok(bytes) => deserialize(&bytes, SerdeFormat::Toml).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        preferences.apply_ml_model_paths();
        preferences
    }

    /// Write the preferences to the working dir. `Err` carries the
    /// display-ready reason — the caller surfaces it (status bar); a
    /// failed persist shouldn't interrupt the user's session.
    pub(crate) fn save(&self) -> Result<(), String> {
        let bytes = serialize(self, SerdeFormat::Toml)
            .map_err(|err| format!("preferences save failed: {err}"))?;
        file_utils::publish_bytes(&Self::path(), &bytes, file_utils::PublicationMode::Durable)
            .map_err(|err| format!("preferences save failed: {err}"))
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
            document_path: Some(PathBuf::from("/tmp/graph.json")),
            // Non-defaults (defaults are `true`) so the round-trip is meaningful.
            load_last_document: false,
            confirm_unsaved_on_exit: false,
            window: Some(WindowState {
                size: UVec2::new(1440, 900),
                maximized: true,
                position: Some(IVec2::new(120, -40)),
            }),
            // Non-defaults (defaults are Theme + Nearest).
            viewer: ViewerPreferences {
                background: ViewerBackground::Checker,
                mag_filter: ImageFilter::Linear,
            },
            ml_models: MlModelPreferences {
                denoise: PathBuf::from("/models/d.onnx"),
                star_removal: PathBuf::from("/models/s.onnx"),
            },
        };
        let bytes = serialize(&cfg, SerdeFormat::Toml).expect("preferences TOML serializes");
        let text = std::str::from_utf8(&bytes).expect("preferences TOML is UTF-8");
        assert!(text.contains("mag_filter = \"linear\""));
        let back = roundtrip(&cfg);
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.document_path, Some(PathBuf::from("/tmp/graph.json")));
        assert!(!back.load_last_document);
        assert!(!back.confirm_unsaved_on_exit);
        assert_eq!(
            back.window,
            Some(WindowState {
                size: UVec2::new(1440, 900),
                maximized: true,
                position: Some(IVec2::new(120, -40)),
            })
        );
        assert_eq!(
            back.viewer,
            ViewerPreferences {
                background: ViewerBackground::Checker,
                mag_filter: ImageFilter::Linear,
            }
        );
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
        // No remembered window geometry until a session saves one.
        assert_eq!(back.window, None);
        // Viewer toolbar defaults: theme backdrop, nearest sampling.
        assert_eq!(back.viewer, ViewerPreferences::default());
        assert_eq!(back.viewer.background, ViewerBackground::Theme);
        assert_eq!(back.viewer.mag_filter, ImageFilter::Nearest);
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

    #[test]
    fn partial_window_table_fills_missing_fields_and_omits_position() {
        // A hand-edited `[window]` table with only a size (glam vec → `[w, h]`
        // array): the missing `maximized` defaults to `false` and, with no
        // `position` key, the physical position stays `None` (the Wayland case).
        let toml = b"[window]\nsize = [800, 600]\n";
        let cfg: Preferences =
            deserialize(toml, SerdeFormat::Toml).expect("partial window table deserializes");
        assert_eq!(
            cfg.window,
            Some(WindowState {
                size: UVec2::new(800, 600),
                maximized: false,
                position: None,
            })
        );
    }
}
