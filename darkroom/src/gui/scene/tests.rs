use super::*;
use crate::gui::scene::internals::{SceneFixture, scene_node_stub};
use palantir::internals::UiHarness;
use scenarium::DataType;
use scenarium::testing;
use scenarium::{Graph, GraphDef};
use scenarium::{GraphId, InputPort, Node, OutputPort};

fn finput(name: &str, ty: DataType) -> FuncInput {
    FuncInput::optional(name, ty)
}

/// Project one root graph, the common single-pane case.
fn rebuild_entry(scene: &mut Scene, ui: &mut Ui, library: &Library, doc: &Document) {
    scene.rebuild(
        ui,
        library,
        &RunState::default(),
        [GraphProjection {
            target: GraphRef::Main,
            source: SceneSource::Entry(&doc.graph),
            view: &doc.main_view,
        }],
    );
}

/// The sole pane of a single-pane scene, over both halves it reads.
fn entry_pane<'a>(scene: &'a Scene, doc: &'a Document) -> Pane<'a> {
    Frame { scene, doc }
        .pane(GraphRef::Main)
        .expect("the entry pane is projected")
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
    let definition_pane = SceneFixture::with_nodes([node]).without_run_target();
    let pane = definition_pane.only_pane();
    let node = pane.nodes().next().expect("the stub is projected");
    assert!(
        !pane.runnable(node),
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
        node.executable_kind(),
        "an instance covers compiled work like a func does"
    );

    node.boundary = true;
    assert!(
        !node.executable_kind(),
        "a boundary node emits no compiled work"
    );

    node.boundary = false;
    node.missing = true;
    assert!(
        !node.executable_kind(),
        "an unresolved stub resolves to nothing"
    );

    // The pane is the other half: an executable node still isn't runnable
    // inside a definition, which is no particular instance of itself.
    let mut runnable = scene_node_stub(arena.ui(), NodeId::unique(), Vec2::ZERO);
    runnable.graph = Some(GraphLink::Local(GraphId::unique()));
    let entry = SceneFixture::with_nodes([runnable]);
    let entry_pane = entry.only_pane();
    assert!(entry_pane.runnable(entry_pane.nodes().next().unwrap()));

    let mut interior = scene_node_stub(arena.ui(), NodeId::unique(), Vec2::ZERO);
    interior.graph = Some(GraphLink::Local(GraphId::unique()));
    let definition = SceneFixture::with_nodes([interior]).without_run_target();
    let definition_pane = definition.only_pane();
    assert!(
        !definition_pane.runnable(definition_pane.nodes().next().unwrap()),
        "the pane, not the node kind, is what withholds a run inside a definition"
    );

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
    let def_id = GraphId::unique();
    let local = GraphRef::Local(def_id);
    let mut doc = Document::default();
    doc.graph.insert_graph(def_id, fixture.graph);
    assert!(doc.ensure_sub_view(def_id), "the def was just inserted");
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    scene.rebuild(
        arena.ui(),
        &Library::default(),
        &RunState::default(),
        [GraphProjection {
            target: local,
            source: SceneSource::Def(doc.graph.find_graph(def_id).unwrap()),
            view: doc.view(local).unwrap(),
        }],
    );
    let view = doc.view(local).unwrap();
    let graph = Frame {
        scene: &scene,
        doc: &doc,
    }
    .pane(local)
    .expect("projected");

    assert_eq!(graph.nodes().count(), 2, "both boundary nodes render");
    let expected_node_order = view.item_placements.keys().copied().collect::<Vec<_>>();
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
        !graph.runnable(input_node) && !graph.runnable(output_node),
        "boundary nodes offer no run affordance — they emit no compiled work"
    );
    assert!(
        !graph.run_available(),
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
    assert_eq!(graph.connections().count(), 1);
    let (consumer, producer) = graph.connections().next().unwrap();
    assert_eq!(producer, OutputPort::new(fixture.input, 0));
    assert_eq!(consumer, InputPort::new(fixture.output, 0));
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

    let mut doc = Document::from(root);
    doc.main_view.viewport = Viewport {
        pan: Vec2::new(11.0, 22.0),
        zoom: 2.0,
    };
    doc.main_view.selected.insert(root_b);
    let local = GraphRef::Local(def_id);
    assert!(doc.ensure_sub_view(def_id), "the def is in the document");
    let def_view = doc.scope_mut(local).expect("the def resolves").view;
    def_view.viewport = Viewport {
        pan: Vec2::new(-5.0, 0.0),
        zoom: 0.5,
    };
    def_view.selected.insert(fixture.input);

    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    scene.rebuild(
        arena.ui(),
        &library,
        &RunState::default(),
        [
            GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(&doc.graph),
                view: &doc.main_view,
            },
            GraphProjection {
                target: local,
                source: SceneSource::Def(doc.graph.find_graph(def_id).unwrap()),
                view: doc.view(local).unwrap(),
            },
        ],
    );
    let frame = Frame {
        scene: &scene,
        doc: &doc,
    };

    // One pool holds every node; each pane slices exactly its own.
    assert_eq!(scene.nodes.len(), 4, "2 root nodes + 2 boundary nodes");
    assert_eq!(
        frame.panes().map(|g| g.target()).collect::<Vec<_>>(),
        [GraphRef::Main, GraphRef::Local(def_id)],
        "panes project in the order given"
    );
    let main = frame.pane(GraphRef::Main).unwrap();
    let nested = frame.pane(local).unwrap();
    // Each pane's span covers exactly its own nodes. Membership, not
    // order: a pane's paint order is its view's item order, which
    // `boundary_nodes_mirror_graph_interface` pins down separately.
    let ids = |g: Pane<'_>| {
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
    assert_eq!(frame.owner(root_a).unwrap().target(), GraphRef::Main);
    assert_eq!(frame.owner(fixture.input).unwrap().target(), local);

    // Viewport, selection, and wiring stay per pane.
    assert_eq!(main.viewport().zoom, 2.0);
    assert_eq!(nested.viewport().zoom, 0.5);
    assert_eq!(main.selected(), &BTreeSet::from([root_b]));
    assert_eq!(nested.selected(), &BTreeSet::from([fixture.input]));
    assert!(main.is_selected(root_b));
    assert!(
        !main.is_selected(fixture.input),
        "the other pane's selection is not this pane's"
    );
    assert_eq!(main.connections().count(), 1, "root's one wire");
    assert_eq!(nested.connections().count(), 1, "the definition's one wire");
    assert_eq!(
        main.connections().next().unwrap().0,
        InputPort::new(root_b, 0),
        "each pane reads its own wiring"
    );

    // Run availability follows the target, not the pane order.
    assert!(main.run_available());
    assert!(!nested.run_available());

    // A second rebuild with only the root drops the closed pane wholesale.
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let frame = Frame {
        scene: &scene,
        doc: &doc,
    };
    assert!(frame.pane(local).is_none());
    assert_eq!(scene.nodes.len(), 2, "the closed pane's nodes are gone");
    assert_eq!(frame.pane(GraphRef::Main).unwrap().nodes().count(), 2);
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

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let projected = entry_pane(&scene, &doc);

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
        known_node.disabled && projected.runnable(known_node),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        !projected.runnable(ghost_func_node) && !projected.runnable(ghost_graph_node),
        "stubs offer no run affordance — they resolve to nothing"
    );

    // The same graph projected as a local definition pane instead: run
    // availability is a property of the target, so the resolved func loses
    // its play chip.
    let def_id = GraphId::unique();
    let local = GraphRef::Local(def_id);
    let mut def_doc = Document::default();
    def_doc.graph.insert_graph(
        def_id,
        GraphDef {
            body: doc.graph.clone_verbatim(),
            ..Default::default()
        },
    );
    assert!(def_doc.ensure_sub_view(def_id), "the def was just inserted");
    scene.rebuild(
        arena.ui(),
        &library,
        &RunState::default(),
        [GraphProjection {
            target: local,
            source: SceneSource::Def(def_doc.graph.find_graph(def_id).unwrap()),
            view: def_doc.view(local).unwrap(),
        }],
    );
    let definition_pane = Frame {
        scene: &scene,
        doc: &def_doc,
    }
    .pane(local)
    .unwrap();
    assert!(
        !definition_pane.runnable(definition_pane.node(known_id).unwrap()),
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
fn a_composites_marker_flags_come_from_its_interior_once_compiled() {
    use scenarium::testing::{TestFuncHooks, test_func_lib};
    use scenarium::{Binding, Compiler, FuncOutput};
    use std::sync::Arc;

    // A composite that exposes an output *and* wraps a sink. Port arity says
    // "not a sink" — which is all the editor can tell on its own, and it is
    // wrong: the interior sink is what a sinks run reaches, what disabling
    // the instance suppresses, and what an event subscription would drive.
    let library = test_func_lib(TestFuncHooks::default());
    let mut nested = GraphDef::new("Nested").output(FuncOutput::new("out", DataType::Int));
    let boundary = nested.body.add(Node::new(NodeKind::GraphOutput));
    let source = nested.body.add(library.by_name("get_b").unwrap().into());
    let printer = nested.body.add(library.by_name("Print").unwrap().into());
    nested
        .body
        .set_input_binding(InputPort::new(printer, 0), Binding::bind(source, 0));
    nested
        .body
        .set_input_binding(InputPort::new(boundary, 0), Binding::bind(source, 0));

    let nested_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
    graph.insert_graph(nested_id, nested);
    let doc = Document::from(graph);

    // Nothing compiled yet: the projection keeps the port-arity reading, so
    // the node still draws before the first run instead of losing markers.
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);
    let node = entry_pane(&scene, &doc).node(instance).unwrap();
    assert!(
        !node.sink,
        "with no program to fold, an instance with outputs reads as a non-sink"
    );
    assert!(
        !node.impure,
        "and as pure — a composite has no declaration of its own to read"
    );

    // Compiled, the interior answers instead.
    let run_state = crate::gui::run_state::internals::with_compiled(Arc::new(
        Compiler::default().compile(&doc.graph, &library).unwrap(),
    ));
    scene.rebuild(
        arena.ui(),
        &library,
        &run_state,
        [GraphProjection {
            target: GraphRef::Main,
            source: SceneSource::Entry(&doc.graph),
            view: &doc.main_view,
        }],
    );
    let node = entry_pane(&scene, &doc).node(instance).unwrap();
    assert!(node.sink, "the interior sink makes the instance one");
    assert!(
        node.can_disable(),
        "which is what puts the disable toggle on it"
    );
    assert!(
        node.impure,
        "and the interior `Print` — impure, like any func not declaring `.pure()` — \
         makes the instance's result unreusable"
    );
    // The storage chips were never the impure marker's business for a
    // composite: they are absent because its storage is the interior's, and
    // eviction stays offered either way.
    assert!(!node.cache_controls && node.can_evict_cache);
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
    published.origin = Some(origin);

    let mut graph = Graph::default();
    let (first, second) = (GraphId::unique(), GraphId::unique());
    graph.insert_graph(first, published);
    graph.insert_graph(second, nested);

    let mut doc = Document::from(graph);
    let inner_pane = GraphRef::Local(second);
    assert!(doc.ensure_sub_view(second), "the def is in the document");
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
                source: SceneSource::Entry(&doc.graph),
                view: &doc.main_view,
            },
            GraphProjection {
                target: inner_pane,
                source: SceneSource::Def(&doc.graph.graphs[&second]),
                view: doc.view(inner_pane).unwrap(),
            },
        ],
    );
    let frame = Frame {
        scene: &scene,
        doc: &doc,
    };

    let root = entry_pane(&scene, &doc).local_defs();
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
    let inner = frame.pane(inner_pane).unwrap().local_defs();
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
fn graph_instances_can_evict_but_have_no_direct_cache_storage_controls() {
    use scenarium::math_library;

    let library = math_library();
    let nested = GraphDef::new("Nested").output(FuncOutput::new("Out", DataType::Int));
    let nested_id = GraphId::unique();
    let mut graph = Graph::default();
    let instance_id = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
    graph.insert_graph(nested_id, nested);

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    rebuild_entry(&mut scene, arena.ui(), &library, &doc);

    let instance = entry_pane(&scene, &doc).node(instance_id).unwrap();
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
