use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str;

use common::{SerdeFormat, TempDir, fs};
use glam::IVec2;
use lens::MlModelPaths;
use palantir::{ColorScheme, ImageFilter, Preference, Size};

use crate::core::io::preferences::error::{
    PreferencesLoadError, PreferencesParseError, PreferencesReadError, PreferencesSaveError,
    PreferencesSetAsideError,
};
use crate::core::io::preferences::interface_size::InterfaceSize;
use crate::core::io::preferences::memory_headroom::MemoryHeadroom;
use crate::core::io::preferences::preferences_store::{
    BACKUP_COUNT_MAX, OpenFailure, PreferencesStore, backup_name,
};
use crate::core::io::preferences::recent_documents::RecentDocuments;
use crate::core::io::preferences::run_mode::RunMode;
use crate::core::io::preferences::{Preferences, ViewerBg, ViewerPreferences, WindowState};

/// The file is named for the app, and lies in the OS's configuration directory
/// (`platform/*` `resolve_config_dir` tests cover that resolution).
#[test]
fn the_preferences_file_is_named_for_the_app() {
    let PreferencesStore::File(path) = PreferencesStore::user() else {
        panic!("the user's store is a file");
    };
    assert_eq!(
        path.file_name(),
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
    let mut recent = RecentDocuments::default();
    recent.note(Path::new("/tmp/graph.darkroom"));
    let populated = Preferences {
        document_path: Some(PathBuf::from("/tmp/graph.darkroom")),
        recent,
        // Every field off its default, so a field the round trip drops fails.
        load_last_document: false,
        confirm_unsaved_changes: false,
        relative_paths: false,
        interface_size: InterfaceSize::OFFERED[4],
        color_scheme: Preference::Fixed(ColorScheme::Light),
        run_mode: RunMode::Manual,
        memory_headroom: MemoryHeadroom::Gib(4),
        window: Some(WindowState {
            size: Size::new(1440.0, 900.0),
            maximized: true,
            physical_pos: Some(IVec2::new(120, -40)),
        }),
        viewer: ViewerPreferences {
            bg: ViewerBg::Checker,
            mag_filter: ImageFilter::Linear,
        },
        // One model set and one unset, so both shapes round-trip.
        ml_models: MlModelPaths {
            denoise: Some(PathBuf::from("/models/d.onnx")),
            star_removal: None,
        },
        library_folder: Some(PathBuf::from("/library")),
    };
    let bytes = SerdeFormat::Ron
        .serialize(&populated)
        .expect("preferences RON serializes");
    let text = str::from_utf8(&bytes).expect("preferences RON is UTF-8");
    assert!(text.contains("mag_filter: linear"));
    assert!(text.contains("memory_headroom: gib(4)"), "{text}");
    assert!(
        text.contains("interface_size: 125"),
        "a size saves as its percent"
    );
    assert!(text.contains("color_scheme: Fixed(Light)"), "{text}");

    for preferences in [populated, Preferences::default()] {
        assert_eq!(roundtrip(&preferences), preferences);
    }
}

/// `#[serde(default)]` fills every absent key, so a hand-trimmed file loads, and a size no rung
/// offers loads as the nearest one. A `window` entry
/// with only a size defaults `maximized` to `false`, and with no `physical_pos` key the
/// position stays `None`, as on Wayland.
#[test]
fn partial_preferences_fill_defaults() {
    for (text, expected) in [
        (&b"()"[..], Preferences::default()),
        (
            b"(interface_size: 117)",
            Preferences {
                interface_size: InterfaceSize::OFFERED[3],
                ..Preferences::default()
            },
        ),
        (
            b"(color_scheme: System)",
            Preferences {
                color_scheme: Preference::System,
                ..Preferences::default()
            },
        ),
        (
            b"(confirm_unsaved_changes: false)",
            Preferences {
                confirm_unsaved_changes: false,
                ..Preferences::default()
            },
        ),
        (
            b"(window: Some((size: (800, 600.5))))",
            Preferences {
                window: Some(WindowState {
                    size: Size::new(800.0, 600.5),
                    maximized: false,
                    physical_pos: None,
                }),
                ..Preferences::default()
            },
        ),
    ] {
        let preferences: Preferences = SerdeFormat::Ron
            .deserialize(text)
            .expect("partial preferences deserialize");
        assert_eq!(preferences, expected, "{}", String::from_utf8_lossy(text));
    }
    // A file that names no scheme opens dark, as a fresh install does.
    assert_eq!(
        Preferences::default().color_scheme,
        Preference::Fixed(ColorScheme::Dark)
    );
    // A window size palantir cannot open is refused at the load, not at the window.
    for text in [
        b"(window: Some((size: (0, 600))))".as_slice(),
        b"(window: Some((size: (800, inf))))",
        b"(window: Some((maximized: true)))",
    ] {
        let read: Result<Preferences, _> = SerdeFormat::Ron.deserialize(text);
        assert!(read.is_err(), "{}", String::from_utf8_lossy(text));
    }
}

/// Each field but the window's is a choice: a change to it reads as different, and a change to the
/// window alone reads as the same.
#[test]
fn every_field_but_the_window_is_a_choice() {
    let base = Preferences::default();
    assert!(base.same_choices(&base.clone()));
    let edits: [fn(&mut Preferences); 8] = [
        |p| p.document_path = Some(PathBuf::from("a.darkroom")),
        |p| p.load_last_document = !p.load_last_document,
        |p| p.confirm_unsaved_changes = !p.confirm_unsaved_changes,
        |p| p.relative_paths = !p.relative_paths,
        |p| {
            p.interface_size = *InterfaceSize::OFFERED
                .iter()
                .find(|size| **size != p.interface_size)
                .expect("more than one size is offered");
        },
        |p| p.color_scheme = Preference::System,
        |p| p.viewer.bg = ViewerBg::Black,
        |p| p.ml_models.denoise = Some(PathBuf::from("b.onnx")),
    ];
    for (idx, edit) in edits.into_iter().enumerate() {
        let mut edited = base.clone();
        edit(&mut edited);
        assert!(!edited.same_choices(&base), "edit {idx}");
    }
    let mut resized = base.clone();
    resized.window = Some(WindowState {
        size: Size::new(640.0, 480.0),
        maximized: false,
        physical_pos: None,
    });
    assert!(resized.same_choices(&base), "the window is no choice");
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
    assert_eq!(
        defaults.ml_models,
        MlModelPaths {
            denoise: None,
            star_removal: None,
        },
        "no model is set until a person picks one"
    );
    assert!(
        defaults.relative_paths,
        "paths inside the folder save relative"
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
        Err(PreferencesLoadError::Parse(PreferencesParseError { path: failed, .. })) if failed == path
    ));

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        Preferences::load_from(&path),
        Err(PreferencesLoadError::Read(PreferencesReadError { path: failed, .. })) if failed == path
    ));
}

