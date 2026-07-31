use scenarium::testing::graph::TestGraph;
use scenarium::{Binding, CacheMode, DataType, Graph, InputPort, Node, NodeKind};

use crate::core::document::PortKind;
use crate::core::document::harness::DocFixture;
use crate::gui::graph_ctx::harness::GraphCtxFixture;

#[test]
fn only_runnable_sinks_expose_the_disable_toggle() {
    use scenarium::FuncId;

    // The two axes `can_disable` reads: sink or not, and resolvable or not.
    // The third node names a func the library has never held.
    let mut g = TestGraph::new();
    let plain = g.add("plain", |n| n.output(DataType::Int));
    let sink = g.add("sink_func", |n| n.sink());
    let ghost = g.graph.add(Node::new(NodeKind::Func(FuncId::unique())));
    let mut fixture = GraphCtxFixture::over(g);
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

    // A resolvable func, plus one whose id the library no longer holds — a
    // document saved against an older library.
    let library = math_library();
    let mut graph = Graph::default();
    let mut known: Node = library.by_name("Add").unwrap().into();
    known.disabled = true;
    let mut ghost = Node::new(NodeKind::Func(
        "7a0265e1-9631-45bd-8ecd-1e923b67a58c".into(),
    ));
    ghost.name = "astro_to_image".into();
    let known_id = graph.add(known);
    let ghost_id = graph.add(ghost);

    let mut fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    // Both nodes resolve, not silently dropped — so the unresolvable one
    // stays selectable and deletable to repair the document.
    assert_eq!(graph_ctx.nodes().count(), 2, "every placed node resolves");
    let known_node = graph_ctx.node(known_id).unwrap();
    let ghost_node = graph_ctx.node(ghost_id).unwrap();

    // The flag tracks resolution; the label names what's missing.
    assert!(!known_node.missing(), "a resolved func is not a stub");
    assert!(ghost_node.missing());
    assert_eq!(ghost_node.kind_label(), "missing func");

    // The stub keeps its saved name and carries no ports.
    assert_eq!(ghost_node.name(), "astro_to_image");
    assert_eq!(ghost_node.inputs().len(), 0);
    assert_eq!(ghost_node.outputs().len(), 0);
    assert_eq!(ghost_node.port_count(PortKind::Input), 0);

    // The resolved node, by contrast, exposes its real ports.
    assert!(
        known_node.inputs().len() > 0,
        "the resolved func still reports its interface"
    );

    // Run seeding follows resolution: the resolved func can be run to even
    // while disabled (a targeted run overrides the flag); the stub can't.
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

    // The `frame event` func declares two events ("Always", "FPS") and two
    // data outputs ("Delta", "Frame #"); the context must surface both
    // independently — events off the declaration, outputs unchanged.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let node_id = graph.add(node);

    let mut fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let n = fixture.graph_ctx().node(node_id).unwrap();

    let event_names: Vec<&str> = n.events().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(event_names, ["Always", "FPS"], "events read in order");
    assert_eq!(n.event_refs().count(), 2, "one ref per declared event");

    let output_names: Vec<&str> = n.outputs().map(|o| o.name()).collect();
    assert_eq!(
        output_names,
        ["Delta", "Frame #"],
        "data outputs are unaffected by events"
    );
}

#[test]
fn subscriptions_read_from_the_graph() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // Two frame-event nodes; subscribe the second to the first's "FPS"
    // event (event_idx 1). The context must surface that one edge.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(emitter_id, 1, subscriber_id);

    let mut fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let subs: Vec<_> = fixture.graph_ctx().subscriptions().collect();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].emitter, emitter_id);
    assert_eq!(subs[0].event_idx, 1);
    assert_eq!(subs[0].subscriber, subscriber_id);
}

#[test]
fn cache_mode_reads_verbatim_per_node() {
    use scenarium::math_library;

    // One `Add` node per cache mode; each node's `cache()` must mirror its
    // source node's mode exactly (the header reads the two bits off it).
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

    let mut fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    for (id, mode) in ids {
        let node = graph_ctx.node(id).unwrap();
        assert_eq!(node.cache(), mode, "{mode:?} reads verbatim");
        assert!(node.cache_controls());
    }
}

#[test]
fn impure_flag_reads_from_func_behavior() {
    // Three funcs differing only in the flags the header gate reads: a `Pure`
    // one (offers the storage toggles), an `Impure` one (no content digest,
    // so the toggles are hidden), and a self-caching one.
    let mut g = TestGraph::new();
    let pure_id = g.add("pure_src", |n| n.pure().output(DataType::Int));
    let impure_id = g.add("impure_src", |n| n.output(DataType::Int));
    let self_cached_id = g.add("self_cached", |n| {
        n.pure().uncacheable().output(DataType::Int)
    });

    let mut fixture = GraphCtxFixture::over(g);
    let graph_ctx = fixture.graph_ctx();

    let pure = graph_ctx.node(pure_id).unwrap();
    let impure = graph_ctx.node(impure_id).unwrap();
    let self_cached = graph_ctx.node(self_cached_id).unwrap();

    assert!(!pure.impure(), "a Pure func keeps its cache chips");
    assert!(impure.impure(), "an Impure func hides its cache chips");
    // Both have an output, so `impure` is the sole eviction differentiator.
    assert!(
        pure.can_evict_cache(),
        "a Pure func with an output can be evicted"
    );
    assert!(
        !impure.can_evict_cache(),
        "an Impure func cannot be evicted"
    );
    assert!(pure.cache_controls());
    assert!(!impure.cache_controls());
    assert!(
        self_cached.can_evict_cache(),
        "self-caching funcs can still have cached downstream consumers"
    );
    assert!(
        !self_cached.cache_controls(),
        "self-caching funcs hide Scenarium storage controls"
    );
    assert!(!pure.sink() && !impure.sink());
}

/// A wildcard output reports what the *graph* wired into the input it
/// mirrors — the one reading that cannot come off the declaration alone, and
/// the reason the context carries a resolved table at all.
///
/// And the edit reaches the very next read with nothing announced: the canvas
/// holds no derived state about the graph, so there is no invalidation step
/// between wiring a port and the canvas reporting its new type — which is the
/// whole reason resolving per read is safe. Wired through the same context the
/// record passes build.
#[test]
fn a_wildcard_output_follows_the_wire_it_mirrors_from_the_next_read_on() {
    // `probe` declares a fixed `Int` output; the passthrough mirrors whatever
    // reaches its input, so unwired it resolves to `Any`.
    let mut g = TestGraph::new();
    let producer = g.add("probe", |n| n.pure().output(DataType::Int));
    let consumer = g.add("passthrough", |n| {
        n.pure().optional(DataType::Any).wildcard(0)
    });
    let mut fixture = GraphCtxFixture::over(g);

    // A fresh context per read, resolving its own output-type table — the
    // same thing each record pass composes.
    let resolved_output = |fixture: &mut GraphCtxFixture| {
        fixture
            .graph_ctx()
            .node(consumer)
            .expect("the passthrough resolves")
            .output(0)
            .expect("it declares one output")
            .ty()
    };

    assert_eq!(
        resolved_output(&mut fixture),
        DataType::Any,
        "an unwired passthrough has nothing to mirror"
    );

    fixture
        .fixture
        .doc
        .graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    assert_eq!(
        resolved_output(&mut fixture),
        DataType::Int,
        "the next read follows the new wire — nothing was invalidated in between"
    );
}
