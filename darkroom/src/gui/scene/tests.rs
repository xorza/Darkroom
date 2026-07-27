use super::*;
use crate::gui::scene::internals::scene_node_stub;
use palantir::internals::UiHarness;
use scenarium::DataType;
use scenarium::testing;
use scenarium::{Graph, GraphDef};
use scenarium::{GraphId, InputPort, Node, OutputPort};

fn finput(name: &str, ty: DataType) -> FuncInput {
    FuncInput::optional(name, ty)
}

/// Project one root graph, the common single-pane case.
fn rebuild_entry<'a>(
    scene: &mut Scene,
    ui: &mut Ui,
    library: &Library,
    graph: &'a Graph,
    view: &'a GraphView,
) {
    scene.rebuild(
        ui,
        library,
        &RunState::default(),
        [GraphProjection {
            target: GraphRef::Main,
            source: SceneSource::Entry(graph),
            view,
        }],
    );
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

    node.missing = false;
    node.run_available = false;
    assert!(
        !node.runnable(),
        "a local definition pane runs nothing directly"
    );
    assert!(
        node.can_disable(),
        "run availability does not hide the authoring disable toggle"
    );
}

#[test]
fn a_graph_instance_runs_like_any_other_node_in_the_entry_graph() {
    // A composite dissolves into its interior rather than vanishing, so it
    // covers compiled work and seeds a run like a func does
    // (`CompiledGraph::run_targets`). Only wiring — a boundary node — and an
    // unresolved stub cover nothing.
    let mut arena = UiHarness::arena();
    let mut node = scene_node_stub(arena.ui(), NodeId::unique(), Vec2::ZERO);
    node.graph = Some(GraphLink::Local(GraphId::unique()));
    assert!(
        node.runnable(),
        "an instance in the entry graph is runnable"
    );

    node.run_available = false;
    assert!(
        !node.runnable(),
        "the pane, not the node kind, is what withholds a run inside a definition"
    );

    node.run_available = true;
    node.boundary = true;
    assert!(!node.runnable(), "a boundary node emits no compiled work");

    node.boundary = false;
    node.missing = true;
    assert!(!node.runnable(), "an unresolved stub resolves to nothing");

    // An output-less composite reads as a sink, and disabling one disables
    // its whole interior — so the toggle belongs to it too.
    node.missing = false;
    node.sink = true;
    assert!(node.can_disable());
}

#[derive(Debug)]
struct AdderGraph {
    graph: GraphDef,
    input: NodeId,
    output: NodeId,
}

fn adder_graph() -> AdderGraph {
    let in_node = Node::new(NodeKind::GraphInput);
    let out_node = Node::new(NodeKind::GraphOutput);
    let mut graph = GraphDef::new("Adder")
        .category("Graph")
        .inputs([finput("A", DataType::Int), finput("B", DataType::Float)])
        .output(FuncOutput::new("Sum", DataType::Int));
    let input = graph.body.add(in_node);
    let output = graph.body.add(out_node);
    graph
        .body
        .set_input_binding(InputPort::new(output, 0), Binding::bind(input, 0));
    AdderGraph {
        graph,
        input,
        output,
    }
}

