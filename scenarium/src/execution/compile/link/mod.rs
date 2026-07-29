//! Linking: one [`FlatGraph`] becomes one [`CompiledGraph`].
//!
//! This is where the crate's two identity spaces meet. Flatten works in stable
//! [`ExecutionNodeId`]s because that is all a walk over authored graphs can
//! know; every run afterwards works in dense `NodeIdx`/`OutputIdx` columns
//! because that is what makes a run an array read. Linking is the one pass that
//! crosses over, and it crosses in one direction: it takes the flat graph
//! whole, orders it, and resolves every id it names into an index.
//!
//! Everything downstream of here is final. The program a link produces is
//! immutable for the life of the install — no later pass fills a field in — so
//! each step below produces its part complete rather than reserving space for
//! it.

use hashbrown::HashMap;

use crate::DataType;
use crate::common::column::Column;
use crate::common::pool::Pool;
use crate::data::output_type_resolver::{OutputTypeResolver, OutputTypeSource};
use crate::execution::compile::flat::{
    FlatBinding, FlatEvent, FlatGraph, FlatInput, FlatNode, FlatOutput, PendingSubscription,
};
use crate::execution::compiled::CompiledGraph;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
};
use crate::execution::source_map::{Attribution, Leaf};

/// The dense node set, in the order that *is* the index space: ids, nodes, and
/// the attribution leaves that came with them, all settled by one sort.
#[derive(Debug, Default)]
struct Placed {
    e_node_ids: Column<NodeIdx, ExecutionNodeId>,
    e_node_index: HashMap<ExecutionNodeId, NodeIdx>,
    e_nodes: Column<NodeIdx, ExecutionNode>,
    leaves: Column<NodeIdx, Leaf>,
}

/// Link `flat` into the installable artifact: place its nodes in the dense
/// index space, resolve every id-named reference against that placement, and
/// build the host-facing indices over the result.
///
/// Takes the flat graph by value, and is the only production path that builds
/// a `CompiledGraph`. Both matter: linking consumes exactly one flatten's
/// output, and the indices are not optional state that could be attached
/// afterwards.
///
/// No `Library` — flatten copied everything the program needs out of it, which
/// is what lets the artifact be self-contained.
pub(super) fn link(flat: FlatGraph) -> CompiledGraph {
    let FlatGraph {
        nodes,
        inputs,
        outputs,
        events,
        subscriptions,
        scopes,
        exposed,
    } = flat;

    let placed = Placed::order(nodes);
    let inputs = intern_bindings(inputs, &placed.e_node_index);
    let outputs = resolve_output_types(outputs, &placed.e_nodes, &inputs);
    let events = wire_subscriptions(events, &subscriptions, &placed);

    let Placed {
        e_node_ids,
        e_node_index,
        e_nodes,
        leaves,
    } = placed;
    let program = Program {
        e_nodes,
        e_node_ids,
        e_node_index,
        inputs,
        outputs,
        events,
    };

    CompiledGraph::indexed(program, Attribution::new(scopes, leaves), exposed)
}

impl Placed {
    /// Sort the walk's nodes by id and split them into the program's columns.
    ///
    /// Id order rather than emit order, so the compiled artifact is
    /// deterministic however the walk happened to reach the nodes — and one
    /// sort settles both the program's node vector and the attribution column
    /// beside it, since the leaves travel inside the nodes being sorted.
    fn order(mut nodes: Vec<FlatNode>) -> Self {
        nodes.sort_unstable_by_key(|node| node.id);
        assert!(
            u32::try_from(nodes.len()).is_ok(),
            "program node count must fit in u32"
        );

        let mut placed = Placed {
            e_node_index: HashMap::with_capacity(nodes.len()),
            ..Default::default()
        };
        for (position, node) in nodes.into_iter().enumerate() {
            let previous = placed
                .e_node_index
                .insert(node.id, NodeIdx(position as u32));
            assert!(previous.is_none(), "flattened node ids must be unique");
            placed.e_node_ids.push(node.id);
            placed.leaves.push(node.leaf);
            placed.e_nodes.push(ExecutionNode {
                sink: node.sink,
                disabled: node.disabled,
                behavior: node.behavior,
                cache: node.cache,
                special: node.special,
                inputs: node.inputs.retype(),
                outputs: node.outputs.retype(),
                events: node.events.retype(),
                func_id: node.func_id,
                version: node.version,
                lambda: node.lambda,
            });
        }
        placed
    }
}

