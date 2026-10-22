use scenarium::internals::test_graph::NodeSpec;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{
    Binding, CacheMode, ConstValue, DataType, EventPort, FuncId, Graph, InputPort, Node, internals,
};

use crate::core::document::internals::DocFixture;
use crate::core::document::tab_ref::TabRef;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::port_ref::PortKind;

/// Composing a context does not ask whether anyone is looking — the document,
/// the library and the run resolve either way — so visibility rides along as a
/// field for the one pass that runs before the tab set settles.
///
/// It tracks the *active tab*, never the graph's contents, and the two come
/// apart in both directions: an empty graph on a Graph tab is a real pane (a
/// fresh document needs a canvas to place its first node on), and a populated
/// graph still reads back with every pane showing something else.
#[test]
fn visibility_tracks_the_active_tab_not_the_graph_contents() {
    let empty_but_shown = GraphCtxFixture::over(DocFixture::default());
    assert!(empty_but_shown.graph_ctx().is_visible());
    assert_eq!(empty_but_shown.graph_ctx().nodes().count(), 0);

    let populated_but_hidden =
        GraphCtxFixture::over(DocFixture::probes(2).with_tab(TabRef::Preferences));
    assert!(!populated_but_hidden.graph_ctx().is_visible());
    assert_eq!(
        populated_but_hidden.graph_ctx().nodes().count(),
        2,
        "the graph still reads back — only the pane showing it is gone"
    );
}

#[test]
fn only_runnable_sinks_expose_the_disable_toggle() {
    use scenarium::FuncId;

    // The two axes the toggle depends on: sink or not, and resolvable or not.
    // `can_disable` reads only the first, since a node whose func the library
    // has never held — the third here — declares no sink.
    let mut g = TestGraph::new();
    let plain = g.add("plain", |n| n.output(DataType::Int));
    let sink = g.add("sink_func", NodeSpec::sink);
    let ghost = g.graph.add(internals::unresolved_node(FuncId::unique()));
    let fixture = GraphCtxFixture::over(g);
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
    let mut ghost =
        internals::unresolved_node(FuncId::literal("7a0265e1-9631-45bd-8ecd-1e923b67a58c"));
    ghost.name = "astro_to_image".into();
    // And one authored against other ports of a func the library does hold.
    let add_id = library.by_name("Add").unwrap().id();
    let known_id = graph.add(known);
    let ghost_id = graph.add(ghost);
    let changed_id = graph.add(internals::unresolved_node(add_id));

    let fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    // Both nodes resolve, not silently dropped — so the unresolvable one
    // stays selectable and deletable to repair the document.
    assert_eq!(graph_ctx.nodes().count(), 3, "every placed node resolves");
    let known_node = graph_ctx.node(known_id).unwrap();
    let ghost_node = graph_ctx.node(ghost_id).unwrap();

    // The flag tracks resolution; the label names what's missing.
    assert!(!known_node.missing(), "a resolved func is not a stub");
    assert!(ghost_node.missing());
    assert_eq!(ghost_node.kind_label(), "missing func");
    let changed_node = graph_ctx.node(changed_id).unwrap();
    assert!(
        changed_node.missing(),
        "a func that moved its ports is a stub too"
    );
    assert_eq!(changed_node.kind_label(), "changed func");

    // The stub keeps its saved name and carries no ports.
    assert_eq!(ghost_node.name(), "astro_to_image");
    assert_eq!(ghost_node.inputs().len(), 0);
    assert_eq!(ghost_node.outputs().len(), 0);
    assert_eq!(ghost_node.port_count(PortKind::Input), 0);

    // The resolved node, by contrast, exposes its real ports.
    assert!(
        known_node.inputs().len() == 2,
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

    let fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
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

    // Two frame-event nodes; subscribe the second to the first's "FPS"
    // event (event_idx 1). The context must surface that one edge.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(EventPort::new(emitter_id, 1), subscriber_id);

    let fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let subs: Vec<_> = fixture.graph_ctx().subscriptions().collect();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event, EventPort::new(emitter_id, 1));
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

    let fixture = GraphCtxFixture::over(DocFixture::with_library(graph, library));
    let graph_ctx = fixture.graph_ctx();

    for (id, mode) in ids {
        let node = graph_ctx.node(id).unwrap();
        assert_eq!(node.cache(), mode, "{mode:?} reads verbatim");
        assert!(node.cacheable());
    }
}

#[test]
fn impure_flag_reads_from_func_behavior() {
    // Three funcs differing only in what the header gate reads: a `Pure` one
    // (offers the storage toggles), an `Impure` one (no content digest, so the
    // toggles are hidden), and an outputless sink (nothing to store).
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
    // Both have an output, so `impure` is what tells them apart.
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

/// An input reads as set aside exactly while the input declared to override
/// it holds something: unbound or `Null` leaves the knob in force, a
/// constant or a wire from an enabled node sets it aside and names the
/// overrider, and a wire from a disabled node leaves it in force again.
#[test]
fn an_input_reads_as_set_aside_while_its_overrider_holds_something() {
    let mut g = TestGraph::new();
    let knob = g.add("knob", |n| {
        n.input(DataType::Float)
            .const_only()
            .optional(DataType::Float)
            .overrides(0)
    });
    let feed = g.add("feed", |n| n.output(DataType::Float));
    let config = InputPort::new(knob, 1);
    let mut fixture = GraphCtxFixture::over(g);
    let set_aside = |fixture: &mut GraphCtxFixture, binding: Option<Binding>| {
        fixture
            .open
            .document
            .graph
            .set_input_binding(config, binding);
        let graph_ctx = fixture.graph_ctx();
        let node = graph_ctx.node(knob).unwrap();
        (
            node.input(0).unwrap().overridden_by().map(str::to_owned),
            node.input(1).unwrap().overridden_by().map(str::to_owned),
        )
    };

    assert_eq!(set_aside(&mut fixture, None), (None, None));
    assert_eq!(
        set_aside(&mut fixture, Some(Binding::Const(ConstValue::Null))),
        (None, None)
    );
    assert_eq!(
        set_aside(&mut fixture, Some(Binding::Const(ConstValue::Float(2.0)))),
        (Some("in1".to_owned()), None),
        "a constant sets the knob aside, and the overrider itself is never set aside"
    );
    assert_eq!(
        set_aside(&mut fixture, Some(Binding::bind(feed, 0))),
        (Some("in1".to_owned()), None)
    );
    fixture.open.document.graph.find_mut(feed).unwrap().disabled = true;
    assert_eq!(
        set_aside(&mut fixture, Some(Binding::bind(feed, 0))),
        (None, None),
        "a disabled producer delivers nothing, so the knob is in force"
    );
}