#[test]
fn boundary_nodes_mirror_graph_interface() {
    let fixture = adder_graph();
    let view = GraphView::for_graph(&fixture.graph.body);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    let def_id = GraphId::unique();
    scene.rebuild(
        arena.ui(),
        &Library::default(),
        &RunState::default(),
        [GraphProjection {
            target: GraphRef::Local(def_id),
            source: SceneSource::Def(&fixture.graph),
            view: &view,
        }],
    );
    let graph = scene.graph(GraphRef::Local(def_id)).expect("projected");

    assert_eq!(graph.nodes().count(), 2, "both boundary nodes render");
    let expected_node_order = view
        .item_placements
        .keys()
        .filter_map(|item| match item {
            ItemRef::Node(node_id) => Some(*node_id),
            ItemRef::Pin(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        graph.nodes().map(|n| n.id).collect::<Vec<_>>(),
        expected_node_order,
        "node projection follows paint order"
    );
    let input_node = graph.node(fixture.input).unwrap();
    let output_node = graph.node(fixture.output).unwrap();
    assert!(
        graph.nodes().all(|n| n.owner == GraphRef::Local(def_id)),
        "every projected node names the graph it came from"
    );

    // Boundary nodes are labeled by role.
    assert_eq!(&*input_node.kind_label.borrow_str(), "Input");
    assert_eq!(&*output_node.kind_label.borrow_str(), "Output");

    // GraphInput's outputs mirror the graph inputs (A:Int, B:Float)
    // plus the untyped "+" placeholder — types align with names.
    let in_outs = graph.outputs(input_node.outputs);
    assert_eq!(in_outs.len(), 3, "two graph inputs + placeholder");
    assert!(matches!(in_outs[0].ty, DataType::Int));
    assert!(matches!(in_outs[1].ty, DataType::Float));
    assert!(
        matches!(in_outs[2].ty, DataType::Any),
        "placeholder untyped"
    );

    // GraphOutput's inputs mirror the graph output (Sum:Int) plus a
    // placeholder.
    let out_ins = graph.inputs(output_node.inputs);
    assert_eq!(out_ins.len(), 2, "one graph output + placeholder");
    assert!(matches!(out_ins[0].ty, DataType::Int));

    // GraphInput: 0 inputs; one output per graph *input*, named to
    // match, plus the trailing "+" placeholder.
    assert_eq!(graph.inputs(input_node.inputs).len(), 0);
    let in_out_names: Vec<String> = graph
        .outputs(input_node.outputs)
        .iter()
        .map(|o| o.name.borrow_str().to_owned())
        .collect();
    assert_eq!(in_out_names, ["A", "B", "+"]);
    assert!(input_node.graph.is_none() && !input_node.sink);
    assert!(
        input_node.boundary && output_node.boundary,
        "boundary nodes are flagged so const affordances are suppressed"
    );
    assert!(
        !input_node.runnable() && !output_node.runnable(),
        "boundary nodes offer no run affordance — they have no execution identity"
    );
    assert!(
        graph.nodes().all(|n| !n.run_available),
        "a local definition pane has no exact root execution identity"
    );

    // GraphOutput: one input per graph *output* plus the "+"
    // placeholder; 0 outputs.
    assert_eq!(graph.outputs(output_node.outputs).len(), 0);
    let out_in_names: Vec<String> = graph
        .inputs(output_node.inputs)
        .iter()
        .map(|i| i.name.borrow_str().to_owned())
        .collect();
    assert_eq!(out_in_names, ["Sum", "+"]);

    // The interior wire shows up as a connection between the boundaries.
    assert_eq!(graph.connections().len(), 1);
    let c = &graph.connections()[0];
    assert_eq!(c.src, OutputPort::new(fixture.input, 0));
    assert_eq!(c.tgt, InputPort::new(fixture.output, 0));
}

#[test]
fn two_graphs_project_into_one_pool_and_slice_back_apart() {
    use scenarium::math_library;

    // The root graph plus a local definition, both on screen. Every pool is
    // shared; each pane's spans must slice its own contents back out, and
    // nothing may leak across.
    let library = math_library();
    let fixture = adder_graph();
    let def_id = GraphId::unique();
    let mut root = Graph::default();
    let root_a = root.add_func_node(library.by_name("Add").unwrap());
    let root_b = root.add_func_node(library.by_name("Add").unwrap());
    root.set_input_binding(InputPort::new(root_b, 0), Binding::bind(root_a, 0));
    root.insert_graph(def_id, fixture.graph);

    let mut root_view = GraphView::for_graph(&root);
    root_view.viewport = Viewport {
        pan: Vec2::new(11.0, 22.0),
        zoom: 2.0,
    };
    root_view.selected.insert(ItemRef::Node(root_b));
    let def = root.find_graph(def_id).unwrap();
    let mut def_view = GraphView::for_graph(&def.body);
    def_view.viewport = Viewport {
        pan: Vec2::new(-5.0, 0.0),
        zoom: 0.5,
    };
    def_view.selected.insert(ItemRef::Node(fixture.input));

    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    scene.rebuild(
        arena.ui(),
        &library,
        &RunState::default(),
        [
            GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(&root),
                view: &root_view,
            },
            GraphProjection {
                target: GraphRef::Local(def_id),
                source: SceneSource::Def(def),
                view: &def_view,
            },
        ],
    );

    // One pool holds every node; each pane slices exactly its own.
    assert_eq!(scene.nodes.len(), 4, "2 root nodes + 2 boundary nodes");
    assert_eq!(
        scene.graphs().map(|g| g.target()).collect::<Vec<_>>(),
        [GraphRef::Main, GraphRef::Local(def_id)],
        "panes project in the order given"
    );
    let main = scene.graph(GraphRef::Main).unwrap();
    let nested = scene.graph(GraphRef::Local(def_id)).unwrap();
    // Each pane's span covers exactly its own nodes. Membership, not
    // order: a pane's paint order is its view's item order, which
    // `boundary_nodes_mirror_graph_interface` pins down separately.
    let ids = |g: GraphScene<'_>| {
        let mut ids: Vec<NodeId> = g.nodes().map(|n| n.id).collect();
        ids.sort();
        ids
    };
    let sorted = |mut v: Vec<NodeId>| {
        v.sort();
        v
    };
    assert_eq!(ids(main), sorted(vec![root_a, root_b]));
    assert_eq!(ids(nested), sorted(vec![fixture.input, fixture.output]));

    // A pane resolves only its own ids, and `owner` routes the other way.
    assert!(main.contains(root_a) && !main.contains(fixture.input));
    assert!(nested.contains(fixture.input) && !nested.contains(root_a));
    assert_eq!(scene.owner(root_a).unwrap().target(), GraphRef::Main);
    assert_eq!(
        scene.owner(fixture.input).unwrap().target(),
        GraphRef::Local(def_id)
    );

    // Viewport, selection, and wiring stay per pane.
    assert_eq!(main.viewport().zoom, 2.0);
    assert_eq!(nested.viewport().zoom, 0.5);
    assert_eq!(main.selected(), [ItemRef::Node(root_b)]);
    assert_eq!(nested.selected(), [ItemRef::Node(fixture.input)]);
    assert!(main.is_selected(ItemRef::Node(root_b)));
    assert!(
        !main.is_selected(ItemRef::Node(fixture.input)),
        "the other pane's selection is not this pane's"
    );
    assert_eq!(main.connections().len(), 1, "root's one wire");
    assert_eq!(nested.connections().len(), 1, "the definition's one wire");
    assert_eq!(
        main.connections()[0].tgt,
        InputPort::new(root_b, 0),
        "each pane's wire slice is its own"
    );

    // Run availability follows the target, not the pane order.
    assert!(main.nodes().all(|n| n.run_available));
    assert!(nested.nodes().all(|n| !n.run_available));

    // A second rebuild with only the root drops the closed pane wholesale.
    rebuild_entry(&mut scene, arena.ui(), &library, &root, &root_view);
    assert!(scene.graph(GraphRef::Local(def_id)).is_none());
    assert_eq!(scene.nodes.len(), 2, "the closed pane's nodes are gone");
    assert_eq!(scene.graph(GraphRef::Main).unwrap().nodes().count(), 2);
}

