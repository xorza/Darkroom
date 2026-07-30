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

use crate::common::column::Column;
use crate::execution::compiled::CompiledGraph;
use crate::execution::compiled::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput,
};
use crate::execution::flatten::flat::{FlatBinding, FlatGraph, FlatInput, PendingSubscription};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::{EventIdx, InputIdx, NodeIdx, OutputAddr};
use crate::graph::func::Func;
use crate::graph::func::event::EventLambda;
use crate::library::Library;

/// The link stage, and the buffers it would otherwise allocate per compile.
///
/// Held by the [`Compiler`](crate::execution::compile::Compiler) beside the
/// [`Flattener`](crate::execution::flatten::Flattener), for the same
/// reason and on the same terms: nothing here survives into the artifact, so
/// keeping it costs one allocation for the process rather than one per edit.
/// The columns that become the artifact are built fresh in [`Self::link`].
#[derive(Debug, Default)]
pub(super) struct Linker {
    /// Positions into the flat node list, sorted by id — the order that becomes
    /// the dense index space. An index sort rather than sorting the nodes, since
    /// the flat graph is read-only here.
    order: Vec<u32>,
    /// Each event's subscribers, grouped before the events are built. Only the
    /// outer vector is reused: the inner ones move into the events.
    subscribers: Vec<Vec<NodeIdx>>,
}

impl Linker {
    /// Link `flat` into `out`: place its nodes in the dense index space, resolve
    /// every id-named reference against that placement.
    ///
    /// The only production path that fills a `CompiledGraph`, which is why the
    /// artifact's program is written from here rather than handed to it whole:
    /// there is no half-built `CompiledGraph` for anything else to observe.
    ///
    /// **Reads the flat graph, writes the artifact.** `flat` is shared, so
    /// linking copies what it needs (the lambdas it copies are `Arc`s) and leaves
    /// the graph exactly as flatten left it: the same one links twice to the same
    /// answer, and nothing has to know which buffers this pass would otherwise
    /// have eaten.
    ///
    /// The columns built below are *not* on [`Linker`] with the scratch,
    /// because they do not survive the call as buffers — they become the
    /// program's. Only what stays behind belongs on the linker.
    ///
    /// `library` is read for the one port kind the walk reserved rather than
    /// emitted: an event's lambda. It does not follow the artifact — what lands
    /// in the program is an `Arc`'d lambda — so the artifact stays
    /// self-contained and the library is a compile-time input like the graph
    /// beside it.
    pub(super) fn link(&mut self, flat: &FlatGraph, library: &Library, out: &mut CompiledGraph) {
        // Id order rather than emit order, so the artifact is deterministic
        // however the walk happened to reach the nodes — `Graph::iter` is a
        // `HashMap` walk, so emit order is not. `order` holds positions rather
        // than sorting the nodes, which are the caller's and read-only here.
        assert!(
            u32::try_from(flat.e_nodes.len()).is_ok(),
            "program node count must fit in u32"
        );

        self.order.clear();
        self.order.extend(0..flat.e_nodes.len() as u32);
        self.order
            .sort_unstable_by_key(|&at| flat.e_node_ids[at as usize]);

        let mut e_node_ids = Column::default();
        let mut e_node_index = HashMap::with_capacity(flat.e_nodes.len());
        let mut e_nodes = Column::default();
        for (position, &at) in self.order.iter().enumerate() {
            let at = at as usize;
            let e_node_id = flat.e_node_ids[at];
            let previous = e_node_index.insert(e_node_id, NodeIdx(position as u32));
            assert!(previous.is_none(), "flattened node ids must be unique");
            e_node_ids.push(e_node_id);
            // Placing a node is a copy, not a translation: the walk built it
            // whole, and the port pools are rebuilt slot for slot below, so the
            // runs it already owns are the runs it keeps. Copying rather than
            // moving is what leaves the flat graph readable — the lambda is an
            // `Arc`, so it costs a refcount bump.
            e_nodes.push(flat.e_nodes[at].clone());
        }

        let inputs = intern_bindings(&flat.inputs, &e_node_index);
        // Slot for slot, like the input column: the walk resolved every output's
        // effective type against the authoring graph, and placing a node keeps
        // the run it already owned.
        let mut outputs = Column::default();
        outputs.append(flat.outputs.iter().map(|data_type| ExecutionOutput {
            data_type: data_type.clone(),
        }));
        let events = self.wire_subscriptions(
            library,
            flat.events,
            &flat.subscriptions,
            &e_nodes,
            &e_node_index,
        );

        *out = CompiledGraph {
            e_nodes,
            e_node_ids,
            e_node_index,
            inputs,
            outputs,
            events,
        };
    }

