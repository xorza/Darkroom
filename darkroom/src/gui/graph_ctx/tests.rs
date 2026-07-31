use scenarium::testing;
use scenarium::testing::graph::TestGraph;
use scenarium::{
    Binding, CacheMode, DataType, FuncOutput, Graph, InputPort, Library, Node, NodeKind,
    OutputTypes,
};

use crate::core::document::harness::wildcard_library;
use crate::core::document::{Document, GraphView, PortKind};
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph::harness::*;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

#[test]
fn only_runnable_sinks_expose_the_disable_toggle() {
    use scenarium::{Func, FuncId};

    // The two axes `can_disable` reads: sink or not, and resolvable or not.
    // The third node names a func the library has never held.
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "plain").output(FuncOutput::new("out", DataType::Int)),
    ));
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "sink_func").sink(),
    ));

    let mut g = TestGraph::over(library);
    let plain = g.add_declared("plain");
    let sink = g.add_declared("sink_func");
    let ghost = g.graph.add(Node::new(NodeKind::Func(FuncId::unique())));
    let mut fixture = GraphCtxFixture::over(Document::from(g.graph), g.library);
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

    let mut fixture = GraphCtxFixture::over(Document::from(graph), library);
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

    let mut fixture = GraphCtxFixture::over(Document::from(graph), library);
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

    let mut fixture = GraphCtxFixture::over(Document::from(graph), library);
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

    let mut fixture = GraphCtxFixture::over(Document::from(graph), library);
    let graph_ctx = fixture.graph_ctx();

    for (id, mode) in ids {
        let node = graph_ctx.node(id).unwrap();
        assert_eq!(node.cache(), mode, "{mode:?} reads verbatim");
        assert!(node.cache_controls());
    }
}

#[test]
fn impure_flag_reads_from_func_behavior() {
    use scenarium::{Func, FuncId};

    // Three funcs differing only in the flags the header gate reads: a `Pure`
    // one (offers the storage toggles), an `Impure` one (no content digest,
    // so the toggles are hidden), and a self-caching one.
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new("bbebd119-82d8-45cc-a710-cdaa45426521", "pure_src")
            .pure()
            .output(FuncOutput::new("out", DataType::Int)),
    ));
    library.add(testing::with_stub_lambda(
        Func::new("9a97bb06-2c2e-443a-a836-6a11e29cbea7", "impure_src")
            .output(FuncOutput::new("out", DataType::Int)),
    ));
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "self_cached")
            .pure()
            .uncacheable()
            .output(FuncOutput::new("out", DataType::Int)),
    ));

    let mut g = TestGraph::over(library);
    let pure_id = g.add_declared("pure_src");
    let impure_id = g.add_declared("impure_src");
    let self_cached_id = g.add_declared("self_cached");

    let mut fixture = GraphCtxFixture::over(Document::from(g.graph), g.library);
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

#[test]
fn a_wildcard_output_resolves_through_the_wire_it_mirrors() {
    use scenarium::{Func, FuncId, FuncInput, OutputType};

    // A passthrough mirroring input 0, and an Int source. `Graph` and
    // `Library` aren't `Clone`, so each half builds its own — the wiring is
    // the only difference between them.
    fn fixture(wired: bool) -> (GraphCtxFixture, scenarium::NodeId) {
        let mut library = Library::default();
        let mut passthrough = Func::new(FuncId::unique(), "passthrough")
            .pure()
            .input(FuncInput::optional("in", DataType::Any));
        passthrough.outputs.push(FuncOutput {
            name: "out".to_owned(),
            description: None,
            ty: OutputType::Wildcard { mirrors: 0 },
        });
        library.add(testing::with_stub_lambda(passthrough));
        library.add(testing::with_stub_lambda(
            Func::new(FuncId::unique(), "int_src")
                .pure()
                .output(FuncOutput::new("out", DataType::Int)),
        ));

        let mut g = TestGraph::over(library);
        let consumer = g.add_declared("passthrough");
        g.add_declared("int_src");
        if wired {
            g.wire("int_src", 0, "passthrough", 0);
        }
        (
            GraphCtxFixture::over(Document::from(g.graph), g.library),
            consumer,
        )
    }

    // Unwired the passthrough's output is polymorphic; wired it reports what
    // reached it — the one reading that cannot come off the declaration
    // alone, and the reason the context carries a resolved table at all.
    let (mut unwired, consumer) = fixture(false);
    assert_eq!(
        unwired
            .graph_ctx()
            .node(consumer)
            .unwrap()
            .output(0)
            .unwrap()
            .ty(),
        DataType::Any,
        "an unwired passthrough has nothing to mirror"
    );

    let (mut wired, consumer) = fixture(true);
    assert_eq!(
        wired
            .graph_ctx()
            .node(consumer)
            .unwrap()
            .output(0)
            .unwrap()
            .ty(),
        DataType::Int,
        "the wildcard follows the wire to the Int source"
    );
}

/// A graph edit reaches the very next read, with nothing announced.
///
/// The canvas holds no derived state about the graph, so there is no
/// invalidation step between wiring a port and the canvas reporting its new
/// type — which is the whole reason the wildcard resolution is safe to do per
/// read. Wired through the same context the record passes build.
#[test]
fn a_wire_edit_reaches_the_next_read_with_nothing_announced() {
    let library = wildcard_library();
    let probe = library.by_name("probe").expect("just added").clone();
    let passthrough = library.by_name("passthrough").expect("just added").clone();

    let mut doc = Document::default();
    // `probe` declares a fixed `Int` output; the passthrough mirrors whatever
    // reaches its input, so unwired it resolves to `Any`.
    let producer = doc.graph.add_func_node(&probe);
    let consumer = doc.graph.add_func_node(&passthrough);
    doc.main_view = GraphView::for_graph(&doc.graph);

    let theme = Theme::default();
    let run_state = RunState::default();
    let resolved_output = |doc: &Document| {
        let mut types = OutputTypes::default();
        graph_ctx_for(app(&theme, &library, &run_state), doc, &mut types)
            .node(consumer)
            .expect("the passthrough resolves")
            .output(0)
            .expect("it declares one output")
            .ty()
    };

    assert_eq!(
        resolved_output(&doc),
        DataType::Any,
        "an unwired passthrough has nothing to mirror"
    );

    doc.graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    assert_eq!(
        resolved_output(&doc),
        DataType::Int,
        "the next read follows the new wire — nothing was invalidated in between"
    );
}
