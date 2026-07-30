use super::*;
use crate::gui::scene::internals::scene_node_stub;
use palantir::internals::UiHarness;
use scenarium::DataType;
use scenarium::FuncOutput;
use scenarium::Graph;
use scenarium::Node;
use scenarium::testing;

/// Project the document's graph, which is the only thing a scene ever shows.
fn rebuild_entry(scene: &mut Scene, ui: &mut Ui, library: &Library, doc: &Document) {
    scene.rebuild(ui, library, &RunState::default(), doc);
}

/// The graph pane, over both halves it reads.
fn entry_pane<'a>(scene: &'a Scene, doc: &'a Document) -> Pane<'a> {
    scene.pane(doc).expect("the graph pane is projected")
}

#[test]
fn only_runnable_sinks_expose_the_disable_toggle() {
    let mut arena = UiHarness::arena();
    let mut node = scene_node_stub(arena.ui(), NodeId::unique(), Vec2::ZERO);
    assert!(!node.can_disable(), "a non-sink has no disable toggle");

    node.sink = true;
    assert!(node.can_disable(), "a runnable sink can be disabled");

    node.missing = true;
    assert!(
        !node.can_disable(),
        "an unresolved sink cannot be disabled because it cannot be run explicitly"
    );
}

#[test]
fn a_missing_func_renders_as_a_deletable_stub() {
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

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let projected = entry_pane(&scene, &doc);

    // Both nodes render, not silently dropped — so the unresolvable one
    // stays selectable and deletable to repair the document.
    assert_eq!(projected.nodes().count(), 2, "all nodes render");
    let known_node = projected.node(known_id).unwrap();
    let ghost_node = projected.node(ghost_id).unwrap();

    // The flag tracks resolution; the label names what's missing.
    assert!(!known_node.missing, "a resolved func is not a stub");
    assert!(ghost_node.missing);
    assert_eq!(&*ghost_node.kind_label.borrow_str(), "missing func");

    // The stub keeps its saved name and carries no ports.
    assert_eq!(&*ghost_node.name.borrow_str(), "astro_to_image");
    assert_eq!(projected.inputs(ghost_node.inputs).len(), 0);
    assert_eq!(projected.outputs(ghost_node.outputs).len(), 0);

    // The resolved node, by contrast, exposes its real ports.
    assert!(
        !projected.inputs(known_node.inputs).is_empty(),
        "the resolved func still renders its interface"
    );

    // Run seeding follows resolution: the resolved func can be run to even
    // while disabled (a targeted run overrides the flag); the stub can't.
    assert!(
        known_node.disabled && projected.runnable(known_node),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        !projected.runnable(ghost_node),
        "a stub offers no run affordance — it resolves to nothing"
    );
}

#[test]
fn func_events_project_in_order_alongside_outputs() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // The `frame event` func declares two events ("Always", "FPS") and two
    // data outputs ("Delta", "Frame #"); the projection must surface both
    // independently — events in their own pool, outputs unchanged.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let node_id = graph.add(node);

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let projected = entry_pane(&scene, &doc);

    let n = projected.node(node_id).unwrap();
    let event_names: Vec<String> = projected
        .events(n.events)
        .iter()
        .map(|e| e.name.borrow_str().to_owned())
        .collect();
    assert_eq!(event_names, ["Always", "FPS"], "events project in order");

    let output_names: Vec<String> = projected
        .outputs(n.outputs)
        .iter()
        .map(|o| o.name.borrow_str().to_owned())
        .collect();
    assert_eq!(
        output_names,
        ["Delta", "Frame #"],
        "data outputs are unaffected by events"
    );
}

#[test]
fn subscriptions_project_from_graph() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // Two frame-event nodes; subscribe the second to the first's "FPS"
    // event (event_idx 1). The projection must mirror that one edge.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(emitter_id, 1, subscriber_id);

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);

    let subs: Vec<_> = entry_pane(&scene, &doc).subscriptions().collect();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].emitter, emitter_id);
    assert_eq!(subs[0].event_idx, 1);
    assert_eq!(subs[0].subscriber, subscriber_id);
}

#[test]
fn cache_mode_projects_verbatim_per_node() {
    use scenarium::math_library;

    // One `Add` node per cache mode; each `SceneNode.cache` must mirror its
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

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let projected = entry_pane(&scene, &doc);

    for (id, mode) in ids {
        let node = projected.node(id).unwrap();
        assert_eq!(node.cache, mode, "{mode:?} projects verbatim");
        assert!(node.cache_controls);
    }
}

#[test]
fn impure_flag_projects_from_func_behavior() {
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

    let mut graph = Graph::default();
    let pure_id = graph.add_func_node(library.by_name("pure_src").unwrap());
    let impure_id = graph.add_func_node(library.by_name("impure_src").unwrap());
    let self_cached_id = graph.add_func_node(library.by_name("self_cached").unwrap());

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let projected = entry_pane(&scene, &doc);

    let pure = projected.node(pure_id).unwrap();
    let impure = projected.node(impure_id).unwrap();
    let self_cached = projected.node(self_cached_id).unwrap();

    assert!(!pure.impure, "a Pure func keeps its cache chips");
    assert!(impure.impure, "an Impure func hides its cache chips");
    // Both have an output, so `impure` is the sole eviction differentiator.
    assert!(
        pure.can_evict_cache,
        "a Pure func with an output can be evicted"
    );
    assert!(!impure.can_evict_cache, "an Impure func cannot be evicted");
    assert!(pure.cache_controls);
    assert!(!impure.cache_controls);
    assert!(
        self_cached.can_evict_cache,
        "self-caching funcs can still have cached downstream consumers"
    );
    assert!(
        !self_cached.cache_controls,
        "self-caching funcs hide Scenarium storage controls"
    );
    assert!(!pure.sink && !impure.sink);
}
