use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::error::{DetachReason, Unresolved};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSource, InputState};
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::{FuncId, GraphId, InputPort, PortId};
use crate::graph::manifest::Manifest;
use crate::graph::node_decl::NodeDecl;
use crate::graph::port_map::{NodeRemap, PortMap, PortMapError};
use crate::graph::ports::{InputDecl, OutputDecl};
use crate::graph::reconciled::Repair;
use crate::graph::saved_decl::SavedPort;
use crate::internals;
use crate::internals::test_graph::sets::{SINK, SRC, library, n, node};
use crate::library::Library;
use crate::library::func_redirect::FuncRedirect;

fn p(id: u32) -> PortId {
    PortId::new(id)
}

/// `blur`: inputs 1 `x`, an `Int`, and 2 `y`, a `Float`; output 1 `out`, an `Int`.
const BLUR: FuncId = FuncId::from_u128(70);
/// `sharpen`: inputs 5 `x`, an `Int`, and 1 `z`, a `String`; output 1 `out`, an `Int`.
const SHARPEN: FuncId = FuncId::from_u128(71);
/// `soften`: input 1 `w`, a `String`, and output 1 `out`, an `Int`: no `x`, and its input 1 is
/// another port than `blur`'s.
const SOFTEN: FuncId = FuncId::from_u128(72);

fn with_filters(mut library: Library) -> Library {
    library.add(
        internals::stub_func(BLUR, "blur")
            .pure()
            .input(1, InputDecl::optional("x", DataType::Int))
            .input(2, InputDecl::optional("y", DataType::Float))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build(),
    );
    library.add(
        internals::stub_func(SHARPEN, "sharpen")
            .pure()
            .input(5, InputDecl::optional("x", DataType::Int))
            .input(1, InputDecl::optional("z", DataType::String))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build(),
    );
    library.add(
        internals::stub_func(SOFTEN, "soften")
            .pure()
            .input(1, InputDecl::optional("w", DataType::String))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build(),
    );
    library
}

/// A map is an injection: an old id twice or a new id twice is refused, a lookup answers the
/// pairs alone, and one map followed by another drops what the second does not name.
#[test]
fn a_port_map_is_an_injection_and_composes() {
    assert_eq!(
        PortMap::checked([(p(1), p(2)), (p(1), p(3))]),
        Err(PortMapError::OldTwice(p(1)))
    );
    assert_eq!(
        PortMap::checked([(p(1), p(3)), (p(2), p(3))]),
        Err(PortMapError::NewTwice(p(3)))
    );
    let first = PortMap::new([(p(2), p(20)), (p(1), p(10))]);
    assert_eq!(first.pairs(), [(p(1), p(10)), (p(2), p(20))]);
    assert_eq!(first.get(p(2)), Some(p(20)));
    assert_eq!(first.get(p(3)), None);
    let second = PortMap::new([(p(10), p(100))]);
    assert_eq!(first.then(&second).pairs(), [(p(1), p(100))]);
}

/// A proposal pairs a port with the new one of its id and type, else an unpaired one of its name
/// and type: `blur` to `sharpen` takes `x` 1 to `x` 5 by name, as 1 is a `String` there, leaves
/// `y`, which `sharpen` lacks, and takes `out` 1 to 1 by id.
#[test]
fn a_proposal_pairs_by_id_and_type_then_name_and_type() {
    let library = with_filters(Library::default());
    let saved = |id| NodeDecl::Func(library.by_id(id).unwrap()).saved();
    let remap = NodeRemap::propose(&saved(BLUR), &saved(SHARPEN));
    assert_eq!(remap.inputs.pairs(), [(p(1), p(5))]);
    assert_eq!(remap.outputs.pairs(), [(p(1), p(1))]);
    assert!(remap.events.pairs().is_empty());
}

