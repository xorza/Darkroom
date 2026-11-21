use scenarium::internals::test_graph::TestGraph;
use scenarium::{
    CacheMode, DataType, EventPort, FuncId, Graph, InstancePath, Keep, Node, NodeId, ProgramId,
    internals,
};
use scenarium::{PortId, TriggerPort, internals::test_graph::NodeSpec};

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
    let ghost = fixture.add_ghost_node(internals::unresolved_node(FuncId::unique()));
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
fn a_missing_func_reads_as_a_deletable_ghost_node() {
    use scenarium::math_library;

    // A resolvable func, and one whose id the library no longer holds.
    let library = math_library().build();
    let mut graph = Graph::default();
    let mut known: Node = library.by_name("Add").unwrap().into();
    known.disabled = true;
    let mut ghost =
        internals::unresolved_node(FuncId::literal("7a0265e1-9631-45bd-8ecd-1e923b67a58c"));
    ghost.name = Some("astro_to_image".to_owned());
    let known_id = graph.add(known);
    let mut fixture = DocumentFixture::with_library(graph, library);
    let ghost_id = fixture.add_ghost_node(ghost);

    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();

    // An unresolvable node is not dropped: it stays selectable and deletable.
    assert_eq!(graph_ctx.nodes().count(), 2, "every placed node resolves");
    let known_node = graph_ctx.node(known_id).unwrap();
    let ghost_node = graph_ctx.node(ghost_id).unwrap();

    assert!(!known_node.ghost(), "a resolved func is not a ghost node");
    assert!(ghost_node.ghost());
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

    // A targeted run overrides the disabled flag; a ghost node can't be run to.
    assert!(
        known_node.disabled() && known_node.runnable() == Some(ProgramId::from(known_node.node_id)),
        "a resolved disabled func can be targeted by a one-run override"
    );
    assert!(
        ghost_node.runnable().is_none(),
        "a ghost node offers no run affordance — it resolves to nothing"
    );
}

