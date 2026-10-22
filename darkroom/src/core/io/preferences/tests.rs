use std::ffi::OsStr;
use std::path::PathBuf;
use std::str;

use common::{SerdeFormat, TempDir, fs};
use glam::{IVec2, UVec2};
use lens::MlModelPaths;
use palantir::ImageFilter;

use crate::core::io::preferences;
use crate::core::io::preferences::error::{PreferencesLoadError, PreferencesSaveError};
use crate::core::io::preferences::{Preferences, ViewerBg, ViewerPreferences, WindowState};

/// The file is named for the app, and lies in the OS's configuration directory
/// (`platform/*` `resolve_config_dir` tests cover that resolution).
#[test]
fn the_preferences_file_is_named_for_the_app() {
    assert_eq!(
        preferences::path().file_name(),
        Some(OsStr::new("darkroom.preferences.ron"))
    );
}

fn roundtrip(preferences: &Preferences) -> Preferences {
    let bytes = SerdeFormat::Ron
        .serialize(preferences)
        .expect("preferences RON serializes");
    SerdeFormat::Ron
        .deserialize(&bytes)
        .expect("preferences RON round-trips")
}

/// A populated file and the default both read back equal, field for field.
#[test]
fn preferences_roundtrip() {
    let populated = Preferences {
        document_path: Some(PathBuf::from("/tmp/graph.darkroom")),
        // Every field off its default, so a field the round trip drops fails.
        load_last_document: false,
        confirm_unsaved_changes: false,
        window: Some(WindowState {
            size: UVec2::new(1440, 900),
            maximized: true,
            pos: Some(IVec2::new(120, -40)),
        }),
        viewer: ViewerPreferences {
            bg: ViewerBg::Checker,
            mag_filter: ImageFilter::Linear,
        },
        ml_models: MlModelPaths {
            denoise: PathBuf::from("/models/d.onnx"),
            star_removal: PathBuf::from("/models/s.onnx"),
        },
    };
    let bytes = SerdeFormat::Ron
        .serialize(&populated)
        .expect("preferences RON serializes");
    let text = str::from_utf8(&bytes).expect("preferences RON is UTF-8");
    assert!(text.contains("mag_filter: linear"));

    for preferences in [populated, Preferences::default()] {
        assert_eq!(roundtrip(&preferences), preferences);
    }
}

/// `#[serde(default)]` fills every absent key, so a hand-trimmed file loads.
#[test]
fn partial_preferences_fill_defaults() {
    let empty: Preferences = SerdeFormat::Ron
        .deserialize(b"()")
        .expect("empty preferences");
    assert_eq!(empty, Preferences::default());

    let partial = b"(confirm_unsaved_changes: false)";
    let preferences: Preferences = SerdeFormat::Ron
        .deserialize(partial)
        .expect("partial preferences deserializes");
    assert_eq!(
        preferences,
        Preferences {
            confirm_unsaved_changes: false,
            ..Preferences::default()
        }
    );
}

/// The default reopens the last document and asks before it discards changes,
/// remembers no window, and shows the theme backdrop with nearest sampling.
#[test]
fn default_preferences() {
    let defaults = Preferences::default();
    assert_eq!(defaults.document_path, None);
    assert!(defaults.load_last_document);
    assert!(defaults.confirm_unsaved_changes);
    assert_eq!(defaults.window, None);
    assert_eq!(defaults.viewer.bg, ViewerBg::Theme);
    assert_eq!(defaults.viewer.mag_filter, ImageFilter::Nearest);
    assert_eq!(defaults.ml_models, MlModelPaths::default());
}

#[test]
fn partial_window_entry_fills_missing_fields_and_omits_position() {
    // A hand-edited `window` entry carrying only a size: the missing
    // `maximized` defaults to `false` and, with no `pos` key, the
    // physical position stays `None` (the Wayland case).
    let partial = b"(window: Some((size: (800, 600))))";
    let preferences: Preferences = SerdeFormat::Ron
        .deserialize(partial)
        .expect("partial window entry deserializes");
    assert_eq!(
        preferences.window,
        Some(WindowState {
            size: UVec2::new(800, 600),
            maximized: false,
            pos: None,
        })
    );
}

/// A missing file is a first run and reads as the defaults; a file that is
/// there but unreadable or unparsable is an error naming it, so the caller can
/// report it rather than overwrite it with defaults.
#[test]
fn only_a_missing_file_reads_as_the_defaults() {
    let dir = TempDir::new("darkroom-preferences-load");
    let path = dir.join("darkroom.preferences.ron");

    assert_eq!(
        Preferences::load_from(&path).unwrap(),
        Preferences::default()
    );

    fs::write(&path, b"(confirm_unsaved_changes: false)").unwrap();
    assert_eq!(
        Preferences::load_from(&path).unwrap(),
        Preferences {
            confirm_unsaved_changes: false,
            ..Preferences::default()
        }
    );

    fs::write(&path, b"(confirm_unsaved_changes: maybe)").unwrap();
    assert!(matches!(
        Preferences::load_from(&path),
        Err(PreferencesLoadError::Parse { path: failed, .. }) if failed == path
    ));

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        Preferences::load_from(&path),
        Err(PreferencesLoadError::Read { path: failed, .. }) if failed == path
    ));
}

/// A save creates the missing configuration directory and writes a file the
/// next load reads back whole; a save whose directory cannot be made, or
/// whose target cannot be written, names the path that failed.
#[test]
fn a_save_reads_back_and_a_failed_one_names_the_path() {
    let dir = TempDir::new("darkroom-preferences-save");
    let path = dir.join("config").join("darkroom.preferences.ron");
    let saved = Preferences {
        load_last_document: false,
        ..Preferences::default()
    };
    saved.save_to(&path).unwrap();
    assert_eq!(Preferences::load_from(&path).unwrap(), saved);

    let blocker = dir.join("not-a-dir");
    fs::write(&blocker, b"").unwrap();
    let under_file = blocker.join("darkroom.preferences.ron");
    assert!(matches!(
        saved.save_to(&under_file),
        Err(PreferencesSaveError::CreateDir { path: failed, .. }) if failed == blocker
    ));

    let occupied = dir.join("occupied");
    fs::create_dir(&occupied).unwrap();
    assert!(matches!(
        saved.save_to(&occupied),
        Err(PreferencesSaveError::Write { path: failed, .. }) if failed == occupied
    ));
}
