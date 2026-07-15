use glam::Vec2;
use scenarium::FuncId;
use scenarium::StaticValue;
use scenarium::{Binding, InputPort, Node, NodeId, NodeKind, NodeSearch};

use super::*;
use crate::core::document::ItemRef;
use crate::core::document::view_item::ViewItem;

#[test]
fn apply_intents_adds_node_and_flags_reconcile() {
    let mut doc = empty_document();
    assert_eq!(doc.graph.len(), 0);

    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    let intent = Intent::AddNode {
        pos: Vec2::new(10.0, 20.0),
        node,
        def: None,
        bindings: vec![],
    };

    let reconcile = apply_intents(&mut doc, vec![intent], &Library::default());
    assert_eq!(doc.graph.len(), 1);
    assert!(
        doc.graph.find_node(&id, NodeSearch::TopLevel).is_some(),
        "node landed in the graph"
    );
    assert!(reconcile, "AddNode can change the interface → reconcile");
}

#[test]
fn apply_add_node_seeds_initial_bindings() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    let port = InputPort::new(id, 0);
    let intent = Intent::AddNode {
        pos: Vec2::ZERO,
        node,
        def: None,
        bindings: vec![(port, Binding::Const(StaticValue::Float(5.0)))],
    };

    apply_intents(&mut doc, vec![intent], &Library::default());
    assert_eq!(
        doc.graph.input_binding(port),
        Binding::Const(StaticValue::Float(5.0)),
        "the seeded default landed as a const binding",
    );
}

#[test]
fn apply_intents_drops_stale_intent() {
    let mut doc = empty_document();
    // RemoveNode targeting a node that isn't in the graph: `build_step`
    // returns None, so it's dropped without touching the document.
    let reconcile = apply_intents(
        &mut doc,
        vec![Intent::RemoveNode {
            node_id: NodeId::unique(),
        }],
        &Library::default(),
    );
    assert!(!reconcile);
    assert_eq!(doc.graph.len(), 0);
}

#[test]
fn apply_intents_selection_skips_reconcile() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    doc.graph.add(node);
    doc.main_view.view_items.add(ViewItem::node(id, Vec2::ZERO));

    // Selecting an existing node is a real change but a pure view edit —
    // no interface impact, so it must not request a reconcile.
    let reconcile = apply_intents(
        &mut doc,
        vec![Intent::SetSelection {
            to: [ItemRef::Node(id)].into_iter().collect(),
        }],
        &Library::default(),
    );
    assert!(!reconcile);
    assert!(doc.main_view.selected.contains(&ItemRef::Node(id)));
}

#[test]
fn apply_intents_batches_multiple() {
    let mut doc = empty_document();
    let intents: Vec<Intent> = (0..3)
        .map(|i| {
            let node = Node::new(NodeKind::Func(FuncId::unique()));
            Intent::AddNode {
                pos: Vec2::new(i as f32 * 100.0, 0.0),
                node,
                def: None,
                bindings: vec![],
            }
        })
        .collect();

    let reconcile = apply_intents(&mut doc, intents, &Library::default());
    assert_eq!(doc.graph.len(), 3, "all three nodes applied in one batch");
    assert!(reconcile);
}

#[test]
fn apply_intents_severs_incompatible_passthrough_output_edges() {
    use scenarium::DataType;
    use scenarium::Library;
    use scenarium::{Func, FuncInput, FuncOutput};

    // Float producer → wildcard passthrough → Float sink, all headless.
    let float_src =
        Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float));
    let string_src =
        Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String));
    let float_sink = Func::new(FuncId::unique(), "fsink")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("o", DataType::Float));
    let pass_func = Func::new(FuncId::unique(), "pass")
        .input(FuncInput::required("x", DataType::Any))
        .wildcard_output("o", 0);
    let library = Library::from([
        float_src.clone(),
        string_src.clone(),
        float_sink.clone(),
        pass_func.clone(),
    ]);

    let mut doc = empty_document();
    let fp = doc.graph.add_func_node(&float_src);
    let sp = doc.graph.add_func_node(&string_src);
    let pass = doc.graph.add_func_node(&pass_func);
    let sink = doc.graph.add_func_node(&float_sink);
    doc.graph
        .set_input_binding(InputPort::new(pass, 0), Binding::bind(fp, 0));
    doc.graph
        .set_input_binding(InputPort::new(sink, 0), Binding::bind(pass, 0));

    // A script rewires the passthrough's input to the String producer: the
    // output type becomes String, so the cascade drops the Float sink edge.
    apply_intents(
        &mut doc,
        vec![Intent::SetInput {
            input: InputPort::new(pass, 0),
            to: Binding::bind(sp, 0),
        }],
        &library,
    );

    assert_eq!(
        doc.graph.input_binding(InputPort::new(pass, 0)),
        Binding::bind(sp, 0),
        "the rewire landed"
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(sink, 0)),
        Binding::None,
        "the now-incompatible Float sink edge was severed in the same batch"
    );
}

#[test]
fn apply_intents_severs_through_a_passthrough_chain() {
    use scenarium::DataType;
    use scenarium::Library;
    use scenarium::{Func, FuncInput, FuncOutput};

    // Float producer → pass1 → pass2 → Float sink: a valid two-passthrough chain.
    let float_src =
        Func::new(FuncId::unique(), "fsrc").output(FuncOutput::new("o", DataType::Float));
    let string_src =
        Func::new(FuncId::unique(), "ssrc").output(FuncOutput::new("o", DataType::String));
    let float_sink = Func::new(FuncId::unique(), "fsink")
        .input(FuncInput::required("x", DataType::Float))
        .output(FuncOutput::new("o", DataType::Float));
    let pass_func = Func::new(FuncId::unique(), "pass")
        .input(FuncInput::required("x", DataType::Any))
        .wildcard_output("o", 0);
    let library = Library::from([
        float_src.clone(),
        string_src.clone(),
        float_sink.clone(),
        pass_func.clone(),
    ]);

    let add_pass = |doc: &mut Document| doc.graph.add_func_node(&pass_func);

    let mut doc = empty_document();
    let fp = doc.graph.add_func_node(&float_src);
    let sp = doc.graph.add_func_node(&string_src);
    let p1 = add_pass(&mut doc);
    let p2 = add_pass(&mut doc);
    let sink = doc.graph.add_func_node(&float_sink);
    doc.graph
        .set_input_binding(InputPort::new(p1, 0), Binding::bind(fp, 0));
    doc.graph
        .set_input_binding(InputPort::new(p2, 0), Binding::bind(p1, 0));
    doc.graph
        .set_input_binding(InputPort::new(sink, 0), Binding::bind(p2, 0));

    // Rewire pass1 to the String producer: both passthrough outputs retype to
    // String, so the edge *two hops down* (pass2 → sink) is the invalid one.
    apply_intents(
        &mut doc,
        vec![Intent::SetInput {
            input: InputPort::new(p1, 0),
            to: Binding::bind(sp, 0),
        }],
        &library,
    );

    assert_eq!(
        doc.graph.input_binding(InputPort::new(p2, 0)),
        Binding::bind(p1, 0),
        "pass2's wildcard input accepts the new type and is kept"
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(sink, 0)),
        Binding::None,
        "the cascade follows the chain and severs the two-hops-down sink edge"
    );
}