/// Rewrite each id-named producer into a dense [`OutputAddr`] — the one place a
/// producer id is ever hashed, once per compile instead of once per run.
///
/// Every target exists: flatten only binds producers it emitted, so a miss is a
/// flatten bug rather than bad input, and degrading it to an unbound input
/// would lose an edge the graph asked for.
fn intern_bindings(
    flat: Pool<FlatInput>,
    e_node_index: &HashMap<ExecutionNodeId, NodeIdx>,
) -> Pool<ExecutionInput> {
    let mut inputs = Pool::default();
    inputs.append(flat.into_values().map(|input| ExecutionInput {
        required: input.required,
        stamps_fs_path: input.stamps_fs_path,
        binding: match input.binding {
            FlatBinding::None => ExecutionBinding::None,
            FlatBinding::Const(value) => ExecutionBinding::Const(value),
            FlatBinding::Bind(producer) => {
                let node_idx = *e_node_index
                    .get(&producer.e_node_id)
                    .expect("flatten only binds producers it emitted");
                ExecutionBinding::Bind(OutputAddr {
                    node_idx,
                    port_idx: producer.port_idx as u32,
                })
            }
        },
    }));
    inputs
}

/// Turn each output's declaration into its effective type, following wildcards
/// through the bindings just interned. Memoized per output, so a chain of
/// reroutes is walked once however many ports read it.
///
/// A wildcard whose mirrored input is unbound, or whose chain closes a cycle,
/// resolves to `Any` — the same drift tolerance the editor shows.
fn resolve_output_types(
    flat: Pool<FlatOutput>,
    e_nodes: &Column<NodeIdx, ExecutionNode>,
    inputs: &Pool<ExecutionInput>,
) -> Pool<ExecutionOutput> {
    let source = |port: OutputAddr| {
        let e_node = &e_nodes[port.node_idx];
        match &flat[(e_node.outputs.start + port.port_idx) as usize] {
            FlatOutput::Fixed(data_type) => OutputTypeSource::Fixed(data_type.clone()),
            FlatOutput::Wildcard {
                mirrors,
                mirrored_declared,
            } => match &inputs[e_node.inputs][*mirrors as usize].binding {
                ExecutionBinding::Bind(address) => OutputTypeSource::Bind(*address),
                ExecutionBinding::Const(value) => OutputTypeSource::Const {
                    declared: mirrored_declared.clone(),
                    value: value.clone(),
                },
                ExecutionBinding::None => OutputTypeSource::Unresolved,
            },
        }
    };

    // Resolution is keyed by node and port, while the pool is in emit order, so
    // the answers land by pool index rather than in the order they are found.
    // Local scratch: every slot is written before it becomes a pool.
    let mut data_types = vec![DataType::Any; flat.len()];
    let mut resolver = OutputTypeResolver::new();
    for (node_idx, e_node) in e_nodes.iter_indexed() {
        for port_idx in 0..e_node.outputs.len {
            data_types[(e_node.outputs.start + port_idx) as usize] =
                resolver.resolve(OutputAddr { node_idx, port_idx }, &source);
        }
    }

    let mut outputs = Pool::default();
    outputs.append(
        data_types
            .into_iter()
            .map(|data_type| ExecutionOutput { data_type }),
    );
    outputs
}

/// Give each event the subscribers the walk resolved for it.
///
/// Both endpoints exist for the same reason a bind target does: flatten
/// resolves emitters and subscribers to nodes it emitted, and drops the edge
/// itself when either side is missing or the emitter no longer declares the
/// event. So a miss here is a flatten bug, and panics rather than silently
/// unwiring an event the graph asked for.
fn wire_subscriptions(
    flat: Pool<FlatEvent>,
    subscriptions: &[PendingSubscription],
    placed: &Placed,
) -> Pool<ExecutionEvent> {
    // Grouped before the events are built, so each one is whole when it enters
    // the pool — an empty subscriber list then means "nothing subscribes"
    // rather than "not wired yet".
    let mut subscribers: Vec<Vec<NodeIdx>> = vec![Vec::new(); flat.len()];
    for subscription in subscriptions {
        let subscriber_idx = *placed
            .e_node_index
            .get(&subscription.subscriber)
            .expect("flatten only subscribes nodes it emitted");
        let emitter_idx = *placed
            .e_node_index
            .get(&subscription.event.e_node_id)
            .expect("flatten only subscribes to emitters it emitted");
        let events = placed.e_nodes[emitter_idx].events;
        assert!(
            subscription.event.event_idx < events.len as usize,
            "flatten only subscribes to events the emitter declares"
        );
        subscribers[events.start as usize + subscription.event.event_idx].push(subscriber_idx);
    }

    let mut events = Pool::default();
    events.append(
        flat.into_values()
            .zip(subscribers)
            .map(|(event, subscribers)| ExecutionEvent {
                lambda: event.lambda,
                subscribers,
            }),
    );
    events
}

#[cfg(test)]
mod tests;
