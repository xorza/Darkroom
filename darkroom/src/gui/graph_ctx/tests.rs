use scenarium::internals::test_graph::NodeSpec;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{CacheMode, DataType, EventPort, FuncId, Graph, Node, internals};

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
    // And one authored against other ports of a func the library holds.
    let add_id = library.by_name("Add").unwrap().id();
    let known_id = graph.add(known);
    let mut fixture = DocumentFixture::with_library(graph, library);
    let ghost_id = fixture.add_stub(ghost);
    let changed_id = fixture.add_stub(internals::unresolved_node(add_id));

    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();

    // An unresolvable node is not dropped: it stays selectable and deletable.
    assert_eq!(graph_ctx.nodes().count(), 3, "every placed node resolves");
    let known_node = graph_ctx.node(known_id).unwrap();
    let ghost_node = graph_ctx.node(ghost_id).unwrap();

    assert!(!known_node.stub(), "a resolved func is not a stub");
    assert!(ghost_node.stub());
    assert_eq!(ghost_node.kind_label(), "missing func");
    let changed_node = graph_ctx.node(changed_id).unwrap();
    assert!(
        changed_node.stub(),
        "a func that moved its ports is a stub too"
    );
    assert_eq!(changed_node.kind_label(), "changed func");

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
        known_node.disabled() && known_node.runnable(),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        !ghost_node.runnable(),
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

    // The second node subscribes to the first's "FPS" event (event_idx 1).
    let library = worker_events_library();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(EventPort::new(emitter_id, 1), subscriber_id);

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let subs: Vec<_> = fixture.graph_ctx().subscriptions().collect();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event, EventPort::new(emitter_id, 1));
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
        node.cache = mode;
        let node_id = graph.add(node);
        ids.push((node_id, mode));
    }

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    for (id, mode) in ids {
        let node = graph_ctx.node(id).unwrap();
        assert_eq!(node.cache(), mode, "{mode:?} reads verbatim");
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