#[test]
fn missing_func_and_graph_render_as_deletable_stubs() {
    use scenarium::GraphLink;
    use scenarium::math_library;

    // A resolvable func, plus two unresolvable nodes (e.g. a document
    // saved against an older library): a func id and a shared graph id.
    let library = math_library();
    let mut graph = Graph::default();
    let mut known: Node = library.by_name("Add").unwrap().into();
    known.disabled = true;
    let mut ghost_func = Node::new(NodeKind::Func(
        "7a0265e1-9631-45bd-8ecd-1e923b67a58c".into(),
    ));
    ghost_func.name = "astro_to_image".into();
    let mut ghost_graph = Node::new(NodeKind::Graph(GraphLink::Shared(
        "00000000-0000-0000-0000-0000000000ff".into(),
    )));
    ghost_graph.name = "removed_graph".into();
    let known_id = graph.add(known);
    let ghost_func_id = graph.add(ghost_func);
    let ghost_graph_id = graph.add(ghost_graph);

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    let projected = scene.graph(GraphRef::Main).unwrap();

    // Every node renders, not silently dropped — so the unresolvable ones
    // stay selectable and deletable to repair the document.
    assert_eq!(projected.nodes().count(), 3, "all nodes render");
    let known_node = projected.node(known_id).unwrap();
    let ghost_func_node = projected.node(ghost_func_id).unwrap();
    let ghost_graph_node = projected.node(ghost_graph_id).unwrap();

    // The flag tracks resolution; the label names what's missing.
    assert!(!known_node.missing, "a resolved func is not a stub");
    assert!(ghost_func_node.missing && ghost_graph_node.missing);
    assert_eq!(&*ghost_func_node.kind_label.borrow_str(), "missing func");
    assert_eq!(&*ghost_graph_node.kind_label.borrow_str(), "missing graph");

    // Both stubs keep their saved name and carry no ports — and the
    // graph stub drops its link so "open in tab" is unavailable.
    assert_eq!(&*ghost_func_node.name.borrow_str(), "astro_to_image");
    assert_eq!(&*ghost_graph_node.name.borrow_str(), "removed_graph");
    assert!(ghost_graph_node.graph.is_none());
    for stub in [ghost_func_node, ghost_graph_node] {
        assert_eq!(projected.inputs(stub.inputs).len(), 0, "stub has no inputs");
        assert_eq!(
            projected.outputs(stub.outputs).len(),
            0,
            "stub has no outputs"
        );
    }

    // The resolved node, by contrast, exposes its real ports.
    assert!(
        !projected.inputs(known_node.inputs).is_empty(),
        "the resolved func still renders its interface"
    );

    // Run seeding follows resolution: the resolved func can be run to,
    // the stubs (and any graph instance) can't.
    assert!(
        known_node.disabled && known_node.runnable(),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        !ghost_func_node.runnable() && !ghost_graph_node.runnable(),
        "stubs offer no run affordance — they resolve to nothing"
    );

    // The same graph projected as a local definition pane instead: run
    // availability is a property of the target, so the resolved func loses
    // its play chip.
    let def = GraphDef {
        body: graph,
        ..Default::default()
    };
    let def_id = GraphId::unique();
    scene.rebuild(
        arena.ui(),
        &library,
        &RunState::default(),
        [GraphProjection {
            target: GraphRef::Local(def_id),
            source: SceneSource::Def(&def),
            view: &view,
        }],
    );
    assert!(
        !scene
            .graph(GraphRef::Local(def_id))
            .unwrap()
            .node(known_id)
            .unwrap()
            .runnable(),
        "a local-definition projection hides the run affordance from resolved functions"
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

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    let projected = scene.graph(GraphRef::Main).unwrap();

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
fn pinned_output_projects_per_output_port_and_shares_the_z_order() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // "frame event" has two data outputs (Delta, Frame #); pin only the
    // second and confirm the flag lands on the right pooled entry, not
    // both or neither.
    let library = worker_events_library();
    let mut graph = Graph::default();
    let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let node_id = graph.add(node);
    let port = OutputPort::new(node_id, 1);
    graph.set_output_pinned(port, true);

    let mut view = GraphView::for_graph(&graph);
    let pin_key = ItemRef::Pin(port);
    *view.item_placements.get_mut(&pin_key).unwrap() = Vec2::new(320.0, -40.0);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    let projected = scene.graph(GraphRef::Main).unwrap();

    let n = projected.node(node_id).unwrap();
    let pins: Vec<Option<Vec2>> = projected
        .outputs(n.outputs)
        .iter()
        .map(|o| o.pin_position)
        .collect();
    assert_eq!(
        pins,
        [None, Some(Vec2::new(320.0, -40.0))],
        "only the pinned port carries a position, projected from its item"
    );
    // The surfaced pin carries the node it hangs off, so a pin scan can
    // reach its run affordance and its owning graph without a second lookup.
    let pinned: Vec<_> = projected.pinned_outputs().collect();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].port, port);
    assert_eq!(pinned[0].node.id, node_id);
    assert_eq!(pinned[0].node.owner, GraphRef::Main);

    // The shared paint stack mirrors `item_placements` order — node then
    // pin here (`for_graph` seeds pins after nodes)...
    assert_eq!(
        projected.z_order(),
        [ItemRef::Node(node_id), pin_key],
        "z_order interleaves node bodies and pin previews in item order"
    );

    // ...and a reorder (pin buried beneath the node) projects verbatim.
    view.move_item_to_index(&pin_key, 0);
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    assert_eq!(
        scene.graph(GraphRef::Main).unwrap().z_order(),
        [pin_key, ItemRef::Node(node_id)],
        "restacking the view items restacks the projected z_order"
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

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);

    let subs = scene.graph(GraphRef::Main).unwrap().subscriptions();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].emitter, emitter_id);
    assert_eq!(subs[0].event_idx, 1);
    assert_eq!(subs[0].subscriber, subscriber_id);
}