/// The ports a node shows that no declaration gives it: a ghost node's saved ports, in its
/// func's order, each input with the ghost it holds; and on a live node each port its ghost
/// wiring names that the node no longer declares, an output once however many ghost wires read
/// it, by its saved name where a save kept one. Ghost rows sit under the declared ones, and the
/// footer counts each ghost that touches the node.
#[test]
fn a_node_shows_the_ghost_ports_its_ghosts_name() {
    use scenarium::{
        Binding, DetachReason, GhostBinding, GhostNode, GhostSource, InputPort, InputState,
        Numbered, OutputPort, SavedDecl, SavedEvent, SavedInput, SavedPort, Subscription,
        SubscriptionState, Unresolved,
    };

    use crate::core::document::graph_view::NodePlacement;
    use crate::gui::graph_ctx::ghost_port::GhostPort;
    use crate::gui::port_ref::PortRef;

    fn rows<'a>(
        ports: impl Iterator<Item = GhostPort<'a>>,
    ) -> Vec<(PortRef, Option<&'a str>, Option<DataType>, bool)> {
        ports
            .map(|port| (port.port, port.name, port.data_type, port.held.is_some()))
            .collect()
    }

    let mut fixture = DocumentFixture::probes(2);
    let [a, b] = [0, 1].map(|i| fixture.node(i));
    let ghost = NodeId::unique();
    let saved = SavedDecl {
        name: "Denoise".to_owned(),
        category: "Astro".to_owned(),
        inputs: vec![Numbered::new(
            PortId::new(1),
            SavedInput {
                name: "Strength".to_owned(),
                data_type: DataType::Float,
                default_value: None,
            },
        )],
        outputs: vec![Numbered::new(
            PortId::new(1),
            SavedPort {
                name: "Image".to_owned(),
                data_type: DataType::Int,
            },
        )],
        events: vec![Numbered::new(
            PortId::new(1),
            SavedEvent {
                name: "done".to_owned(),
            },
        )],
    };
    let graph = fixture.document.graphs.main_mut();
    graph.insert_ghost_node(
        ghost,
        GhostNode {
            node: internals::unresolved_node(FuncId::unique()),
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            saved: Some(saved),
        },
    );
    let kept = |binding: Binding| InputState::Ghost(GhostBinding::from(binding));
    // `a` feeds the ghost node's Strength; the ghost node's Image feeds `b`'s input 1; `a`'s
    // output 7, which `probe` does not declare, feeds `b`'s 9, which it does not either, and
    // `b`'s 8 under a saved name.
    graph.set_input_state(
        InputPort::new(ghost, PortId::new(1)),
        kept(Binding::wire(a, PortId::new(1))),
    );
    graph.set_input_state(
        InputPort::new(b, PortId::new(1)),
        kept(Binding::wire(ghost, PortId::new(1))),
    );
    graph.set_input_state(
        InputPort::new(b, PortId::new(9)),
        kept(Binding::wire(a, PortId::new(7))),
    );
    graph.set_input_state(
        InputPort::new(b, PortId::new(8)),
        InputState::Ghost(GhostBinding {
            input: Some(SavedPort {
                name: "Gain".to_owned(),
                data_type: DataType::Float,
            }),
            source: GhostSource::Wire {
                output: OutputPort::new(a, PortId::new(7)),
                saved: None,
            },
        }),
    );
    for subscription in [
        Subscription::new(EventPort::new(a, PortId::new(4)), b),
        Subscription::new(EventPort::new(a, PortId::new(5)), ghost),
    ] {
        graph.set_subscription_state(
            subscription,
            SubscriptionState::Ghost(scenarium::GhostSubscription::default()),
        );
    }
    graph.validate().unwrap();
    fixture
        .document
        .main_view
        .node_placements
        .insert(ghost, NodePlacement::default());
    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();
    let node = graph_ctx.node(ghost).unwrap();

    assert_eq!(
        node.saved().map(|saved| saved.name.as_str()),
        Some("Denoise")
    );
    assert_eq!(
        rows(node.ghost_inputs()),
        [(
            PortRef::input(ghost, PortId::new(1)),
            Some("Strength"),
            Some(DataType::Float),
            true
        )]
    );
    assert_eq!(
        rows(node.ghost_outputs()),
        [(
            PortRef::output(ghost, PortId::new(1)),
            Some("Image"),
            Some(DataType::Int),
            false
        )]
    );
    assert_eq!(
        node.ghost_events()
            .map(|event| (event.event, event.name))
            .collect::<Vec<_>>(),
        [(EventPort::new(ghost, PortId::new(1)), Some("done"))]
    );
    assert_eq!(
        node.ghost_count(),
        3,
        "the wire in, the wire out and the subscription"
    );

    let producer = graph_ctx.node(a).unwrap();
    assert_eq!(rows(producer.ghost_inputs()), []);
    assert_eq!(
        rows(producer.ghost_outputs()),
        [(PortRef::output(a, PortId::new(7)), None, None, false)],
        "two ghost wires read output 7: one row"
    );
    assert_eq!(
        producer
            .ghost_events()
            .map(|event| (event.event, event.name))
            .collect::<Vec<_>>(),
        [
            (EventPort::new(a, PortId::new(4)), None),
            (EventPort::new(a, PortId::new(5)), None)
        ]
    );
    // A ghost trigger where a ghost subscription ends on a node that has no trigger: the ghost node,
    // and `b`, whose `probe` is no sink; never on the emitter.
    assert!(node.ghost_triggered());
    assert!(graph_ctx.node(b).unwrap().ghost_triggered());
    assert!(!producer.ghost_triggered());
    let consumer = graph_ctx.node(b).unwrap();
    assert_eq!(
        rows(consumer.ghost_inputs()),
        [
            (
                PortRef::input(b, PortId::new(8)),
                Some("Gain"),
                Some(DataType::Float),
                true
            ),
            (PortRef::input(b, PortId::new(9)), None, None, true),
        ],
        "input 1 is declared, so its ghost sits on its own row"
    );
    assert_eq!(consumer.ghost_count(), 4);
    assert!(consumer.input(PortId::new(1)).unwrap().ghost().is_some());
    assert_eq!(
        graph_ctx.ghost_end_type(PortRef::output(ghost, PortId::new(1)), None),
        Some(DataType::Int),
        "a ghost node's saved output types its ghost wires"
    );
    assert_eq!(
        graph_ctx.ghost_end_type(PortRef::output(a, PortId::new(7)), None),
        None
    );
    let gain = SavedPort {
        name: "Gain".to_owned(),
        data_type: DataType::Float,
    };
    assert_eq!(
        graph_ctx.ghost_end_type(PortRef::input(b, PortId::new(8)), Some(&gain)),
        Some(DataType::Float),
        "a port no declaration gives reads the one the ghost saved"
    );
    assert_eq!(
        graph_ctx.ghost_end_type(PortRef::input(b, PortId::new(1)), Some(&gain)),
        Some(DataType::Int),
        "a declared port reads its declaration, whatever the ghost saved"
    );
}