    /// Give each event the subscribers the walk resolved for it.
    ///
    /// Both endpoints exist for the same reason a bind target does: flatten
    /// resolves emitters and subscribers to nodes it emitted, and drops the edge
    /// itself when either side is missing or the emitter no longer declares the
    /// event. So a miss here is a flatten bug, and panics rather than silently
    /// unwiring an event the graph asked for.
    fn wire_subscriptions(
        &mut self,
        library: &Library,
        event_count: u32,
        subscriptions: &[PendingSubscription],
        e_nodes: &Column<NodeIdx, ExecutionNode>,
        e_node_index: &HashMap<ExecutionNodeId, NodeIdx>,
    ) -> Column<EventIdx, ExecutionEvent> {
        // Grouped before the events are built, so each one is whole when it enters
        // the pool — an empty subscriber list then means "nothing subscribes"
        // rather than "not wired yet".
        self.subscribers.clear();
        self.subscribers.resize_with(event_count as usize, Vec::new);
        for subscription in subscriptions {
            let subscriber_idx = *e_node_index
                .get(&subscription.subscriber)
                .expect("flatten only subscribes nodes it emitted");
            let emitter_idx = *e_node_index
                .get(&subscription.event.e_node_id)
                .expect("flatten only subscribes to emitters it emitted");
            let events = e_nodes[emitter_idx].events;
            assert!(
                subscription.event.event_idx < events.len as usize,
                "flatten only subscribes to events the emitter declares"
            );
            self.subscribers[events.start as usize + subscription.event.event_idx]
                .push(subscriber_idx);
        }

        // The lambdas land by pool index, as the resolved output types do: the
        // pool is in emit order while the nodes that own its runs are placed,
        // so each node writes into the run it reserved. Every slot is covered —
        // the runs partition the pool by construction.
        let mut events = Column::default();
        events.append(
            self.subscribers
                .drain(..)
                .map(|subscribers| ExecutionEvent {
                    subscribers,
                    lambda: EventLambda::default(),
                }),
        );
        for (_, e_node) in e_nodes.iter_indexed() {
            // Looked up per node that *has* events, so a node declaring none
            // costs no hash — and a fixture standing up bare nodes needs no
            // library entry for them.
            if e_node.events.len == 0 {
                continue;
            }
            let func = declaring_func(library, e_node);
            for port_idx in 0..e_node.events.len {
                // An `Arc`, so copying rather than moving it out of the library
                // is a refcount bump.
                events[e_node.events.nth(port_idx)].lambda =
                    func.events[port_idx as usize].event_lambda.clone();
            }
        }
        events
    }
}

/// The declaration behind one placed node: its library func, or the hardcoded
/// spec when it is a built-in.
///
/// Every lookup succeeds — `validate_with` resolved each authored node's func
/// before the walk ran, and the walk copied that same `func_id` onto the node —
/// so a miss is a compile bug rather than drift to absorb.
fn declaring_func<'a>(library: &'a Library, e_node: &ExecutionNode) -> &'a Func {
    match e_node.special {
        Some(special) => special.func(),
        None => library
            .by_id(e_node.func_id)
            .expect("a compiled node's func is registered in the library"),
    }
}

/// Rewrite each id-named producer into a dense [`OutputAddr`] — the one place a
/// producer id is ever hashed, once per compile instead of once per run.
///
/// Every target exists: flatten only binds producers it emitted, so a miss is a
/// flatten bug rather than bad input, and degrading it to an unbound input
/// would lose an edge the graph asked for.
///
/// A free fn rather than a [`Linker`] method: it keeps no scratch, taking the
/// stage pool it drains and the placement it resolves against.
fn intern_bindings(
    flat: &Column<InputIdx, FlatInput>,
    e_node_index: &HashMap<ExecutionNodeId, NodeIdx>,
) -> Column<InputIdx, ExecutionInput> {
    let mut inputs = Column::default();
    inputs.append(flat.iter().map(|input| ExecutionInput {
        required: input.required,
        stamps_fs_path: input.stamps_fs_path,
        binding: match &input.binding {
            FlatBinding::None => ExecutionBinding::None,
            // The one payload a link copies rather than moves — the price of the
            // flat graph staying readable afterwards.
            FlatBinding::Const(value) => ExecutionBinding::Const(value.clone()),
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

#[cfg(test)]
mod tests;
