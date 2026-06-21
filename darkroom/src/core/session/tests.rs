use glam::Vec2;
use scenarium::data::StaticValue;
use scenarium::graph::{Binding, InputPort, Node, NodeId, NodeKind};
use scenarium::prelude::FuncId;

use super::*;
use crate::core::document::view_node::ViewNode;

#[test]
fn apply_intents_adds_node_and_flags_reconcile() {
    let mut doc = empty_document();
    assert_eq!(doc.graph.len(), 0);

    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    let intent = Intent::AddNode {
        view_node: ViewNode {
            id,
            pos: Vec2::new(10.0, 20.0),
        },
        node,
        def: None,
        bindings: vec![],
    };

    let reconcile = apply_intents(&mut doc, vec![intent]);
    assert_eq!(doc.graph.len(), 1);
    assert!(doc.graph.by_id(&id).is_some(), "node landed in the graph");
    assert!(reconcile, "AddNode can change the interface → reconcile");
}

#[test]
fn apply_add_node_seeds_initial_bindings() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    let port = InputPort::new(id, 0);
    let intent = Intent::AddNode {
        view_node: ViewNode {
            id,
            pos: Vec2::ZERO,
        },
        node,
        def: None,
        bindings: vec![(port, Binding::Const(StaticValue::Float(5.0)))],
    };

    apply_intents(&mut doc, vec![intent]);
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
    doc.main_view.view_nodes.add(ViewNode {
        id,
        pos: Vec2::ZERO,
    });

    // Selecting an existing node is a real change but a pure view edit —
    // no interface impact, so it must not request a reconcile.
    let reconcile = apply_intents(
        &mut doc,
        vec![Intent::SetSelection {
            to: [id].into_iter().collect(),
        }],
    );
    assert!(!reconcile);
    assert!(doc.main_view.selected_nodes.contains(&id));
}

#[test]
fn apply_intents_batches_multiple() {
    let mut doc = empty_document();
    let intents: Vec<Intent> = (0..3)
        .map(|i| {
            let node = Node::new(NodeKind::Func(FuncId::unique()));
            Intent::AddNode {
                view_node: ViewNode {
                    id: node.id,
                    pos: Vec2::new(i as f32 * 100.0, 0.0),
                },
                node,
                def: None,
                bindings: vec![],
            }
        })
        .collect();

    let reconcile = apply_intents(&mut doc, intents);
    assert_eq!(doc.graph.len(), 3, "all three nodes applied in one batch");
    assert!(reconcile);
}