#[test]
fn func_events_read_in_order_alongside_outputs() {
    use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

    // The `frame event` func declares two events and two data outputs; each reads independently.
    let library = worker_events_library().build();
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
    let library = worker_events_library().build();
    let mut graph = Graph::default();
    let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let emitter_id = graph.add(emitter);
    let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
    let subscriber_id = graph.add(subscriber);
    graph.subscribe(EventPort::new(emitter_id, PortId::new(2)), subscriber_id);

    let fixture = GraphCtxFixture::over(DocumentFixture::with_library(graph, library));
    let subs: Vec<_> = fixture.graph_ctx().subscriptions().collect();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].event, EventPort::new(emitter_id, PortId::new(2)));
    assert_eq!(subs[0].trigger, TriggerPort::from(subscriber_id));
}

#[test]
fn cache_mode_reads_verbatim_per_node() {
    use scenarium::math_library;

    let library = math_library().build();
    let mut graph = Graph::default();
    let mut ids = Vec::new();
    for mode in [
        CacheMode::NEVER,
        CacheMode::ALWAYS,
        CacheMode {
            ram: Keep::Never,
            disk: true,
        },
        CacheMode {
            ram: Keep::Always,
            disk: true,
        },
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

/// An instance caches the values that leave it: one whose interface has an output shows the
/// storage chips, starting with nothing kept, and one with no output has nothing to store; a
/// boundary node has no cache mode.
#[test]
fn an_instance_with_an_output_is_cacheable() {
    use scenarium::{Compound, CompoundId, Interface, Numbered, OutputDecl, PortId, Ports};

    use crate::core::document::internals::insert_compound;

    let mut fixture = DocumentFixture::default();
    let (with, without) = (CompoundId::from_u128(1), CompoundId::from_u128(2));
    let outputs = Interface::new(Ports {
        inputs: vec![],
        outputs: vec![Numbered::new(
            PortId::new(1),
            OutputDecl::new("out", DataType::Int),
        )],
    });
    insert_compound(&mut fixture.document, with, Compound::new("with", outputs));
    insert_compound(
        &mut fixture.document,
        without,
        Compound::new("without", Interface::default()),
    );
    let [cached, empty] = [with, without].map(|id| fixture.add_instance(id));
    let fixture = GraphCtxFixture::over(fixture);
    let graph_ctx = fixture.graph_ctx();
    let cached = graph_ctx.node(cached).unwrap();
    assert_eq!(cached.cache(), Some(CacheMode::AUTO));
    assert!(cached.cacheable());
    assert!(!graph_ctx.node(empty).unwrap().cacheable());
}

/// A pane reads each run result through the instance it shows: a node of a compound reads its
/// program node under the side's `through`, and none when it shows none, when it can seed no run
/// either; an instance in the main graph reads what its inside came to and seeds no run, as it
/// has no body, and names the node inside that failed. Two instances, `i1` where the node ran in
/// 1.0 s and `i2` where it failed.
#[test]
fn run_results_read_through_the_instance_a_pane_shows() {
    use std::io;

    use palantir::Display;
    use scenarium::{
        Compound, CompoundId, GraphId, Interface, InvokeError, NodeExecStatus, NodeStatus,
        RamUsage, RunError, RunSummary,
    };

    use crate::core::document::internals::insert_compound;
    use crate::gui::state::run_state::ExecStatus;

    let mut document = DocumentFixture::default();
    let id = CompoundId::from_u128(1);
    let mut compound = Compound::new("inner", Interface::default());
    let inside = compound.graph.add(document.plain_node());
    insert_compound(&mut document.document, id, compound);
    let [i1, i2] = [(); 2].map(|()| document.add_instance(id));
    let mut fixture = GraphCtxFixture::over(document);
    let row = |instance, status| NodeStatus {
        program_id: ProgramId::of(InstancePath::new(&[instance]), inside),
        status: Some(status),
        ram: RamUsage::default(),
    };
    fixture.run_state.complete(RunSummary {
        nodes: vec![
            row(i1, NodeExecStatus::Executed { elapsed_secs: 1.0 }),
            row(
                i2,
                NodeExecStatus::Errored {
                    error: RunError::Invoke(InvokeError::external(io::Error::other("boom"))),
                },
            ),
        ],
        ..RunSummary::default()
    });
    let graph = GraphId::Compound(id);
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
    let in_i1 = ProgramId::of(InstancePath::new(&[i1]), inside);
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
    let inside_ctx = fixture.graph_ctx_of(graph, Display::default());
    assert_eq!(
        failing.failed_inside(),
        Some(inside_ctx.node(inside).unwrap().name()),
        "the footer names it"
    );
    assert_eq!(main.node(i1).unwrap().failed_inside(), None);
    assert_eq!(
        main.node(i1).unwrap().runnable(),
        None,
        "an instance has no body to run"
    );
}
