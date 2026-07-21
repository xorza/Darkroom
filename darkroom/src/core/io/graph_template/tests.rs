use common::{SerdeFormat, deserialize};
use common::test_utils::test_output_path;
use scenarium::{Graph, GraphEvent, GraphId, NodeId};

use crate::core::io::graph_template::{GraphTemplateLoadError, load, save};

#[test]
fn graph_template_roundtrips_and_validates_input() {
    let path = test_output_path("darkroom_graph_template/graph.json");
    let graph = Graph::new("ok").category("test");
    save(&graph, &path).unwrap();
    assert_eq!(load(&path).unwrap(), graph);

    let extensionless = test_output_path("darkroom_graph_template/graph");
    save(&graph, &extensionless).unwrap();
    let bytes = std::fs::read(extensionless).unwrap();
    let decoded: Graph = deserialize(&bytes, SerdeFormat::Json).unwrap();
    assert_eq!(decoded, graph, "unknown save extensions default to JSON");

    let unsupported = test_output_path("darkroom_graph_template/graph.unsupported");
    std::fs::write(&unsupported, b"{}").unwrap();
    assert!(matches!(
        load(&unsupported),
        Err(GraphTemplateLoadError::UnsupportedFormat { path }) if path == unsupported
    ));

    let bad_path = test_output_path("darkroom_graph_template/bad-graph.json");
    let mut bad = Graph::new("bad");
    bad.events.push(GraphEvent {
        name: "tick".into(),
        emitter: NodeId::unique(),
        emitter_event_idx: 0,
    });
    save(&bad, &bad_path).unwrap();
    let error = load(&bad_path).unwrap_err();
    assert!(
        matches!(
            &error,
            GraphTemplateLoadError::Invalid { path, reason }
                if path == &bad_path && reason.contains("names missing emitter")
        ),
        "invalid graph is rejected with its exact path and reason: {error}"
    );

    let nil_origin_path = test_output_path("darkroom_graph_template/nil-origin-graph.json");
    let mut nil_origin = Graph::new("nil origin");
    nil_origin.origin = Some(GraphId::nil());
    save(&nil_origin, &nil_origin_path).unwrap();
    let error = load(&nil_origin_path).unwrap_err();
    assert!(
        matches!(
            &error,
            GraphTemplateLoadError::Invalid { path, reason }
                if path == &nil_origin_path && reason.contains("graph has a nil origin")
        ),
        "nil lineage is rejected with its exact path and reason: {error}"
    );
}