/// `blur` 2 in a graph: `src` 1 feeds `x`, `y` holds 0.5, `sink` 3 reads `out`. As `sharpen`
/// through the proposal: `src` feeds `x` 5 and `sink` reads `out` live, and 0.5 is a ghost on
/// `y` 2, under `y`'s name and type. As `soften`, which has no `x` and gives 1 to `w`: `x`'s wire
/// is a ghost on a minted id that `soften` does not give, so no load makes it live on `w`.
#[test]
fn a_replaced_node_keeps_what_the_map_names_and_ghosts_the_rest() {
    let library = with_filters(library());
    let saved = |id| NodeDecl::Func(library.by_id(id).unwrap()).saved();
    let mut graph = Graph::default();
    graph.insert(n(1), node(&library, SRC));
    graph.insert(n(2), node(&library, BLUR));
    graph.insert(n(3), node(&library, SINK));
    graph.set_input_binding(InputPort::new(n(2), p(1)), Binding::wire(n(1), p(1)));
    graph.set_input_binding(
        InputPort::new(n(2), p(2)),
        Binding::Const(ConstValue::Float(0.5)),
    );
    graph.set_input_binding(InputPort::new(n(3), p(1)), Binding::wire(n(2), p(1)));
    let set = GraphSet::from(graph.clone());
    let scope = set.scope(&library, GraphId::Main);

    let remap = NodeRemap::propose(&saved(BLUR), &saved(SHARPEN));
    let record = graph.replaced(scope, n(2), node(&library, SHARPEN), &remap);
    assert_eq!(record.node().func_id(), Some(SHARPEN));
    let live: Vec<_> = record
        .wiring()
        .bindings()
        .map(|(port, binding)| (port, binding.clone()))
        .collect();
    assert_eq!(
        live,
        [
            (InputPort::new(n(2), p(5)), Binding::wire(n(1), p(1))),
            (InputPort::new(n(3), p(1)), Binding::wire(n(2), p(1))),
        ]
    );
    assert_eq!(
        record.ghost_wiring().binding(InputPort::new(n(2), p(2))),
        Some(&GhostBinding {
            input: Some(SavedPort {
                name: "y".to_owned(),
                data_type: DataType::Float,
            }),
            source: GhostSource::Const(ConstValue::Float(0.5)),
        })
    );

    let remap = NodeRemap::propose(&saved(BLUR), &saved(SOFTEN));
    assert_eq!(
        remap.inputs.pairs(),
        [],
        "soften's 1 is a `String` named `w`"
    );
    let record = graph.replaced(scope, n(2), node(&library, SOFTEN), &remap);
    let ghost_port = |name: &str| {
        record
            .ghost_wiring()
            .bindings_on(n(2))
            .find(|(_, ghost)| ghost.input.as_ref().is_some_and(|saved| saved.name == name))
            .map_or_else(|| panic!("a ghost under {name}"), |(port, _)| port.port)
    };
    assert_ne!(ghost_port("x"), p(1), "x moves off the id soften gives `w`");
    assert_eq!(
        ghost_port("y"),
        p(2),
        "y keeps its id, which soften does not give"
    );
    assert_eq!(record.ghost_wiring().bindings_on(n(2)).count(), 2);
}

