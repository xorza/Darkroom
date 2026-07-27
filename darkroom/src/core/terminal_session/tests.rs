use glam::Vec2;
use scenarium::FuncId;
use scenarium::StaticValue;
use scenarium::{Binding, InputPort, Node, NodeId, NodeKind, NodeSearch};

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::types::Intent;
use crate::core::terminal_session::apply_intents;

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
        bindings: vec![],
    };

    apply_intents(&mut doc, vec![intent]);
    assert_eq!(doc.graph.len(), 1);
    assert!(
        doc.graph.find(id, NodeSearch::TopLevel).is_some(),
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
fn apply_intents_drops_stale_intents_silently_but_reports_malformed_ones() {
    let mut doc = empty_document();
    // RemoveNode targeting a node that isn't in the graph: ordinary
    // staleness, so it's dropped without touching the document *and*
    // without a word — the same drop widgets rely on every frame.
    let reported = apply_intents(
        &mut doc,
        vec![Intent::RemoveNode {
            node_id: NodeId::unique(),
        }],
    );
    assert_eq!(doc.graph.len(), 0);
    assert!(reported.is_empty(), "a stale intent refuses quietly");

    // A payload that could never have applied answers back instead: a
    // script is the only thing that can build one, and it needs to learn
    // its request was refused rather than watch it vanish.
    let live = doc.graph.add(Node::new(NodeKind::Func(FuncId::unique())));
    doc.main_view.item_placements.insert(live, Vec2::ZERO);
    let reported = apply_intents(
        &mut doc,
        vec![Intent::AddNode {
            pos: Vec2::ZERO,
            node_id: live,
            node: Node::new(NodeKind::Func(FuncId::unique())),
            bindings: vec![],
        }],
    );
    assert_eq!(doc.graph.len(), 1, "the collision never reached the graph");
    assert_eq!(reported.len(), 1, "the caller is told once");
    assert!(
        reported[0].contains("already exists"),
        "the reason names the collision: {}",
        reported[0]
    );
    doc.validate().expect("document survives a refused batch");
}

#[test]
fn apply_intents_selects_existing_node() {
    let mut doc = empty_document();
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = doc.graph.add(node);
    doc.main_view.item_placements.insert(id, Vec2::ZERO);

    apply_intents(
        &mut doc,
        vec![Intent::SetSelection {
            to: [id].into_iter().collect(),
        }],
    );
    assert!(doc.main_view.selected.contains(&id));
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
                bindings: vec![],
            }
        })
        .collect();

    apply_intents(&mut doc, intents);
    assert_eq!(doc.graph.len(), 3, "all three nodes applied in one batch");
}
