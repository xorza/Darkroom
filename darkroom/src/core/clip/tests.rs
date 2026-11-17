use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{Compound, CompoundId, GraphId, Interface, Library, NodeId};

use crate::core::clip::{Clip, HEADER};
use crate::core::document::Document;
use crate::core::document::internals::{DocumentFixture, insert_compound};
use crate::core::document::item_id::ItemId;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::open_document::OpenDocument;

/// A document holding compound `id`, one node inside it, and one instance of it in the main
/// graph; the instance's id.
fn holder(fixture: &mut DocumentFixture, id: CompoundId) -> NodeId {
    let mut compound = Compound::new("Denoise", Interface::default());
    compound.graph.add(fixture.plain_node());
    insert_compound(&mut fixture.document, id, compound);
    fixture.add_instance(id)
}

/// Paste `text` into the main graph of `open`.
fn paste(open: &mut OpenDocument, library: &Library, text: &str) {
    let clip = Clip::parse(text).expect("the text is a clip");
    let mut queue = DocumentQueue::default();
    clip.paste(
        open.document(),
        GraphId::Main,
        library,
        Vec2::new(40.0, 40.0),
    )
    .expect("the clip fits the document")
    .queue(GraphId::Main, &mut queue);
    let _ = open.drain_requests(&mut queue, library);
}

/// The instances of the main graph, by the compound each places.
fn placed(document: &Document) -> Vec<CompoundId> {
    let mut placed: Vec<CompoundId> = document
        .graphs
        .main()
        .iter()
        .filter_map(|node| node.compound())
        .collect();
    placed.sort_unstable();
    placed
}

/// Text with no header, a header over no clip, and text past the bound paste nothing.
#[test]
fn text_that_is_no_clip_pastes_nothing() {
    assert!(Clip::parse("Denoise").is_none());
    assert!(Clip::parse(&format!("{HEADER}\nnot ron")).is_none());
    let long = format!("{HEADER}\n{}", " ".repeat(super::MAX_BYTES));
    assert!(Clip::parse(&long).is_none());
}

/// A copied instance pastes into another document with its compound and that compound's view; a
/// second paste reuses the definition it brought, as the digests match; once that document
/// changes the definition, a paste brings the copied one under a fresh id.
#[test]
fn a_paste_brings_its_compounds_by_digest() {
    let id = CompoundId::from_u128(1);
    let mut source = DocumentFixture::default();
    let instance = holder(&mut source, id);
    let text = Clip::copy(
        &source.document,
        GraphId::Main,
        &source.library,
        &BTreeSet::from([ItemId::Node(instance)]),
    )
    .expect("the selection holds a node")
    .to_text();
    assert!(text.starts_with(HEADER));

    // Another document, whose library declares the copied node's func.
    let mut other = DocumentFixture::default();
    let _ = other.plain_node();
    let DocumentFixture {
        document: target,
        library,
    } = other;
    let mut open = OpenDocument::over(target);
    paste(&mut open, &library, &text);
    let document = open.document();
    assert_eq!(
        document.graphs.compound(id),
        source.document.graphs.compound(id)
    );
    assert_eq!(
        document.view(GraphId::Compound(id)).node_placements,
        source.document.view(GraphId::Compound(id)).node_placements
    );
    assert_eq!(placed(document), [id]);

    paste(&mut open, &library, &text);
    assert_eq!(
        open.document().graphs.compounds().len(),
        1,
        "an equal digest reuses it"
    );
    assert_eq!(placed(open.document()), [id, id]);

    let inside = open
        .document()
        .graphs
        .graph(GraphId::Compound(id))
        .iter()
        .find(|node| node.boundary().is_none())
        .unwrap()
        .node_id;
    let mut queue = DocumentQueue::default();
    queue.push_graph(
        GraphId::Compound(id),
        GraphIntent::RenameNode {
            node_id: inside,
            to: "changed here".to_string(),
        },
    );
    let _ = open.drain_requests(&mut queue, &library);
    paste(&mut open, &library, &text);
    let document = open.document();
    assert_eq!(
        document.graphs.compounds().len(),
        2,
        "a changed one keeps its id"
    );
    let fresh = document
        .graphs
        .compounds()
        .map(|(held, _)| held)
        .find(|&held| held != id)
        .unwrap();
    assert_eq!(placed(document).iter().filter(|&&p| p == fresh).count(), 1);
    document.validate().unwrap();
}

/// A clip from a document whose library this one lacks: a node whose func this library lacks
/// is no edit the gate takes, so its paste lands nothing and says why; a compound holding one
/// comes in reconciled, the node kept inside it as a ghost node that holds what the clip's
/// manifest says it declared, as a load keeps it.
#[test]
fn a_clip_naming_a_func_this_library_lacks() {
    use scenarium::{DetachReason, Unresolved};

    use crate::core::edit::error::{IntentError, MalformedIntent};

    let mut source = DocumentFixture::default();
    let node = source.node_at(Vec2::ZERO);
    let id = CompoundId::from_u128(1);
    let instance = holder(&mut source, id);
    let inside = source
        .document
        .graphs
        .compound(id)
        .unwrap()
        .graph
        .iter()
        .find(|node| node.boundary().is_none())
        .unwrap()
        .node_id;
    let copy = |item| {
        Clip::copy(
            &source.document,
            GraphId::Main,
            &source.library,
            &BTreeSet::from([ItemId::Node(item)]),
        )
        .expect("the selection holds a node")
    };
    let lacking = Library::default();

    let refused = copy(node)
        .paste(&Document::default(), GraphId::Main, &lacking, Vec2::ZERO)
        .unwrap_err();
    assert!(
        matches!(
            refused.0,
            IntentError::Malformed(MalformedIntent::UnresolvedNode { .. })
        ),
        "{refused:?}"
    );

    let mut open = OpenDocument::over(Document::default());
    paste(&mut open, &lacking, &copy(instance).to_text());
    let document = open.document();
    assert_eq!(placed(document), [id]);
    let held = &document.graphs.compound(id).unwrap().graph;
    assert!(held.find(inside).is_none());
    let ghost = held.ghost_node(inside).expect("the node is kept");
    assert_eq!(
        ghost.reason,
        DetachReason::Unresolved(Unresolved::MissingFunc)
    );
    let saved = ghost
        .saved
        .as_ref()
        .expect("the clip's manifest says what it was");
    assert_eq!(
        (
            saved.name.as_str(),
            saved
                .inputs
                .iter()
                .map(|input| (input.id.get(), input.name.as_str()))
                .collect::<Vec<_>>(),
            saved.outputs.len(),
            saved.events.len(),
        ),
        ("plain", vec![(1, "a"), (2, "b"), (3, "c")], 1, 1)
    );
    document
        .validate()
        .expect("the paste leaves a valid document");
}
