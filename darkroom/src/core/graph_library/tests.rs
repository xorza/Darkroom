use std::io;
use std::path::{Path, PathBuf};

use scenarium::{Graph, GraphId};

use crate::core::graph_library::GraphLibrary;
use crate::core::io::graph_library::GraphLibrarySaveError;

#[test]
fn edit_distinguishes_noop_success_and_failed_persistence() {
    let mut library = GraphLibrary::default();
    let noop = library.edit_with(|_| false, |_| panic!("a no-op must not persist"));
    assert!(!noop.changed);
    assert!(noop.persist_error.is_none());

    let saved_id = GraphId::unique();
    let saved = library.edit_with(
        |library| {
            library.graphs.insert(saved_id, Graph::new("saved"));
            true
        },
        |_| Ok(()),
    );
    assert!(saved.changed);
    assert!(saved.persist_error.is_none());
    assert_eq!(library.graphs.get(&saved_id).unwrap().name, "saved");

    let unsaved_id = GraphId::unique();
    let failed = library.edit_with(
        |library| {
            library.graphs.insert(unsaved_id, Graph::new("memory only"));
            true
        },
        |_| {
            Err(GraphLibrarySaveError::Publish {
                path: PathBuf::from("graph-library.json"),
                source: io::Error::other("disk unavailable"),
            })
        },
    );
    assert!(failed.changed);
    assert!(
        matches!(
            failed.persist_error,
            Some(GraphLibrarySaveError::Publish { path, source })
                if path == Path::new("graph-library.json")
                    && source.kind() == io::ErrorKind::Other
                    && source.to_string() == "disk unavailable"
        ),
        "the exact persistence failure is retained"
    );
    assert_eq!(
        library.graphs.get(&unsaved_id).unwrap().name,
        "memory only",
        "a failed save does not roll back the active session"
    );
}