/// A load moves a node of a func the library redirects, through a chain of two, to the func it
/// ends at, its wiring through their composed map, and reports it; a ghost node of the old func
/// is moved too and attached, its ghost wire live again. `blur` was redirected to an id that
/// redirects again to `sharpen`, `x` 1 → 3 → 5.
#[test]
fn a_load_moves_a_redirected_funcs_nodes_to_the_func_its_redirects_end_at() {
    let old_blur = FuncId::from_u128(80);
    let between = FuncId::from_u128(81);
    let mut library = with_filters(library());
    let blur = NodeDecl::Func(library.by_id(BLUR).unwrap()).saved();
    library.add_redirect(FuncRedirect {
        from: old_blur,
        to: between,
        remap: NodeRemap {
            inputs: PortMap::new([(p(1), p(3))]),
            outputs: PortMap::new([(p(1), p(1))]),
            events: PortMap::default(),
        },
    });
    library.add_redirect(FuncRedirect {
        from: between,
        to: SHARPEN,
        remap: NodeRemap {
            inputs: PortMap::new([(p(3), p(5))]),
            outputs: PortMap::new([(p(1), p(1))]),
            events: PortMap::default(),
        },
    });
    library.check();
    // The file saved its node of `blur` live, so its manifest lists `blur`; its ghost node was a
    // ghost at the save, so only its own saved declaration knows `blur`'s ports.
    let manifest: Manifest = [(old_blur, blur.clone())].into_iter().collect();

    let mut graph = Graph::default();
    graph.insert(n(1), node(&library, SRC));
    graph.insert(n(2), internals::unresolved_node(old_blur));
    graph.insert(n(3), node(&library, SINK));
    graph.set_input_binding(InputPort::new(n(2), p(1)), Binding::wire(n(1), p(1)));
    graph.set_input_binding(InputPort::new(n(3), p(1)), Binding::wire(n(2), p(1)));
    graph.insert_ghost_node(
        n(4),
        GhostNode {
            node: internals::unresolved_node(old_blur),
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            saved: Some(blur),
        },
    );
    graph.set_input_state(
        InputPort::new(n(4), p(1)),
        InputState::Ghost(GhostBinding::from(Binding::wire(n(1), p(1)))),
    );
    graph.set_input_state(
        InputPort::new(n(4), p(2)),
        InputState::Ghost(GhostBinding::from(Binding::Const(ConstValue::Float(0.5)))),
    );
    let graph_before = graph.clone();
    let mut set = GraphSet::from(graph);
    let report = set.reconcile(&library, &manifest);
    assert_eq!(
        report.of(GraphId::Main)[..2],
        [n(2), n(4)].map(|node_id| Repair::Redirected {
            node_id,
            from: old_blur,
            to: SHARPEN,
        })
    );
    let main = set.main();
    assert_eq!(main.find(n(2)).unwrap().func_id(), Some(SHARPEN));
    assert_eq!(
        main.wiring().binding(InputPort::new(n(2), p(5))),
        Some(&Binding::wire(n(1), p(1)))
    );
    assert_eq!(
        main.wiring().binding(InputPort::new(n(3), p(1))),
        Some(&Binding::wire(n(2), p(1)))
    );
    assert_eq!(main.find(n(4)).unwrap().func_id(), Some(SHARPEN));
    assert_eq!(
        main.wiring().binding(InputPort::new(n(4), p(5))),
        Some(&Binding::wire(n(1), p(1))),
        "the ghost node's ghost wire is live again on its new port"
    );
    assert_eq!(
        main.ghost_wiring()
            .binding(InputPort::new(n(4), p(2)))
            .and_then(|ghost| ghost.input.clone()),
        Some(SavedPort {
            name: "y".to_owned(),
            data_type: DataType::Float,
        }),
        "the redirect names `y`, which it does not map"
    );
    set.validate_with(&library).unwrap();

    // With no manifest the ghost node's own saved declaration still names `y`.
    let mut bare = GraphSet::from(graph_before);
    bare.reconcile(&library, &Manifest::default());
    assert_eq!(
        bare.main()
            .ghost_wiring()
            .binding(InputPort::new(n(4), p(2)))
            .and_then(|ghost| ghost.input.clone())
            .map(|saved| saved.name),
        Some("y".to_owned())
    );
}

/// A redirect from a registered func, and redirects that loop, break the library's contract.
#[test]
#[should_panic(expected = "loop, run past")]
fn redirects_that_loop_are_refused() {
    let (a, b) = (FuncId::from_u128(90), FuncId::from_u128(91));
    let mut library = Library::default();
    for (from, to) in [(a, b), (b, a)] {
        library.add_redirect(FuncRedirect {
            from,
            to,
            remap: NodeRemap::default(),
        });
    }
    library.check();
}
