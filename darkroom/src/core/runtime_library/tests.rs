use std::io;
use std::path::{Path, PathBuf};

use lens::MlModelPaths;
use scenarium::{Graph, GraphId, StaticValue};

use crate::core::graph_library::GraphLibrary;
use crate::core::io::graph_library::GraphLibrarySaveError;
use crate::core::runtime_library::RuntimeLibrary;

#[test]
fn runtime_library_recomposes_builtins_graphs_and_ml_defaults() {
    let graph_id = GraphId::unique();
    let mut graphs = GraphLibrary::default();
    graphs.graphs.insert(graph_id, Graph::new("shared"));
    let defaults = MlModelPaths::default();
    let mut library = RuntimeLibrary::with_graph_library(&defaults, graphs);

    let current = library.published.load();
    assert!(current.by_name("Watch Directory").is_some());
    assert!(current.by_name("Random").is_some());
    assert_eq!(
        current.graphs.get(&graph_id).unwrap().name,
        "shared"
    );
    assert!(!library.update_ml_model_paths(&defaults));

    let paths = MlModelPaths {
        denoise: PathBuf::from("/models/denoise.onnx"),
        star_removal: PathBuf::from("/models/stars.onnx"),
    };
    assert!(library.update_ml_model_paths(&paths));
    let published = library.published.load();
    assert_eq!(
        published.by_name("ML Denoise").unwrap().inputs[1].default_value,
        Some(StaticValue::FsPath(paths.denoise.display().to_string()))
    );
    assert_eq!(
        library
            .published
            .load()
            .by_name("ML Star Removal")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(
            paths.star_removal.display().to_string()
        ))
    );
    assert_eq!(published.graphs.get(&graph_id).unwrap().name, "shared");

    let second_id = GraphId::unique();
    let outcome = library.edit_graph_library_with(
        |graphs| {
            graphs.graphs.insert(second_id, Graph::new("second"));
            true
        },
        |_| Ok(()),
    );
    assert!(outcome.changed);
    assert!(outcome.persist_error.is_none());
    assert_eq!(
        library.published.load().graphs.get(&second_id).unwrap().name,
        "second"
    );
    assert_eq!(
        library
            .published
            .load()
            .by_name("ML Denoise")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(paths.denoise.display().to_string())),
        "graph synchronization retains the configured ML defaults"
    );
    assert!(library.published.load().graphs.contains_key(&second_id));
}

#[test]
fn graph_library_edit_distinguishes_noop_and_failed_persistence() {
    let mut library = RuntimeLibrary::new(&MlModelPaths::default());
    let noop = library.edit_graph_library_with(
        |_| false,
        |_| panic!("a no-op graph-library edit must not persist"),
    );
    assert!(!noop.changed);
    assert!(noop.persist_error.is_none());

    let graph_id = GraphId::unique();
    let failed = library.edit_graph_library_with(
        |graphs| {
            graphs.graphs.insert(graph_id, Graph::new("memory only"));
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
        library.published.load().graphs.get(&graph_id).unwrap().name,
        "memory only"
    );
    assert_eq!(
        library.published.load().graphs.get(&graph_id).unwrap().name,
        "memory only",
        "a failed save does not roll back the active runtime"
    );
}