#[test]
fn local_defs_project_per_pane_ordered_by_id() {
    // The palette instances a `GraphLink::Local`, which resolves only
    // against the graph that *holds* the definition — so each pane must see
    // its own `graphs` map and no one else's.
    let library = Library::default();
    let origin = GraphId::unique();
    let mut nested = GraphDef::new("Inner").category("Nested");
    let buried = GraphId::unique();
    nested.body.insert_graph(buried, GraphDef::new("Buried"));

    let mut published = GraphDef::new("Published").category("Document");
    published.interface.origin = Some(origin);

    let mut graph = Graph::default();
    let (first, second) = (GraphId::unique(), GraphId::unique());
    graph.insert_graph(first, published);
    graph.insert_graph(second, nested);

    let view = GraphView::for_graph(&graph);
    let nested_view = GraphView::default();
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    let ui = arena.ui();
    scene.rebuild(
        ui,
        &library,
        &RunState::default(),
        [
            GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(&graph),
                view: &view,
            },
            GraphProjection {
                target: GraphRef::Local(second),
                source: SceneSource::Def(&graph.graphs[&second]),
                view: &nested_view,
            },
        ],
    );

    let root = scene.graph(GraphRef::Main).unwrap().local_defs();
    assert_eq!(root.len(), 2, "root sees its own two definitions only");
    // Ordered by id, not by `HashMap` iteration order.
    let (lo, hi) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!([root[0].id, root[1].id], [lo, hi]);
    let published = root.iter().find(|def| def.id == first).unwrap();
    assert_eq!(&*published.name.borrow_str(), "Published");
    assert_eq!(&*published.category.borrow_str(), "Document");
    assert_eq!(
        published.origin,
        Some(origin),
        "lineage rides along so the palette can drop the library's own row"
    );
    let nested = root.iter().find(|def| def.id == second).unwrap();
    assert_eq!(&*nested.category.borrow_str(), "Nested");
    assert_eq!(nested.origin, None);

    // The nested pane sees only what *it* holds — its parent's siblings are
    // not instanceable from inside it.
    let inner = scene.graph(GraphRef::Local(second)).unwrap().local_defs();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].id, buried);
    assert_eq!(&*inner[0].name.borrow_str(), "Buried");
    assert_eq!(
        &*inner[0].category.borrow_str(),
        "",
        "a definition with no category interns to the empty handle"
    );
}

