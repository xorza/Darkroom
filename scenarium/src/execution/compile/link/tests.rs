use super::*;
use crate::DataType;
use crate::common::column::Idx;
use crate::execution::compiled::CompiledGraph;
use crate::execution::lower::lowered_graph::internals::LoweredGraphBuilder;
use crate::graph::func::event::EventLambda;
use crate::graph::func::{FuncInput, FuncOutput};
use crate::graph::identity::{EventPort, FuncId, NodeId, OutputPort};
use crate::testing;

fn node_id(id: u128) -> NodeId {
    NodeId::from_u128(id)
}

/// Node `n` carries func id `n`, so the declaration linking reads and the node
/// that owns it are named by the same number and cannot drift apart in a
/// fixture.
fn declaring(id: u128) -> Func {
    testing::with_stub_lambda(Func::new(FuncId::from_u128(id), format!("f{id}")))
}

fn library(funcs: impl IntoIterator<Item = Func>) -> Library {
    let mut library = Library::default();
    for func in funcs {
        library.add(func);
    }
    library
}

/// Four bare nodes in emit order 3, 1, 2, 4 — ids the walk would have
/// reached in that order, which is not the order they will be placed in.
fn emitted(ids: [u128; 4]) -> LoweredGraph {
    let mut builder = LoweredGraphBuilder::default();
    for id in ids {
        builder.insert_node(node_id(id));
    }
    let mut lowered = builder.build();
    for (position, id) in ids.iter().enumerate() {
        lowered.e_nodes[position].func_id = FuncId::from_u128(*id);
    }
    lowered
}

fn bound(producer: u128, port_idx: usize) -> LoweredInput {
    LoweredInput {
        required: true,
        stamps_fs_path: false,
        binding: LoweredBinding::Bind(OutputPort {
            node_id: node_id(producer),
            port_idx,
        }),
    }
}

/// Give the node at `position` the output types lowering would have resolved
/// for it, appended in emit order — the one part of a lowered graph a fixture
/// built by hand still has to state.
fn push_outputs(
    lowered: &mut LoweredGraph,
    position: usize,
    data_types: impl IntoIterator<Item = DataType>,
) {
    let span = lowered.outputs.append(data_types);
    lowered.e_nodes[position].outputs = span;
}

/// Placement is by id, the pools stay in emit order, and every id-named
/// reference resolves against the placement rather than against the order
/// it was written in. The attribution leaves ride through the same sort, so
/// each node still answers for the authored node it came from.
#[test]
fn links_ids_to_dense_indices_over_emit_ordered_pools() {
    let mut lowered = emitted([3, 1, 2, 4]);
    // Three producers with two output ports each, appended as emitted.
    for position in 0..3 {
        push_outputs(&mut lowered, position, [DataType::Int, DataType::Int]);
    }
    lowered.e_nodes[3].inputs = lowered.inputs.append([bound(1, 1), bound(3, 0)]);
    let library = library([
        declaring(3),
        declaring(1),
        declaring(2),
        declaring(4)
            .input(FuncInput::required("x", DataType::Int))
            .input(FuncInput::required("y", DataType::Int)),
    ]);

    let mut compiled = CompiledGraph::default();
    Linker::default().link(&lowered, &library, &mut compiled);

    // Linking reads the lowered graph and leaves it alone, so the same one links
    // again to the same answer — the property that makes `lowered` shared here.
    let mut relinked = CompiledGraph::default();
    Linker::default().link(&lowered, &library, &mut relinked);
    assert_eq!(
        relinked.node_ids.iter().collect::<Vec<_>>(),
        compiled.node_ids.iter().collect::<Vec<_>>(),
        "a second link over the same lowered graph places the same nodes"
    );
    let program = &compiled;

    assert_eq!(
        program.node_ids.iter().copied().collect::<Vec<_>>(),
        (1..=4).map(node_id).collect::<Vec<_>>(),
        "ids 1..=4 take indices 0..=3 despite id 3 being emitted first"
    );
    for id in 1..=4 {
        assert!(
            compiled.contains(node_id(id)),
            "each node followed its id through the sort"
        );
    }

    let consumer = program.by_id(node_id(4));
    let addresses: Vec<_> = program.inputs[consumer.inputs]
        .iter()
        .map(|input| match input.binding {
            ExecutionBinding::Bind(addr) => addr,
            ref other => panic!("expected an interned bind, got {other:?}"),
        })
        .collect();
    assert_eq!(
        addresses,
        [
            OutputAddr {
                node_idx: NodeIdx(0),
                port_idx: 1
            },
            OutputAddr {
                node_idx: NodeIdx(2),
                port_idx: 0
            },
        ]
    );

    // Emit order 3, 1, 2 took output-pool starts 0, 2, 4. So id 1 sits at
    // index 0 while owning slots 2..4, and id 3 at index 2 owning 0..2.
    assert_eq!(program.output_idx(addresses[0]).idx(), 3);
    assert_eq!(program.output_idx(addresses[1]).idx(), 0);
}

