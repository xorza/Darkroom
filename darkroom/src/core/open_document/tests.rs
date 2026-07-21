use scenarium::Graph;
use scenarium::{Binding, Func, FuncId, FuncInput, InputPort, Library, StaticValue};

use crate::core::document::Document;
use crate::core::open_document::OpenDocument;

#[test]
fn normalization_prunes_stale_wiring_once_and_rearms_after_replace() {
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
