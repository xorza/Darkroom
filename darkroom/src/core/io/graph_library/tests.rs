use std::collections::HashMap;

use common::internals::test_output_path;
use scenarium::{GraphDef, GraphEvent, GraphId, NodeId};

use crate::core::graph_library::GraphLibrary;
use crate::core::io::graph_library::{
    GraphLibraryLoadError, GraphLibraryReadError, LibraryEntry, broken_path, commit_entry_to,
    load_from, write_library,
};

fn graph(name: &str) -> GraphDef {
    GraphDef::new(name).category("test")
}

fn entry(origin: Option<GraphId>, name: &str) -> LibraryEntry {
    LibraryEntry {
        origin,
        graph: graph(name),
    }
}

fn library<const N: usize>(names: [&str; N]) -> GraphLibrary {
    GraphLibrary {
        graphs: names
            .into_iter()
            .map(|name| (GraphId::unique(), graph(name)))
            .collect(),
    }
}

#[test]
fn save_load_roundtrip() {
    let path = test_output_path("darkroom_graph_library/roundtrip.json");
    let _ = std::fs::remove_file(&path);
    let library = library(["blur", "sharpen"]);
    write_library(&path, &library).unwrap();

    assert_eq!(load_from(&path).unwrap().graphs, library.graphs);
}

#[test]
fn missing_file_is_empty_and_not_an_error() {
    let path = test_output_path("darkroom_graph_library/never-written.json");
    assert!(load_from(&path).unwrap().graphs.is_empty());
}

#[test]
fn corrupt_file_is_quarantined_and_the_slot_reusable() {
    let path = test_output_path("darkroom_graph_library/corrupt.json");
    let broken = broken_path(&path);
    let garbage = r#"{"graphs": [ truncated"#;
    std::fs::write(&path, garbage).unwrap();

    let error = load_from(&path).unwrap_err();
    assert!(
        matches!(
            &error,
            GraphLibraryLoadError::Quarantined { source, broken_path }
                if matches!(
                    source.as_ref(),
                    GraphLibraryReadError::Deserialize { path: error_path, .. }
                        if error_path == &path
                ) && broken_path == &broken
        ),
        "parse failure is quarantined with both exact paths: {error}"
    );
    let message = error.to_string();
    assert!(
        message.contains(path.to_str().unwrap()) && message.contains(broken.to_str().unwrap()),
        "error names the file and the backup: {message}"
    );
    assert!(!path.exists(), "the corrupt file was moved aside");
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);

    let recovered = library(["recovered"]);
    write_library(&path, &recovered).unwrap();
    assert_eq!(load_from(&path).unwrap().graphs, recovered.graphs);
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);
}

#[test]
fn structurally_invalid_graph_is_quarantined() {
    let path = test_output_path("darkroom_graph_library/invalid-graph.json");
    let _ = std::fs::remove_file(&path);
    let mut bad = graph("dangling");
    bad.events.push(GraphEvent {
        name: "tick".into(),
        emitter: NodeId::unique(),
        emitter_event_idx: 0,
    });
    let library = GraphLibrary {
        graphs: HashMap::from([(GraphId::unique(), bad)]),
    };
    write_library(&path, &library).unwrap();

    let error = load_from(&path).unwrap_err();
    assert!(
        matches!(
            &error,
            GraphLibraryLoadError::Quarantined { source, .. }
                if matches!(
                    source.as_ref(),
                    GraphLibraryReadError::InvalidGraph {
                        path: error_path,
                        graph_name,
                        ..
                    } if error_path == &path && graph_name == "dangling"
                )
        ),
        "structural failure retains its typed context: {error}"
    );
    assert!(!path.exists(), "the invalid file was moved aside");
}

#[test]
fn save_refuses_to_overwrite_an_unreadable_file() {
    let path = test_output_path("darkroom_graph_library/corrupt-at-save.json");
    let garbage = "not a graph library";
    std::fs::write(&path, garbage).unwrap();

    let error = format!(
        "{:#}",
        commit_entry_to(&path, entry(None, "x")).unwrap_err()
    );
    assert!(error.contains(path.to_str().unwrap()), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
}

#[test]
fn unwritable_path_reports_save_failure() {
    let path = test_output_path("darkroom_graph_library/no-such-dir").join("library.json");
    if path.parent().unwrap().exists() {
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    let error = format!(
        "{:#}",
        commit_entry_to(&path, entry(None, "x")).unwrap_err()
    );
    assert!(error.contains(path.to_str().unwrap()), "{error}");
}

#[test]
fn committing_merges_into_the_file_rather_than_overwriting_it() {
    // The lost-update guard. Each instance holds its own snapshot, so a
    // commit that wrote the caller's whole library would drop everything
    // added since that snapshot was taken.
    let path = test_output_path("darkroom_graph_library/merge.json");
    let _ = std::fs::remove_file(&path);

    let ours = commit_entry_to(&path, entry(None, "ours")).unwrap();
    // Stand in for a second instance publishing between our read and write.
    let theirs = commit_entry_to(&path, entry(None, "theirs")).unwrap();
    assert_ne!(ours.id, theirs.id, "independent adds get independent ids");

    let on_disk = load_from(&path).unwrap();
    assert_eq!(on_disk.graphs.len(), 2, "both entries survive");
    assert_eq!(on_disk.graphs[&ours.id].name, "ours");
    assert_eq!(on_disk.graphs[&theirs.id].name, "theirs");
    assert_eq!(
        theirs.library.graphs.len(),
        2,
        "the committer adopts the merged library, not just its own entry"
    );
}

#[test]
fn an_origin_is_reused_only_while_the_file_still_holds_it() {
    // Republishing updates the entry in place; if that entry is gone from
    // the file — deleted by another instance — the commit mints a fresh id
    // instead of resurrecting a dead one.
    let path = test_output_path("darkroom_graph_library/origin.json");
    let _ = std::fs::remove_file(&path);

    let first = commit_entry_to(&path, entry(None, "v1")).unwrap();
    let second = commit_entry_to(&path, entry(Some(first.id), "v2")).unwrap();
    assert_eq!(second.id, first.id, "a live origin is updated in place");
    assert_eq!(second.library.graphs.len(), 1, "no duplicate entry");
    assert_eq!(second.library.graphs[&second.id].name, "v2");

    let stale = GraphId::unique();
    let third = commit_entry_to(&path, entry(Some(stale), "v3")).unwrap();
    assert_ne!(
        third.id, stale,
        "an origin the file lost becomes a fresh id"
    );
    assert_eq!(third.library.graphs.len(), 2);
}
