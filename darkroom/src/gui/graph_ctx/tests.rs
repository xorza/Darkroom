use scenarium::internals::test_graph::TestGraph;
use scenarium::{
    CacheMode, DataType, EventPort, FuncId, Graph, InstancePath, Node, ProgramId, internals,
};
use scenarium::{internals::test_graph::NodeSpec, numbered_port};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::tab_ref::TabRef;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::port_ref::PortKind;

/// Composing a context does not ask whether anyone is looking: the document, the library and the
/// run resolve either way. An empty graph has no nodes on a Graph tab, as a fresh document's canvas
/// has, and a populated one reads back with every pane showing something else.
#[test]
fn a_context_reads_the_graph_whichever_tab_shows() {
    let empty_but_shown = GraphCtxFixture::over(DocumentFixture::default());
    assert_eq!(empty_but_shown.graph_ctx().nodes().count(), 0);

    let populated_but_hidden =
        GraphCtxFixture::over(DocumentFixture::probes(2).with_tab(TabRef::Preferences));
    assert_eq!(populated_but_hidden.graph_ctx().nodes().count(), 2);
}

#[test]
fn only_runnable_sinks_expose_the_disable_toggle() {
    // `can_disable` reads only sink-ness: a node whose func the library never held (the
    // third) declares no sink.
    let mut g = TestGraph::new();
    let plain = g.add("plain", |n| n.output(DataType::Int));
    let sink = g.add("sink_func", NodeSpec::sink);
    let mut fixture = DocumentFixture::from(g);
    let ghost = fixture.add_stub(internals::unresolved_node(FuncId::unique()));
    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();

    assert!(
        !graph_ctx.node(plain).unwrap().can_disable(),
        "a non-sink has no disable toggle"
    );
    assert!(
        graph_ctx.node(sink).unwrap().can_disable(),
        "a runnable sink can be disabled"
    );
    assert!(
        !graph_ctx.node(ghost).unwrap().can_disable(),
        "an unresolved node cannot be disabled because it cannot be run explicitly"
    );
}

#[test]
fn a_missing_func_reads_as_a_deletable_stub() {
    use scenarium::math_library;

    // A resolvable func, and one whose id the library no longer holds.
    let library = math_library();
    let mut graph = Graph::default();
    let mut known: Node = library.by_name("Add").unwrap().into();
    known.disabled = true;
    let mut ghost =
        internals::unresolved_node(FuncId::literal("7a0265e1-9631-45bd-8ecd-1e923b67a58c"));
    ghost.name = "astro_to_image".into();
    let known_id = graph.add(known);
    let mut fixture = DocumentFixture::with_library(graph, library);
    let ghost_id = fixture.add_stub(ghost);

    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();

    // An unresolvable node is not dropped: it stays selectable and deletable.
    assert_eq!(graph_ctx.nodes().count(), 2, "every placed node resolves");
    let known_node = graph_ctx.node(known_id).unwrap();
    let ghost_node = graph_ctx.node(ghost_id).unwrap();

    assert!(!known_node.stub(), "a resolved func is not a stub");
    assert!(ghost_node.stub());
    assert_eq!(ghost_node.kind_label(), "missing func");

    assert_eq!(ghost_node.name(), "astro_to_image");
    assert_eq!(ghost_node.inputs().len(), 0);
    assert_eq!(ghost_node.outputs().len(), 0);
    assert_eq!(ghost_node.port_count(PortKind::Input), 0);

    assert_eq!(
        known_node.inputs().len(),
        2,
        "the resolved func still reports its interface"
    );

    // A targeted run overrides the disabled flag; a stub can't be run to.
    assert!(
        known_node.disabled() && known_node.runnable() == Some(ProgramId::from(known_node.node_id)),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        ghost_node.runnable().is_none(),
        "a stub offers no run affordance — it resolves to nothing"
    );
}

#[test]
fn func_events_read_in_order_alongside_outputs() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // The `frame event` func declares two events and two data outputs; each reads independently.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let node_id = graph.add(node);

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let n = fixture.graph_ctx().node(node_id).unwrap();

    let event_names: Vec<&str> = n.events().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(event_names, ["Always", "FPS"], "events read in order");
    assert_eq!(n.event_ports().count(), 2, "one ref per declared event");

    let output_names: Vec<&str> = n.outputs().map(OutputCtx::name).collect();
    assert_eq!(
        output_names,
        ["Delta", "Frame #"],
        "data outputs are unaffected by events"
    );
}

#[test]
fn subscriptions_read_from_the_graph() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // The second node subscribes to the first's "FPS" event, its event 2.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(EventPort::new(emitter_id, numbered_port(2)), subscriber_id);

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let subs: Vec<_> = fixture.graph_ctx().subscriptions().collect();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event, EventPort::new(emitter_id, numbered_port(2)));
    assert_eq!(subs[0].subscriber, subscriber_id);
}

#[test]
fn cache_mode_reads_verbatim_per_node() {
    use scenarium::math_library;

    let library = math_library();
    let mut graph = Graph::default();
    let mut ids = Vec::new();
    for mode in [
        CacheMode::None,
        CacheMode::Ram,
        CacheMode::Disk,
        CacheMode::Both,
    ] {
        let mut node: Node = library.by_name("Add").unwrap().into();
        *node.cache_mut().unwrap() = mode;
        let node_id = graph.add(node);
        ids.push((node_id, mode));
    }

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    for (id, mode) in ids {
        let node = graph_ctx.node(id).unwrap();
        assert_eq!(node.cache(), Some(mode), "{mode:?} reads verbatim");
        assert!(node.cacheable());
    }
}

