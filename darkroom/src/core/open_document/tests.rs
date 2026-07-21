use std::path::PathBuf;

use scenarium::Graph;
use scenarium::{Binding, Func, FuncId, FuncInput, InputPort, Library, StaticValue};

use crate::core::document::Document;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::OpenDocument;

#[test]
fn disabled_reopen_ignores_the_remembered_path() {
    let preferences = Preferences {
        document_path: Some(PathBuf::from("does-not-exist.darkroom")),
        load_last_document: false,
        ..Preferences::default()
    };

    let open = OpenDocument::load(&preferences).unwrap();

    assert!(open.path.is_none());
    assert!(open.document.graph.is_empty());
}

#[test]
fn failed_reopen_returns_the_load_error() {
    let path = PathBuf::from("does-not-exist.darkroom");
    let preferences = Preferences {
        document_path: Some(path.clone()),
        ..Preferences::default()
    };

    let error = OpenDocument::load(&preferences).unwrap_err();

    assert!(error.to_string().contains(path.to_str().unwrap()));
}

#[test]
fn normalization_prunes_stale_wiring_once_for_each_open_document() {
    let func_id = FuncId::unique();
    let previous = Func::new(func_id, "changed")
        .input(FuncInput::required("removed", scenarium::DataType::Float));
    let library = Library::from([Func::new(func_id, "changed")]);
    let document = || {
        let mut graph = Graph::default();
        let node_id = graph.add_func_node(&previous);
        let input = InputPort::new(node_id, 0);
        graph.set_input_binding(input, Binding::Const(StaticValue::Float(3.0)));
        Document::from(graph)
    };
    let mut open = OpenDocument::new(document(), None);

    assert!(open.normalization_pending);
    assert_eq!(open.document.graph.bindings.len(), 1);
    open.normalize(&library);
    assert!(!open.normalization_pending);
    assert!(open.document.graph.bindings.is_empty());

    open = OpenDocument::new(document(), None);
    assert!(open.normalization_pending);
    assert_eq!(open.document.graph.bindings.len(), 1);
    open.normalize(&library);
    assert!(!open.normalization_pending);
    assert!(open.document.graph.bindings.is_empty());
}

#[test]
fn empty_document_has_the_main_graph_tab() {
    let open = OpenDocument::empty();

    assert!(open.path.is_none());
    assert_eq!(open.document.layout.all_tabs().count(), 1);
    assert!(open.normalization_pending);
}
