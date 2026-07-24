use std::collections::HashMap;

use common::test_utils::test_output_path;
use scenarium::{Graph, GraphEvent, GraphId, NodeId};

use crate::core::graph_library::GraphLibrary;
use crate::core::io::graph_library::{
    GraphLibraryLoadError, GraphLibraryReadError, broken_path, load_from, save_to,
};

fn graph(name: &str) -> Graph {
    Graph::new(name).category("test")
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
    save_to(&path, &library).unwrap();

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
    save_to(&path, &recovered).unwrap();
    assert_eq!(load_from(&path).unwrap().graphs, recovered.graphs);
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);
}

#[test]
fn structurally_invalid_graph_is_quarantined() {
    let path = test_output_path("darkroom_graph_library/invalid-graph.json");
    let _ = std::fs::remove_file(&path);
    let mut bad = graph("dangling");
    bad.definition.as_mut().unwrap().events.push(GraphEvent {
        name: "tick".into(),
        emitter: NodeId::unique(),
        emitter_event_idx: 0,
    });
    let library = GraphLibrary {
        graphs: HashMap::from([(GraphId::unique(), bad)]),
    };
    save_to(&path, &library).unwrap();

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

    let error = format!("{:#}", save_to(&path, &library(["x"])).unwrap_err());
    assert!(error.contains(path.to_str().unwrap()), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
}

#[test]
fn unwritable_path_reports_save_failure() {
    let path = test_output_path("darkroom_graph_library/no-such-dir").join("library.json");
    if path.parent().unwrap().exists() {
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    let error = format!("{:#}", save_to(&path, &library(["x"])).unwrap_err());
    assert!(error.contains(path.to_str().unwrap()), "{error}");
}