#[test]
fn impure_flag_reads_from_func_behavior() {
    // A `Pure` func offers the storage toggles, an `Impure` one has no content digest, and an
    // outputless sink has nothing to store.
    let mut g = TestGraph::new();
    let pure_id = g.add("pure_src", |n| n.pure().output(DataType::Int));
    let impure_id = g.add("impure_src", |n| n.output(DataType::Int));
    let outputless_id = g.add("outputless", |n| n.sink().input(DataType::Int));

    let fixture = GraphCtxFixture::over(g);
    let graph_ctx = fixture.graph_ctx();

    let pure = graph_ctx.node(pure_id).unwrap();
    let impure = graph_ctx.node(impure_id).unwrap();
    let outputless = graph_ctx.node(outputless_id).unwrap();

    assert!(!pure.impure(), "a Pure func keeps its cache chips");
    assert!(impure.impure(), "an Impure func hides its cache chips");
    assert!(
        pure.cacheable(),
        "a Pure func with an output can be cached and evicted"
    );
    assert!(
        !impure.cacheable(),
        "an Impure func has no digest to key a cache on"
    );
    assert!(
        !outputless.cacheable(),
        "an outputless func has nothing to store or evict"
    );
    assert!(!pure.sink() && !impure.sink());
}

/// A pane reads each run result through the instance it shows: a node of a subgraph reads its
/// program node under the side's `through`, and none when it shows none, when it can seed no run
/// either; an instance in the main graph reads what its inside came to and seeds no run, as it
/// has no body, and names the node inside that failed. Two instances, `i1` where the node ran in
/// 1.0 s and `i2` where it failed.
#[test]
fn run_results_read_through_the_instance_a_pane_shows() {
    use std::io;
    use std::sync::Arc;

    use palantir::Display;
    use scenarium::{
        GraphId, Interface, InvokeError, NodeExecStatus, NodeStatus, RamUsage, RunError,
        RunSummary, Subgraph, SubgraphId,
    };

    use crate::core::document::internals::insert_subgraph;
    use crate::gui::state::run_state::ExecStatus;

    let mut document = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let mut subgraph = Subgraph::new("inner", Interface::default());
    let inside = subgraph.graph.add(document.plain_node());
    insert_subgraph(&mut document.document, id, subgraph);
    let [i1, i2] = [(); 2].map(|()| document.add_instance(id));
    let mut fixture = GraphCtxFixture::over(document);
    let row = |instance, status| NodeStatus {
        program_id: InstancePath::new(&[instance]).node(inside),
        status: Some(status),
        ram: RamUsage::default(),
    };
    fixture.run_state.complete(RunSummary {
        nodes: vec![
            row(i1, NodeExecStatus::Executed { elapsed_secs: 1.0 }),
            row(
                i2,
                NodeExecStatus::Errored {
                    error: RunError::Invoke(Arc::new(InvokeError::external(io::Error::other(
                        "boom",
                    )))),
                },
            ),
        ],
        ..RunSummary::default()
    });
    let graph = GraphId::Subgraph(id);
    let through = |fixture: &mut GraphCtxFixture, path: Option<InstancePath>| {
        fixture
            .open
            .document_mut()
            .sides
            .get_mut(&id)
            .unwrap()
            .through = path;
    };

    through(&mut fixture, Some(InstancePath::new(&[i1])));
    let node = fixture
        .graph_ctx_of(graph, Display::default())
        .node(inside)
        .unwrap();
    assert_eq!(node.exec_status(), &ExecStatus::Executed(1.0));
    let in_i1 = InstancePath::new(&[i1]).node(inside);
    assert_eq!(node.runnable(), Some(in_i1));

    through(&mut fixture, Some(InstancePath::new(&[i2])));
    let node = fixture
        .graph_ctx_of(graph, Display::default())
        .node(inside)
        .unwrap();
    assert!(matches!(node.exec_status(), ExecStatus::Errored(Some(_))));
    assert_ne!(
        node.runnable(),
        Some(in_i1),
        "each instance its own program node"
    );

    through(&mut fixture, None);
    let node = fixture
        .graph_ctx_of(graph, Display::default())
        .node(inside)
        .unwrap();
    assert_eq!(node.exec_status(), &ExecStatus::None);
    assert_eq!(node.runnable(), None);

    let main = fixture.graph_ctx();
    assert_eq!(
        main.node(i1).unwrap().exec_status(),
        &ExecStatus::Executed(1.0)
    );
    let failing = main.node(i2).unwrap();
    assert!(matches!(
        failing.exec_status(),
        ExecStatus::Errored(Some(_))
    ));
    let name = &fixture
        .open
        .document()
        .graphs
        .graph(graph)
        .find(inside)
        .unwrap()
        .name;
    assert_eq!(
        failing.failed_inside(),
        Some(name.as_str()),
        "the footer names it"
    );
    assert_eq!(main.node(i1).unwrap().failed_inside(), None);
    assert_eq!(
        main.node(i1).unwrap().runnable(),
        None,
        "an instance has no body to run"
    );
}
