use glam::Vec2;
use scenarium::FuncId;
use scenarium::StaticValue;
use scenarium::{Binding, InputPort, Node, NodeId, NodeKind, NodeSearch};

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::types::Intent;
use crate::core::terminal_session::apply_intents;

use crate::core::document::ItemRef;

fn empty_document() -> Document {
    OpenDocument::default().document
}

#[test]
fn apply_intents_adds_node() {
    let mut doc = empty_document();
    assert_eq!(doc.graph.len(), 0);

    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = NodeId::unique();
    let intent = Intent::AddNode {
        pos: Vec2::new(10.0, 20.0),
        node_id: id,
        node,
        graph: None,
        bindings: vec![],
    };

    apply_intents(&mut doc, vec![intent]);
    assert_eq!(doc.graph.len(), 1);
    assert!(
        doc.graph.find(&id, NodeSearch::TopLevel).is_some(),
        "node landed in the graph"
    );
}

#[test]
fn apply_add_node_seeds_initial_bindings() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = NodeId::unique();
    let port = InputPort::new(id, 0);
    let intent = Intent::AddNode {
        pos: Vec2::ZERO,
        node_id: id,
        node,
        graph: None,
        bindings: vec![(port, Binding::Const(StaticValue::Float(5.0)))],
    };

    apply_intents(&mut doc, vec![intent]);
    assert_eq!(
        doc.graph.bindings.get(&port),
        Some(&Binding::Const(StaticValue::Float(5.0))),
        "the seeded default landed as a const binding",
    );
}

#[test]
fn apply_intents_drops_stale_intent() {
    let mut doc = empty_document();
    // RemoveNode targeting a node that isn't in the graph: `build_step`
    // returns None, so it's dropped without touching the document.
    apply_intents(
        &mut doc,
        vec![Intent::RemoveNode {
            node_id: NodeId::unique(),
        }],
    );
    assert_eq!(doc.graph.len(), 0);
}

#[test]
fn apply_intents_selects_existing_node() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = doc.graph.add(node);
    doc.main_view
        .item_placements
        .insert(ItemRef::Node(id), Vec2::ZERO);

    apply_intents(
        &mut doc,
        vec![Intent::SetSelection {
            to: [ItemRef::Node(id)].into_iter().collect(),
        }],
    );
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
                node_id: NodeId::unique(),
                node,
                graph: None,
                bindings: vec![],
            }
        })
        .collect();

    apply_intents(&mut doc, intents);
    assert_eq!(doc.graph.len(), 3, "all three nodes applied in one batch");
}
