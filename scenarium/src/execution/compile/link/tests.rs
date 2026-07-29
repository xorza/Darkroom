use super::*;
use crate::StaticValue;
use crate::common::column::Idx;
use crate::common::pool::PoolRange;
use crate::execution::compile::flat::internals::FlatGraphBuilder;
use crate::execution::identity::{ExecutionEventPort, ExecutionOutputPort};
use crate::graph::func::event::EventLambda;
use crate::graph::identity::NodeId;

fn e_node_id(id: u128) -> ExecutionNodeId {
    ExecutionNodeId::from_u128(id)
}

/// Four bare nodes in emit order 3, 1, 2, 4 — ids the walk would have
/// reached in that order, which is not the order they will be placed in.
fn emitted(ids: [u128; 4]) -> FlatGraph {
    let mut builder = FlatGraphBuilder::default();
    for id in ids {
        builder.insert_leaf(e_node_id(id), [], NodeId::from_u128(id));
    }
    builder.build()
}

fn bound(producer: u128, port_idx: usize) -> FlatInput {
    FlatInput {
        required: true,
        stamps_fs_path: false,
        binding: FlatBinding::Bind(ExecutionOutputPort {
            e_node_id: e_node_id(producer),
            port_idx,
        }),
    }
}

/// Placement is by id, the pools stay in emit order, and every id-named
/// reference resolves against the placement rather than against the order
/// it was written in. The attribution leaves ride through the same sort, so
/// each node still answers for the authored node it came from.
#[test]
fn links_ids_to_dense_indices_over_emit_ordered_pools() {
    let mut flat = emitted([3, 1, 2, 4]);
    // Three producers with two output ports each, appended as emitted.
    for position in 0..3 {
        flat.nodes[position].outputs = flat.outputs.append([
            FlatOutput::Fixed(DataType::Int),
            FlatOutput::Fixed(DataType::Int),
        ]);
    }
    flat.nodes[3].inputs = flat.inputs.append([bound(1, 1), bound(3, 0)]);

    let compiled = link(flat);
    let program = &compiled.program;

    assert_eq!(
        program.e_node_ids.iter().copied().collect::<Vec<_>>(),
        (1..=4).map(e_node_id).collect::<Vec<_>>(),
        "ids 1..=4 take indices 0..=3 despite id 3 being emitted first"
    );
    for id in 1..=4 {
        assert_eq!(
            compiled
                .attribution(e_node_id(id))
                .unwrap()
                .collect::<Vec<_>>(),
            vec![NodeId::from_u128(id)],
            "each leaf followed its node through the sort"
        );
    }

    let consumer = program.by_id(e_node_id(4));
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

    // Emit order 3, 1, 2 gave out output-pool starts 0, 2, 4. So id 1 sits
    // at index 0 while owning slots 2..4, and id 3 at index 2 owning 0..2.
    assert_eq!(program.output_idx(addresses[0]).idx(), 3);
    assert_eq!(program.output_idx(addresses[1]).idx(), 0);
}

/// A wildcard resolves through the binding just interned, and a `Const`
/// mirror resolves against the declared type flatten carried over — the
/// two reasons linking needs no library.
#[test]
fn resolves_wildcard_outputs_through_interned_bindings() {
    let mut flat = emitted([3, 1, 2, 4]);
    // Node 1 produces a `String`; node 4 mirrors it through a wildcard.
    flat.nodes[1].outputs = flat.outputs.append([FlatOutput::Fixed(DataType::String)]);
    flat.nodes[3].inputs = flat.inputs.append([
        bound(1, 0),
        FlatInput {
            required: false,
            stamps_fs_path: false,
            binding: FlatBinding::Const(StaticValue::Int(7)),
        },
    ]);
    flat.nodes[3].outputs = flat.outputs.append([
        FlatOutput::Wildcard {
            mirrors: 0,
            mirrored_declared: DataType::Any,
        },
        FlatOutput::Wildcard {
            mirrors: 1,
            mirrored_declared: DataType::Float,
        },
    ]);

    let program = &link(flat).program;
    let consumer = program.by_id(e_node_id(4));
    let types: Vec<_> = program.outputs[consumer.outputs]
        .iter()
        .map(|output| output.data_type.clone())
        .collect();
    assert_eq!(
        types,
        vec![DataType::String, DataType::Float],
        "the bound mirror follows its producer; the const mirror takes the \
         declared type of the input it mirrors"
    );
}

/// An endpoint the walk never emitted is a flatten bug, not drift to
/// absorb: linking panics rather than dropping an edge the graph asked for.
#[test]
fn panics_on_edges_naming_nodes_the_walk_never_emitted() {
    let mut flat = emitted([3, 1, 2, 4]);
    flat.nodes[3].inputs = flat.inputs.append([bound(9, 0)]);
    let bind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || link(flat)));
    assert!(
        bind.is_err(),
        "binding a producer the program never adopted must panic"
    );

    let mut flat = emitted([3, 1, 2, 4]);
    flat.nodes[0].events = flat.events.append([FlatEvent {
        lambda: EventLambda::default(),
    }]);
    flat.subscriptions.push(PendingSubscription {
        event: ExecutionEventPort {
            e_node_id: e_node_id(3),
            event_idx: 0,
        },
        subscriber: e_node_id(9),
    });
    let subscription = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || link(flat)));
    assert!(
        subscription.is_err(),
        "subscribing a node the program never adopted must panic"
    );
}

/// The event's subscriber list is built before the event is, so an empty
/// one means "nothing subscribes" rather than "not wired yet".
#[test]
fn wires_each_event_with_the_subscribers_resolved_for_it() {
    let mut flat = emitted([3, 1, 2, 4]);
    flat.nodes[0].events = flat.events.append([
        FlatEvent {
            lambda: EventLambda::default(),
        },
        FlatEvent {
            lambda: EventLambda::default(),
        },
    ]);
    flat.subscriptions.push(PendingSubscription {
        event: ExecutionEventPort {
            e_node_id: e_node_id(3),
            event_idx: 1,
        },
        subscriber: e_node_id(4),
    });

    let program = &link(flat).program;
    let emitter = program.by_id(e_node_id(3));
    let subscribers: Vec<_> = program.events[emitter.events]
        .iter()
        .map(|event| event.subscribers.clone())
        .collect();
    assert_eq!(
        subscribers,
        vec![vec![], vec![program.e_node_index[&e_node_id(4)]]],
        "only the subscribed port carries a subscriber"
    );
}

/// A pool range means the same run of ports in the program's pool as in the
/// flat one, which is what [`PoolRange::retype`] stands on.
#[test]
fn port_ranges_survive_the_rebuild() {
    let mut flat = emitted([3, 1, 2, 4]);
    flat.nodes[1].inputs = flat.inputs.append([bound(3, 0), bound(2, 0)]);
    flat.nodes[0].outputs = flat.outputs.append([FlatOutput::Fixed(DataType::Bool)]);
    flat.nodes[2].outputs = flat.outputs.append([FlatOutput::Fixed(DataType::Bool)]);
    let expected: PoolRange<ExecutionInput> = flat.nodes[1].inputs.retype();

    let program = &link(flat).program;
    let placed = program.by_id(e_node_id(1)).inputs;
    assert_eq!((placed.start, placed.len), (expected.start, expected.len));
    assert_eq!(program.inputs[placed].len(), 2);
}
