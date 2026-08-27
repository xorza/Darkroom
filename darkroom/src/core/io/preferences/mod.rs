use std::path::PathBuf;

use common::{SerdeFormat, deserialize, file_utils, serialize};
use glam::{IVec2, UVec2};
use palantir::ImageFilter;

use crate::platform;

/// Preferences file name, resolved inside the platform's configuration
/// directory. RON so it's hand-editable and matches every other file the app
/// reads and writes.
const PREFERENCES_FILE: &str = "darkroom.preferences.ron";

/// Persisted session state: the document open when the app last closed,
/// and editor behavior.
/// Reloaded on startup so darkroom reopens where the user left off.
/// Missing / unreadable preferences fall back to `default()`.
/// `#[serde(default)]` so a partial preferences file still deserializes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
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
    /// (platform picks).
    pub(crate) window: Option<WindowState>,
    /// Image-viewer toolbar choices (backdrop + magnification sampling),
    /// shared by all viewer tabs: a toolbar click in any viewer edits this
    /// in place and persists.
    pub(crate) viewer: ViewerPreferences,
    /// Default ONNX model paths copied into newly-authored ML node inputs.
    pub(crate) ml_models: MlModelPreferences,
}

/// Backdrop behind (and around) a viewer's image, as offered by the
/// viewer toolbar's swatch row. A frontend-agnostic persisted choice.
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
    /// The preferences file, in the OS's own configuration directory (see
    /// [`crate::platform::config_dir`]) — so the same user reads back the same
    /// settings however the editor was launched and wherever it is installed.
    ///
    /// Resolving beside the executable instead would tie the file to the
    /// install location, and every packaged install puts that somewhere the
    /// user cannot write: the flatpak's `/app/bin` is a read-only mount, a
    /// `PREFIX=/usr` install needs root, and writing inside a macOS `.app`
    /// invalidates its code signature.
    ///
    /// A platform that can name no home leaves the file name bare, which
    /// resolves against the working directory. Degrading beats panicking:
    /// everything else here already falls back rather than block a launch
    /// over settings.
    fn path() -> PathBuf {
        platform::config_dir()
            .unwrap_or_default()
            .join(PREFERENCES_FILE)
    }

    /// Read the preferences from the configuration directory. Any failure (missing
    /// file, parse error) degrades to the default rather than
    /// blocking startup — a corrupt preferences file shouldn't brick the app.
    pub(crate) fn load() -> Self {
        match std::fs::read(Self::path()) {
            Ok(bytes) => deserialize(&bytes, SerdeFormat::Ron).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Write the preferences to the configuration directory. `Err` carries the
    /// display-ready reason — the caller surfaces it (status bar); a
    /// failed persist shouldn't interrupt the user's session.
    pub(crate) fn save(&self) -> Result<(), String> {
        let bytes = serialize(self, SerdeFormat::Ron)
            .map_err(|err| format!("preferences save failed: {err}"))?;
        let path = Self::path();
        // Nothing has created the configuration directory on a first run, and
        // publication needs it to exist to place its temporary file beside the
        // target. Skipped for the bare-name fallback, whose parent is empty.
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("preferences save failed: {err}"))?;
        }
        file_utils::publish_bytes(&path, &bytes, file_utils::PublicationMode::Durable)
            .map_err(|err| format!("preferences save failed: {err}"))
    }
}

#[cfg(test)]
mod tests;
