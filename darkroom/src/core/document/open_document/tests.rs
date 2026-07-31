use std::path::PathBuf;

use common::internals::test_output_path;

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::document::{self, DocumentLoadError};
use crate::core::io::preferences::Preferences;
use crate::core::status::StatusLog;

#[test]
fn load_returns_the_document_error() {
    let path = PathBuf::from("not-a-document.json");

    let error = OpenDocument::load(path.clone()).unwrap_err();

    assert!(matches!(
        error,
        DocumentLoadError::InvalidExtension { path: error_path } if error_path == path
    ));
}

#[test]
fn empty_document_has_the_main_graph_tab() {
    let open = OpenDocument::default();

    assert!(open.path.is_none());
    assert_eq!(open.document.layout.all_tabs().count(), 1);
}

#[test]
fn preferred_document_reopens_and_a_failed_load_forgets_the_path() {
    let path = test_output_path("darkroom_open_document/preferred.darkroom");
    document::save(&Document::default(), &path).unwrap();
    let mut preferences = Preferences {
        document_path: Some(path.clone()),
        ..Preferences::default()
    };
    let mut status = StatusLog::default();

    // A remembered path with reopening on comes back as the open document.
    let open = OpenDocument::load_preferred_with(&mut preferences, &mut status, |_| Ok(()));
    assert_eq!(open.path, Some(path.clone()));

    // Reopening off: empty document, but the path stays remembered.
    preferences.load_last_document = false;
    let open = OpenDocument::load_preferred_with(&mut preferences, &mut status, |_| Ok(()));
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, Some(path));
    assert_eq!(status.lines().count(), 0, "neither path reports a failure");

    // An unloadable path degrades to an empty document and is forgotten;
    // a failing preferences write is reported on top of the load failure.
    preferences.load_last_document = true;
    preferences.document_path = Some("invalid.json".into());
    let open = OpenDocument::load_preferred_with(&mut preferences, &mut status, |_| {
        Err("preferences save failed: disk unavailable".into())
    });
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, None);
    assert_eq!(
        status.lines().collect::<Vec<_>>(),
        [
            "load failed: invalid.json must use the .darkroom extension",
            "preferences save failed: disk unavailable"
        ]
    );
}
