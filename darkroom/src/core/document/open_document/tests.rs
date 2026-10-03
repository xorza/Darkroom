use std::path::{Path, PathBuf};

use common::TempDir;

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::document::{self, DocumentLoadError};
use crate::core::io::preferences::Preferences;
use crate::core::status::StatusLog;
use scenarium::{DataType, Graph, Library, Node, system_library};

#[test]
fn load_returns_the_document_error() {
    let path = PathBuf::from("not-a-document.json");

    let error = OpenDocument::load(path.clone(), &Library::default()).unwrap_err();

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
    let dir = TempDir::new("darkroom-open-document");
    let path = dir.join("preferred.darkroom");
    document::save(&Document::default(), &path).unwrap();
    let mut preferences = Preferences {
        document_path: Some(path.clone()),
        ..Preferences::default()
    };
    let mut status = StatusLog::default();

    // A remembered path with reopening on comes back as the open document.
    let open = OpenDocument::open_at_launch_with(
        None,
        &mut preferences,
        &mut status,
        &Library::default(),
        |_| Ok(()),
    );
    assert_eq!(open.path, Some(path.clone()));

    // Reopening off: empty document, but the path stays remembered.
    preferences.load_last_document = false;
    let open = OpenDocument::open_at_launch_with(
        None,
        &mut preferences,
        &mut status,
        &Library::default(),
        |_| Ok(()),
    );
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, Some(path));
    assert_eq!(status.lines().count(), 0, "neither path reports a failure");

    // An unloadable path degrades to an empty document and is forgotten;
    // a failing preferences write is reported on top of the load failure.
    preferences.load_last_document = true;
    preferences.document_path = Some("invalid.json".into());
    let open = OpenDocument::open_at_launch_with(
        None,
        &mut preferences,
        &mut status,
        &Library::default(),
        |_| Err("preferences save failed: disk unavailable".into()),
    );
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

#[test]
fn a_command_line_document_outranks_the_remembered_one_and_leaves_it_alone() {
    let dir = TempDir::new("darkroom-open-document-argument");
    let named = dir.join("named.darkroom");
    document::save(&Document::default(), &named).unwrap();
    let remembered = PathBuf::from("remembered.darkroom");
    let mut preferences = Preferences {
        document_path: Some(remembered.clone()),
        // Off, and pointed at an unloadable file: a named document wins over
        // both, and the preferences are untouched on the way through.
        load_last_document: false,
        ..Preferences::default()
    };
    let mut status = StatusLog::default();

    // The `.` component is what proves the path went through
    // `std::path::absolute`: it is the one thing that strips it, and a
    // relative argument is made absolute by that same call. Compared as
    // `OsStr`, since `Path`'s own `==` skips `.` components and would pass
    // either way.
    let argument = dir.join(".").join("named.darkroom");
    assert_ne!(
        argument.as_os_str(),
        named.as_os_str(),
        "the fixture has to differ before the load"
    );
    let open = OpenDocument::open_at_launch_with(
        Some(argument),
        &mut preferences,
        &mut status,
        &Library::default(),
        |_| panic!("a command-line document writes no preferences"),
    );
    assert_eq!(
        open.path.as_deref().map(Path::as_os_str),
        Some(named.as_os_str())
    );
    assert_eq!(preferences.document_path, Some(remembered.clone()));
    assert_eq!(status.lines().count(), 0);

    // An unloadable argument degrades to an empty document rather than to the
    // remembered one, and still leaves the remembered path standing — it is
    // not what failed.
    let missing = dir.join("missing.darkroom");
    let open = OpenDocument::open_at_launch_with(
        Some(missing.clone()),
        &mut preferences,
        &mut status,
        &Library::default(),
        |_| panic!("a command-line document writes no preferences"),
    );
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, Some(remembered));
    // The tail is the OS's own wording for a missing file, so only the part
    // we compose is pinned.
    let lines = status.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one failure, not two: {lines:?}");
    let prefix = format!("load failed: {}: ", missing.display());
    assert!(lines[0].starts_with(&prefix), "got {:?}", lines[0]);
}

/// A node keeps the ports it was authored against. Saved against `Print`, it loads against the
/// same library and adopts nothing new; against a library whose `Print` retyped its input, the
/// load fails and names the node and the func. A node saved before signatures were recorded
/// loads against either and takes the library's.
#[test]
fn a_node_authored_against_other_ports_fails_the_load_by_name() {
    let dir = TempDir::new("darkroom-open-document-signature");
    let path = dir.join("signed.darkroom");
    let library = system_library();
    let print = library.by_name("Print").unwrap();
    let mut graph = Graph::default();
    let node = graph.add(Node::from(print));
    let signature = graph.find(node).unwrap().signature;
    document::save(&Document::from(graph), &path).unwrap();

    let open = OpenDocument::load(path.clone(), &library).unwrap();
    assert_eq!(open.document.graph.find(node).unwrap().signature, signature);

    let mut retyped = print.clone();
    retyped.inputs[0].data_type = DataType::Int;
    let mut changed = Library::default();
    changed.add(retyped);
    let error = OpenDocument::load(path.clone(), &changed).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&format!("{node:?}")) && message.contains("\"Print\""),
        "{message}"
    );

    let mut older = OpenDocument::load(path.clone(), &library).unwrap().document;
    older.graph.find_mut(node).unwrap().signature = None;
    document::save(&older, &path).unwrap();
    for library in [&library, &changed] {
        let open = OpenDocument::load(path.clone(), library).unwrap();
        let expected = scenarium::FuncSignature::of(library.by_name("Print").unwrap());
        assert_eq!(
            open.document.graph.find(node).unwrap().signature,
            Some(expected)
        );
    }
}
