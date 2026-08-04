use std::path::{Path, PathBuf};

use common::{SerdeFormat, deserialize, file_utils, serialize};
use glam::{IVec2, UVec2};
use palantir::ImageFilter;

use crate::core::theme_pref::ThemeChoice;

/// Preferences file name, resolved beside the running executable. TOML so
/// it's hand-editable and matches the theme on-disk format.
const PREFERENCES_FILE: &str = "darkroom.preferences.toml";

/// Persisted session state: the theme preference to restore, the
/// document open when the app last closed, and editor behavior.
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
    pub(crate) theme: ThemeChoice,
    /// Document to reopen on launch. `None` starts with an empty doc.
    pub(crate) document_path: Option<PathBuf>,
    /// Reopen `document_path` on launch. When `false`, launch starts with
    /// an empty document (the path is still remembered, just not opened).
    /// Defaults to `true` — the historical reopen-where-you-left-off behavior.
    pub(crate) load_last_document: bool,
    /// Prompt to save unsaved changes before any transition that would
    /// discard them — window close, ⌘Q, File ▸ Quit, File ▸ New, File ▸
    /// Open. When `false`, those proceed without asking. The prompt's
    /// "Don't ask again" checkbox clears it; the Preferences tab can
    /// restore it. Defaults to `true`.
    pub(crate) confirm_unsaved_changes: bool,
    /// Main window geometry from the last session, restored at launch so
    /// the editor reopens at the same size / position. `None` on first run
    /// (platform picks). A TOML `[window]` table — a table field, so it
    /// sits with the other tables after every scalar key.
    pub(crate) window: Option<WindowState>,
    /// Image-viewer toolbar choices (backdrop + magnification sampling),
    /// shared by all viewer tabs: a toolbar click in any viewer edits this
    /// in place and persists. A TOML `[viewer]` table.
    pub(crate) viewer: ViewerPreferences,
    /// Default ONNX model paths copied into newly-authored ML node inputs.
    pub(crate) ml_models: MlModelPreferences,
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
    pub(crate) background: ViewerBackground,
    /// Magnification sampling for the shown image. Defaults to `Nearest`
    /// for pixel peeping; zoomed-out minification always stays linear.
    pub(crate) mag_filter: ImageFilter,
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
    pub(crate) size: UVec2,
    pub(crate) maximized: bool,
    /// Physical outer position `[x, y]`; `None` on Wayland.
    pub(crate) position: Option<IVec2>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            document_path: None,
            load_last_document: true,
            confirm_unsaved_changes: true,
            window: None,
            viewer: ViewerPreferences::default(),
            ml_models: MlModelPreferences::default(),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct MlModelPreferences {
    pub(crate) denoise: PathBuf,
    pub(crate) star_removal: PathBuf,
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
        Self {
            denoise: preferences.denoise.clone(),
            star_removal: preferences.star_removal.clone(),
        }
    }
}

impl Preferences {
    /// The preferences file, beside the running executable rather than in the
    /// working directory — so the same install reads back the same settings
    /// however it was launched, instead of one file per directory a shell
    /// happened to be in.
    ///
    /// `current_exe` resolves symlinks, so a launcher symlinked onto `PATH`
    /// resolves to wherever the real binary sits — for a cargo build, inside
    /// `target/`, which `cargo clean` removes. `cargo install` puts the real
    /// binary on `PATH` and settles that.
    ///
    /// A failure to locate the executable at all leaves the file name bare,
    /// which resolves against the working directory. Degrading beats
    /// panicking: everything else here already falls back rather than block
    /// a launch over settings.
    fn path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_default()
            .join(PREFERENCES_FILE)
    }

    /// Read the preferences from beside the executable. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt preferences file shouldn't brick the app.
    pub(crate) fn load() -> Self {
        match std::fs::read(Self::path()) {
            Ok(bytes) => deserialize(&bytes, SerdeFormat::Toml).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Write the preferences beside the executable. `Err` carries the
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
mod tests;
