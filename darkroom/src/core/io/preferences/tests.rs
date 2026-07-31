use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};
use glam::{IVec2, UVec2};
use palantir::ImageFilter;

use crate::core::io::preferences::{
    MlModelPreferences, Preferences, ViewerBackground, ViewerPreferences, WindowState,
};
use crate::core::theme_pref::ThemeChoice;

fn roundtrip(cfg: &Preferences) -> Preferences {
    let bytes = serialize(cfg, SerdeFormat::Toml).expect("preferences TOML serializes");
    deserialize(&bytes, SerdeFormat::Toml).expect("preferences TOML round-trips")
}

#[test]
fn populated_preferences_roundtrips() {
    let cfg = Preferences {
        theme: ThemeChoice::Light,
        document_path: Some(PathBuf::from("/tmp/graph.darkroom")),
        // Non-defaults (defaults are `true`) so the round-trip is meaningful.
        load_last_document: false,
        confirm_unsaved_changes: false,
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
    assert_eq!(
        back.document_path,
        Some(PathBuf::from("/tmp/graph.darkroom"))
    );
    assert_eq!(back.ml_models.denoise, PathBuf::from("/models/d.onnx"));
    assert_eq!(back.ml_models.star_removal, PathBuf::from("/models/s.onnx"));
    assert!(!back.load_last_document);
    assert!(!back.confirm_unsaved_changes);
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
    assert!(back.confirm_unsaved_changes);
    // No remembered window geometry until a session saves one.
    assert_eq!(back.window, None);
    // Viewer toolbar defaults: theme backdrop, nearest sampling.
    assert_eq!(back.viewer, ViewerPreferences::default());
    assert_eq!(back.viewer.background, ViewerBackground::Theme);
    assert_eq!(back.viewer.mag_filter, ImageFilter::Nearest);
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
fn partial_preferences_fill_defaults() {
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