/// A file that does not parse moves to the first free backup name beside it, and the session
/// starts on the defaults and keeps saving to the file: the first to `<stem>.invalid.ron`, the next
/// to `<stem>.invalid-2.ron`, each holding the bytes it held. With every backup name taken, or a
/// file that cannot be read, the file stays as it is and the session runs in memory.
#[test]
fn a_file_that_does_not_parse_is_set_aside_and_one_unreadable_is_kept() {
    let dir = TempDir::new("darkroom-preferences-open");
    let path = dir.join("darkroom.preferences.ron");
    assert_eq!(
        backup_name(&path, 1),
        dir.join("darkroom.preferences.invalid.ron")
    );
    assert_eq!(
        backup_name(&path, 2),
        dir.join("darkroom.preferences.invalid-2.ron")
    );
    assert_eq!(
        backup_name(Path::new("prefs"), 3),
        Path::new("prefs.invalid-3")
    );

    for (n, bytes) in [
        (1, b"not ron".as_slice()),
        (2, b"(run_mode: sometimes)".as_slice()),
    ] {
        fs::write(&path, bytes).unwrap();
        let opened = PreferencesStore::File(path.clone()).open();
        assert_eq!(opened.preferences, Preferences::default());
        assert_eq!(opened.store, PreferencesStore::File(path.clone()));
        let backup = backup_name(&path, n);
        assert!(
            matches!(
                opened.failure,
                Some(OpenFailure::SetAside {
                    failure: PreferencesParseError { path: ref failed, .. },
                    backup: ref moved,
                }) if *failed == path && *moved == backup
            ),
            "{:?}",
            opened.failure
        );
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        assert!(!fs::exists(&path), "the file moved");
    }

    for n in 3..=BACKUP_COUNT_MAX {
        fs::write(backup_name(&path, n), b"").unwrap();
    }
    fs::write(&path, b"not ron").unwrap();
    let opened = PreferencesStore::File(path.clone()).open();
    assert_eq!(opened.store, PreferencesStore::Memory);
    assert!(
        matches!(
            opened.failure,
            Some(OpenFailure::Kept {
                failure: PreferencesParseError { .. },
                set_aside: PreferencesSetAsideError::Exhausted { path: ref failed },
            }) if *failed == path
        ),
        "{:?}",
        opened.failure
    );
    assert_eq!(fs::read(&path).unwrap(), b"not ron", "the file stays");
    assert_eq!(
        fs::read(backup_name(&path, 1)).unwrap(),
        b"not ron",
        "no backup is overwritten"
    );

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let opened = PreferencesStore::File(path.clone()).open();
    assert_eq!(opened.store, PreferencesStore::Memory);
    assert!(
        matches!(
            opened.failure,
            Some(OpenFailure::Unreadable(PreferencesReadError { path: ref failed, .. }))
                if *failed == path
        ),
        "{:?}",
        opened.failure
    );
    assert!(fs::is_dir(&path), "the unreadable file stays");
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