#[test]
fn cache_mode_projects_verbatim_per_node() {
    use scenarium::math_library;

    // One `Add` node per cache mode; each `SceneNode.cache` must mirror its source
    // node's mode exactly (the header reads the two bits off it).
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

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    let projected = scene.graph(GraphRef::Main).unwrap();

    for (id, mode) in ids {
        let node = projected.node(id).unwrap();
        assert_eq!(node.cache, mode, "{mode:?} projects verbatim");
        assert!(node.cache_controls);
    }
}

#[test]
fn graph_instances_can_evict_but_have_no_direct_cache_storage_controls() {
    use scenarium::math_library;

    let library = math_library();
    let nested = GraphDef::new("Nested").output(FuncOutput::new("Out", DataType::Int));
    let nested_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance_id = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
    graph.insert_graph(nested_id, nested);

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);

    let instance = scene
        .graph(GraphRef::Main)
        .unwrap()
        .node(instance_id)
        .unwrap();
    assert!(
        instance.can_evict_cache,
        "an instance can evict its flattened interior"
    );
    assert!(
        !instance.cache_controls,
        "an instance has no runtime slot on which to store an output"
    );
}

#[test]
fn impure_flag_projects_from_func_behavior() {
    use scenarium::{Func, FuncId};

    // Two funcs identical but for behavior: a `Pure` one (offers the disk-cache
    // toggle) and an `Impure` one (has no content digest, so the toggle is hidden).
    // Both have an output, so `impure` is the sole
    // differentiator the header gate reads.
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

    let view = GraphView::for_graph(&graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &graph, &view);
    let projected = scene.graph(GraphRef::Main).unwrap();

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
