use std::path::PathBuf;

use crate::core::io::initial_document;
use crate::core::io::preferences::Preferences;

#[test]
fn disabled_reopen_ignores_the_remembered_path() {
    let preferences = Preferences {
        document_path: Some(PathBuf::from("does-not-exist.darkroom")),
        load_last_document: false,
        ..Preferences::default()
    };

    let initial = initial_document::load(&preferences);

    assert!(initial.open_document.path.is_none());
    assert!(initial.load_error.is_none());
}

#[test]
fn failed_reopen_reports_error_and_returns_an_empty_document() {
    let preferences = Preferences {
        document_path: Some(PathBuf::from("does-not-exist.darkroom")),
        ..Preferences::default()
    };

    let initial = initial_document::load(&preferences);

    assert!(initial.open_document.path.is_none());
    assert!(initial.load_error.is_some());
    assert!(initial.open_document.document.graph.is_empty());
}
