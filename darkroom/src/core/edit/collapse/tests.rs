use std::collections::BTreeSet;

use scenarium::{Binding, Compiler, GraphId, InputPort, Library, OutputPort, PortId};

use crate::core::document;
use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::edit::collapse::{CollapseRefusal, collapse, expand};
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::open_document::OpenDocument;

/// A chain `a → b → c` of probes, each named after its letter, with `c` also reading `a`.
fn chain() -> DocumentFixture {
    let mut fixture = DocumentFixture::probes(3);
    let ids: Vec<_> = (0..3).map(|i| fixture.node(i)).collect();
    for (id, name) in ids.iter().zip(["a", "b", "c"]) {
        fixture
            .document
            .graphs
            .main_mut()
            .find_mut(*id)
            .unwrap()
            .name = name.to_string();
    }
    for (from, to) in [(0, 1), (1, 2)] {
        fixture.wire(
            OutputPort::new(ids[from], PortId::new(1)),
            InputPort::new(ids[to], PortId::new(1)),
        );
    }
    fixture
}

/// The main graph's wiring by node title, resolved against `library`: each wire as
/// `source.port -> consumer.port`, sorted, for comparing graphs whose ids differ.
fn wiring(document: &Document, library: &Library) -> Vec<String> {
    let graph = document.graphs.main();
    let scope = document.graphs.scope(library, GraphId::Main);
    let name = |id| {
        graph
            .find(id)
            .map_or("?", |node| document::title(node, scope))
            .to_string()
    };
    let mut wires: Vec<String> = graph
        .bindings
        .iter()
        .map(|(input, binding)| match binding {
            Binding::Wire(source) => format!(
                "{}.{:?} -> {}.{:?}",
                name(source.node_id),
                source.port,
                name(input.node_id),
                input.port
            ),
            Binding::Const(value) => {
                format!("{value:?} -> {}.{:?}", name(input.node_id), input.port)
            }
        })
        .collect();
    wires.sort();
    wires
}

/// Collapsing `b` of the chain makes a compound of one input and one output, of the probe's
/// type, and an instance between `a` and `c`, all in one history entry; expanding that instance
/// puts `b` back between them under a fresh id, so the wiring reads as before and both graphs
/// compile.
#[test]
fn collapse_then_expand_gives_back_the_wiring_but_for_ids() {
    let fixture = chain();
    let b = fixture.node(1);
    let DocumentFixture { document, library } = fixture;
    let before = wiring(&document, &library);
    let mut open = OpenDocument::over(document);
    let mut queue = DocumentQueue::default();

    let selection = BTreeSet::from([ItemId::Node(b)]);
    let plan = collapse(open.document(), GraphId::Main, &library, &selection)
        .expect("the selection holds a node")
        .expect("the selection collapses");
    let id = plan.compound.id;
    plan.queue(GraphId::Main, &mut queue);
    let _ = open.drain_requests(&mut queue, &library);
    let document = open.document();
    let interface = &document.graphs.compound(id).unwrap().interface;
    assert_eq!(
        (interface.inputs().len(), interface.outputs().len()),
        (1, 1)
    );
    let instance = document
        .graphs
        .main()
        .iter()
        .find(|node| node.compound() == Some(id))
        .unwrap()
        .node_id;
    assert_eq!(
        wiring(document, &library),
        [
            format!("Compound.{:?} -> c.{:?}", PortId::new(1), PortId::new(1)),
            format!("a.{:?} -> Compound.{:?}", PortId::new(1), PortId::new(1)),
        ]
    );
    assert!(
        document
            .graphs
            .graph(GraphId::Compound(id))
            .find(b)
            .is_some(),
        "b keeps its id"
    );
    Compiler::default().compile(&document.graphs, &library);
    assert!(open.can_undo());
    let _ = open.undo();
    assert_eq!(
        wiring(open.document(), &library),
        before,
        "one entry takes the collapse back"
    );
    assert!(open.document().graphs.compound(id).is_none());
    let _ = open.redo();

    for intent in expand(open.document(), GraphId::Main, &library, instance) {
        queue.push_graph(GraphId::Main, intent);
    }
    let _ = open.drain_requests(&mut queue, &library);
    let document = open.document();
    assert_eq!(wiring(document, &library), before);
    assert!(
        document.graphs.main().find(b).is_none(),
        "the expanded b takes a fresh id"
    );
    assert!(
        document.graphs.compound(id).is_some(),
        "the definition stays, unused"
    );
    Compiler::default().compile(&document.graphs, &library);
}

/// Selecting `a` and `c` but not `b`, which reads `a` and feeds `c`, would make an instance that
/// feeds itself: the collapse is refused, naming `b`; an empty selection plans nothing.
#[test]
fn a_selection_that_would_feed_itself_is_refused_naming_the_node_between() {
    let fixture = chain();
    let (a, b, c) = (fixture.node(0), fixture.node(1), fixture.node(2));
    let refused = collapse(
        &fixture.document,
        GraphId::Main,
        &fixture.library,
        &BTreeSet::from([ItemId::Node(a), ItemId::Node(c)]),
    );
    assert_eq!(
        refused.unwrap().unwrap_err(),
        CollapseRefusal::FeedsItself {
            node: b,
            name: "b".to_string()
        }
    );
    assert!(
        collapse(
            &fixture.document,
            GraphId::Main,
            &fixture.library,
            &BTreeSet::new()
        )
        .is_none()
    );
}
