use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{GraphId, Interface, Library, NodeId, Subgraph, SubgraphId};

use crate::core::clip::{Clip, HEADER};
use crate::core::document::Document;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::document::item_id::ItemId;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::open_document::OpenDocument;

/// A document holding subgraph `id`, one node inside it, and one instance of it in the main
/// graph; the instance's id.
fn holder(fixture: &mut DocumentFixture, id: SubgraphId) -> NodeId {
    let mut subgraph = Subgraph::new("Denoise", Interface::default());
    subgraph.graph.add(fixture.plain_node());
    insert_subgraph(&mut fixture.document, id, subgraph);
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

/// The instances of the main graph, by the subgraph each places.
fn placed(document: &Document) -> Vec<SubgraphId> {
    let mut placed: Vec<SubgraphId> = document
        .graphs
        .main()
        .iter()
        .filter_map(|node| node.subgraph())
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

/// A copied instance pastes into another document with its subgraph and that subgraph's view; a
/// second paste reuses the definition it brought, as the digests match; once that document
/// changes the definition, a paste brings the copied one under a fresh id.
#[test]
fn a_paste_brings_its_subgraphs_by_digest() {
    let id = SubgraphId::from_u128(1);
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
        document.graphs.subgraph(id),
        source.document.graphs.subgraph(id)
    );
    assert_eq!(
        document.view(GraphId::Subgraph(id)).node_placements,
        source.document.view(GraphId::Subgraph(id)).node_placements
    );
    assert_eq!(placed(document), [id]);

    paste(&mut open, &library, &text);
    assert_eq!(
        open.document().graphs.subgraphs().len(),
        1,
        "an equal digest reuses it"
    );
    assert_eq!(placed(open.document()), [id, id]);

    let inside = open
        .document()
        .graphs
        .graph(GraphId::Subgraph(id))
        .iter()
        .find(|node| node.boundary().is_none())
        .unwrap()
        .node_id;
    let mut queue = DocumentQueue::default();
    queue.push_graph(
        GraphId::Subgraph(id),
        GraphIntent::RenameNode {
            node_id: inside,
            to: "changed here".to_string(),
        },
    );
    let _ = open.drain_requests(&mut queue, &library);
    paste(&mut open, &library, &text);
    let document = open.document();
    assert_eq!(
        document.graphs.subgraphs().len(),
        2,
        "a changed one keeps its id"
    );
    let fresh = document
        .graphs
        .subgraphs()
        .map(|(held, _)| held)
        .find(|&held| held != id)
        .unwrap();
    assert_eq!(placed(document).iter().filter(|&&p| p == fresh).count(), 1);
    document.validate().unwrap();
}

/// A clip from a document whose library this one lacks pastes nothing and says why: a node whose
/// func this library lacks is no edit the gate takes, and a subgraph holding one breaks the load
/// contract. The document is left as it was.
#[test]
fn a_clip_naming_a_func_this_library_lacks_pastes_nothing() {
    use crate::core::clip::error::PasteError;
    use crate::core::edit::error::{IntentError, MalformedIntent};

    let mut source = DocumentFixture::default();
    let node = source.node_at(Vec2::ZERO);
    let instance = holder(&mut source, SubgraphId::from_u128(1));
    let copy = |item| {
        Clip::copy(
            &source.document,
            GraphId::Main,
            &source.library,
            &BTreeSet::from([ItemId::Node(item)]),
        )
        .expect("the selection holds a node")
    };
    let (lacking, target) = (Library::default(), Document::default());

    let refused = copy(node)
        .paste(&target, GraphId::Main, &lacking, Vec2::ZERO)
        .unwrap_err();
    assert!(
        matches!(
            refused,
            PasteError::Edit(IntentError::Malformed(
                MalformedIntent::UnresolvedNode { .. }
            ))
        ),
        "{refused:?}"
    );
    let refused = copy(instance)
        .paste(&target, GraphId::Main, &lacking, Vec2::ZERO)
        .unwrap_err();
    assert!(matches!(refused, PasteError::Subgraph(_)), "{refused:?}");
    assert!(
        copy(instance)
            .paste(&target, GraphId::Main, &source.library, Vec2::ZERO)
            .is_ok(),
        "the same clip fits a document with the library"
    );
}
