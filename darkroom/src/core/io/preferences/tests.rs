use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::str;

use common::{SerdeFormat, TempDir, deserialize, serialize};
use glam::{IVec2, UVec2};
use palantir::ImageFilter;

use crate::core::io::preferences::error::PreferencesLoadError;
use crate::core::io::preferences::{
    MlModelPreferences, Preferences, ViewerBackground, ViewerPreferences, WindowState,
};

/// The file is named for the app, and lies in the OS's configuration directory
/// (`platform/*` `resolve_config_dir` tests cover that resolution).
#[test]
fn the_preferences_file_is_named_for_the_app() {
    assert_eq!(
        Preferences::path().file_name(),
        Some(OsStr::new("darkroom.preferences.ron"))
    );
}

fn roundtrip(cfg: &Preferences) -> Preferences {
    let bytes = serialize(cfg, SerdeFormat::Ron).expect("preferences RON serializes");
    deserialize(&bytes, SerdeFormat::Ron).expect("preferences RON round-trips")
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
            position: Some(IVec2::new(120, -40)),
        }),
        viewer: ViewerPreferences {
            background: ViewerBackground::Checker,
            mag_filter: ImageFilter::Linear,
        },
        ml_models: MlModelPreferences {
            denoise: PathBuf::from("/models/d.onnx"),
            star_removal: PathBuf::from("/models/s.onnx"),
        },
    };
    let bytes = serialize(&populated, SerdeFormat::Ron).expect("preferences RON serializes");
    let text = str::from_utf8(&bytes).expect("preferences RON is UTF-8");
    assert!(text.contains("mag_filter: linear"));

    for cfg in [populated, Preferences::default()] {
        assert_eq!(roundtrip(&cfg), cfg);
    }
}

/// `#[serde(default)]` fills every absent key, so a hand-trimmed file loads.
#[test]
fn partial_preferences_fill_defaults() {
    let empty: Preferences = deserialize(b"()", SerdeFormat::Ron).expect("empty preferences");
    assert_eq!(empty, Preferences::default());

    let partial = b"(confirm_unsaved_changes: false)";
    let cfg: Preferences =
        deserialize(partial, SerdeFormat::Ron).expect("partial preferences deserializes");
    assert_eq!(
        cfg,
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
    assert_eq!(defaults.viewer.background, ViewerBackground::Theme);
    assert_eq!(defaults.viewer.mag_filter, ImageFilter::Nearest);
    let models = lens::MlModelPaths::default();
    assert_eq!(defaults.ml_models.denoise, models.denoise);
    assert_eq!(defaults.ml_models.star_removal, models.star_removal);
}

#[test]
fn partial_window_entry_fills_missing_fields_and_omits_position() {
    // A hand-edited `window` entry carrying only a size: the missing
    // `maximized` defaults to `false` and, with no `position` key, the
    // physical position stays `None` (the Wayland case).
    let partial = b"(window: Some((size: (800, 600))))";
    let cfg: Preferences =
        deserialize(partial, SerdeFormat::Ron).expect("partial window entry deserializes");
    assert_eq!(
        cfg.window,
        Some(WindowState {
            size: UVec2::new(800, 600),
            maximized: false,
            position: None,
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