/// Linking copies the walk's resolved output types slot for slot rather than
/// re-deriving them, so what the lowered graph says a port produces is what the
/// program says — including a type no declaration in the library mentions.
#[test]
fn copies_the_walks_resolved_output_types() {
    let mut lowered = emitted([3, 1, 2, 4]);
    push_outputs(&mut lowered, 1, [DataType::String]);
    push_outputs(&mut lowered, 3, [DataType::String, DataType::Float]);
    // Declares a *wildcard* pair: were linking still resolving these itself,
    // both would come back `Any` from the unbound mirrors below.
    let library = library([
        declaring(1).output(FuncOutput::new("s", DataType::String)),
        declaring(4)
            .input(FuncInput::required("mirrored", DataType::Any))
            .input(FuncInput::optional("scalar", DataType::Float))
            .wildcard_output("from_bind", 0)
            .wildcard_output("from_const", 1),
    ]);

    let mut compiled = CompiledGraph::default();
    Linker::default().link(&lowered, &library, &mut compiled);
    let program = &compiled;
    let consumer = program.by_id(node_id(4));
    assert_eq!(
        program.outputs[consumer.outputs],
        [DataType::String, DataType::Float]
    );
}

/// An endpoint the walk never emitted is a lowering bug, not drift to
/// absorb: linking panics rather than dropping an edge the graph asked for.
#[test]
fn panics_on_edges_naming_nodes_the_walk_never_emitted() {
    let mut lowered = emitted([3, 1, 2, 4]);
    lowered.e_nodes[3].inputs = lowered.inputs.append([bound(9, 0)]);
    let empty = Library::default();
    let bind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Linker::default().link(&lowered, &empty, &mut CompiledGraph::default());
    }));
    assert!(
        bind.is_err(),
        "binding a producer the program never adopted must panic"
    );

    let mut lowered = emitted([3, 1, 2, 4]);
    lowered.e_nodes[0].events = lowered.reserve_events(1);
    lowered.subscriptions.push(PendingSubscription {
        event: EventPort {
            node_id: node_id(3),
            event_idx: 0,
        },
        subscriber: node_id(9),
    });
    let library = library([declaring(3).event("fired", EventLambda::default())]);
    let subscription = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Linker::default().link(&lowered, &library, &mut CompiledGraph::default());
    }));
    assert!(
        subscription.is_err(),
        "subscribing a node the program never adopted must panic"
    );
}

/// The event's subscriber list is built before the event is, so an empty
/// one means "nothing subscribes" rather than "not wired yet".
#[test]
fn wires_each_event_with_the_subscribers_resolved_for_it() {
    let mut lowered = emitted([3, 1, 2, 4]);
    lowered.e_nodes[0].events = lowered.reserve_events(2);
    lowered.subscriptions.push(PendingSubscription {
        event: EventPort {
            node_id: node_id(3),
            event_idx: 1,
        },
        subscriber: node_id(4),
    });
    let library = library([declaring(3)
        .event("quiet", EventLambda::default())
        .event("subscribed", EventLambda::default())]);

    let mut compiled = CompiledGraph::default();
    Linker::default().link(&lowered, &library, &mut compiled);
    let program = &compiled;
    let emitter = program.by_id(node_id(3));
    let subscribers: Vec<_> = program.events[emitter.events]
        .iter()
        .map(|event| event.subscribers.clone())
        .collect();
    assert_eq!(
        subscribers,
        vec![vec![], vec![program.node_index[&node_id(4)]]],
        "only the subscribed port carries a subscriber"
    );
}

/// One port range means the same run of ports in the program's pool as in the
/// lowered one — the whole reason a single range type spans both stages. Both port
/// pools are rebuilt slot for slot, so a placed node owns the run its lowered self
/// claimed.
#[test]
fn port_ranges_survive_the_rebuild() {
    let mut lowered = emitted([3, 1, 2, 4]);
    lowered.e_nodes[1].inputs = lowered.inputs.append([bound(3, 0), bound(2, 0)]);
    push_outputs(&mut lowered, 0, [DataType::Bool]);
    push_outputs(&mut lowered, 2, [DataType::Bool]);
    let expected = lowered.e_nodes[1].inputs;
    let expected_outputs = lowered.e_nodes[2].outputs;
    let library = library([
        declaring(3),
        declaring(2),
        declaring(1)
            .input(FuncInput::required("x", DataType::Bool))
            .input(FuncInput::required("y", DataType::Bool)),
    ]);

    let mut compiled = CompiledGraph::default();
    Linker::default().link(&lowered, &library, &mut compiled);
    let program = &compiled;
    let placed = program.by_id(node_id(1)).inputs;
    assert_eq!(placed, expected);
    assert_eq!(program.inputs[placed].len(), 2);
    // The output run is the run the placed node owns, carrying the type the
    // walk put in it.
    assert_eq!(program.by_id(node_id(2)).outputs, expected_outputs);
    assert_eq!(program.outputs[expected_outputs][0], DataType::Bool);
}
