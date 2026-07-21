use std::path::PathBuf;

use lens::MlModelPaths;
use scenarium::{Graph, GraphId, StaticValue};

use crate::core::graph_library::GraphLibrary;
use crate::core::runtime_library::RuntimeLibrary;

#[test]
fn runtime_library_recomposes_builtins_graphs_and_ml_defaults() {
    let graph_id = GraphId::unique();
    let mut graphs = GraphLibrary::default();
    graphs.graphs.insert(graph_id, Graph::new("shared"));
    let defaults = MlModelPaths::default();
    let mut library = RuntimeLibrary::new(&defaults, &graphs);

    assert!(library.current.by_name("Watch Directory").is_some());
    assert!(library.current.by_name("Random").is_some());
    assert_eq!(library.current.graphs.get(&graph_id).unwrap().name, "shared");
    assert!(!library.update_ml_model_paths(&defaults, &graphs));

    let paths = MlModelPaths {
        denoise: PathBuf::from("/models/denoise.onnx"),
        star_removal: PathBuf::from("/models/stars.onnx"),
    };
    assert!(library.update_ml_model_paths(&paths, &graphs));
    let published = library.published.load();
    assert_eq!(
        published.by_name("ML Denoise").unwrap().inputs[1].default_value,
        Some(StaticValue::FsPath(paths.denoise.display().to_string()))
    );
    assert_eq!(
        library.current.by_name("ML Star Removal").unwrap().inputs[1].default_value,
        Some(StaticValue::FsPath(
            paths.star_removal.display().to_string()
        ))
    );
    assert_eq!(published.graphs.get(&graph_id).unwrap().name, "shared");

    let second_id = GraphId::unique();
    graphs.graphs.insert(second_id, Graph::new("second"));
    library.sync_graph_library(&graphs);
    assert_eq!(library.current.graphs.get(&second_id).unwrap().name, "second");
    assert_eq!(
        library.current.by_name("ML Denoise").unwrap().inputs[1].default_value,
        Some(StaticValue::FsPath(paths.denoise.display().to_string())),
        "graph synchronization retains the configured ML defaults"
    );
    assert!(library.published.load().graphs.contains_key(&second_id));
}
