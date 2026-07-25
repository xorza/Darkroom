//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the compiler through graph flattening and installed as runtime
//! state; it is deliberately not a persistence format. Mutable state is split
//! between the per-run plan/resolver/executor and the cross-run runtime cache.

pub(crate) mod index;
pub(crate) mod pool;

use hashbrown::HashMap;

use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort};
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr, OutputIdx};
use crate::execution::program::pool::{Pool, PoolRange};
use crate::graph::CacheMode;
use crate::library::Library;
use crate::node::definition::{FuncBehavior, FuncId, OutputType};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::node::output_type::{OutputTypeResolver, OutputTypeSource};
use crate::node::special::SpecialNode;
use crate::{DataType, StaticValue};

#[derive(Clone, Debug, Default)]
pub(crate) enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(OutputAddr),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExecutionInput {
    pub required: bool,
    /// Whether a bound value's filesystem referent contributes to this input's digest.
    pub stamps_fs_path: bool,
    pub binding: ExecutionBinding,
}

#[derive(Default, Debug)]
pub(crate) struct ExecutionEvent {
    pub subscribers: Vec<NodeIdx>,
    pub lambda: EventLambda,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutput {
    pub(crate) data_type: DataType,
    pub(crate) pinned: bool,
}

/// A resolved data edge awaiting interning: the input-pool slot to write, and
/// the id-based producer it binds to. Flatten records these instead of dense
/// addresses because a `Bind` can name a node emitted later in the walk —
/// [`ExecutionProgram::intern_bindings`] resolves them once the node set is adopted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingBind {
    pub(crate) input_idx: u32,
    pub(crate) producer: ExecutionOutputPort,
}

/// A resolved event edge awaiting wiring — the [`PendingBind`] counterpart for
/// subscriptions, applied by [`ExecutionProgram::apply_subscriptions`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSubscription {
    pub(crate) event: ExecutionEventPort,
    pub(crate) subscriber: ExecutionNodeId,
}

pub(crate) type InputRange = PoolRange<ExecutionInput>;
pub(crate) type OutputRange = PoolRange<ExecutionOutput>;
pub(crate) type EventRange = PoolRange<ExecutionEvent>;

/// Topology + code for one flat node. Immutable across runs; mutable per-run
/// state lives in `NodeIdx`-aligned columns, and cross-run cache slots are
/// keyed by the node's stable id.
#[derive(Default, Debug)]
pub(crate) struct ExecutionNode {
    pub sink: bool,
    /// The authoring node or one of its composite ancestors is disabled.
    /// Ambient planning excludes it; an explicit node seed overrides it for
    /// that run.
    pub disabled: bool,
    /// Copied from the node's func at flatten. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub behavior: FuncBehavior,

    /// The authoring node's cache mode, copied from
    /// [`CacheMode`](crate::graph::CacheMode) at flatten. Its two bits
    /// ([`caches_in_ram`](crate::graph::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::CacheMode::persists_to_disk)) gate RAM retention
    /// and the disk load/store; disk is honored only when the node has a
    /// content digest (a reproducible cone) and a disk root is configured — see `digest.rs`
    /// and `disk_store.rs`.
    pub cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (flattened from
    /// [`NodeKind::Special`](crate::graph::NodeKind::Special)). The planner
    /// recognizes the kind — a subscribed `RunSinks` promotes a fired event
    /// into a full sinks run — and it marks the node's interface as coming
    /// from the hardcoded spec rather than the library.
    pub special: Option<SpecialNode>,

    pub inputs: InputRange,
    pub outputs: OutputRange,
    pub events: EventRange,

    pub func_id: FuncId,
    /// Copied from `Func`; changing it invalidates this pure node's cache key.
    pub version: u32,

    pub lambda: FuncLambda,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionProgram {
    /// The dense node column — every per-run column and set aligns to it.
    /// Ordered by `ExecutionNodeId` (see [`Self::adopt_nodes`]) so compiled
    /// artifacts and program walks are deterministic.
    pub(crate) e_nodes: NodeColumn<ExecutionNode>,
    /// `NodeIdx` → authoring-derived id, for the host boundary (reports,
    /// seeds, eviction, cache slots).
    pub(crate) e_node_ids: NodeColumn<ExecutionNodeId>,
    /// Id → `NodeIdx`, for resolving host-supplied identities once per use;
    /// nothing per-run iterates or rebuilds it.
    pub(crate) e_node_index: HashMap<ExecutionNodeId, NodeIdx>,
    pub(crate) inputs: Pool<ExecutionInput>,
    pub(crate) events: Pool<ExecutionEvent>,
    /// Each node's resolved declared output types (wildcards followed) and pin bits,
    /// packed in the same index space as the plan's output columns. Resolved once at flatten
    /// by [`Self::resolve_output_types`]
    /// from the func library (which the program doesn't retain), so the compiled
    /// program is self-describing. Read by the digest (an output-signature change
    /// re-keys). An unresolved wildcard port is
    /// `DataType::Any`. Its length is the program's total output count.
    pub(crate) outputs: Pool<ExecutionOutput>,
}

impl std::ops::Index<NodeIdx> for ExecutionProgram {
    type Output = ExecutionNode;

    fn index(&self, index: NodeIdx) -> &ExecutionNode {
        &self.e_nodes[index]
    }
}

impl ExecutionProgram {
    /// Append one node, assigning the next dense index. Panics on a duplicate
    /// id — flattened ids are unique by construction.
    pub(crate) fn push(&mut self, id: ExecutionNodeId, e_node: ExecutionNode) -> NodeIdx {
        debug_assert!(
            u32::try_from(self.e_nodes.len()).is_ok(),
            "program node count must fit in u32"
        );
        let node_idx = NodeIdx(self.e_nodes.len() as u32);
        let previous = self.e_node_index.insert(id, node_idx);
        assert!(previous.is_none(), "flattened node ids must be unique");
        self.e_node_ids.push(id);
        self.e_nodes.push(e_node);
        node_idx
    }

    /// Adopt the flattened node set, assigning dense indices in id order so the
    /// compiled artifact is deterministic regardless of flatten's walk. Drains
    /// `e_nodes` (sorting it in place) so the flattener keeps the allocation for
    /// the next compile.
    pub(crate) fn adopt_nodes(&mut self, e_nodes: &mut Vec<(ExecutionNodeId, ExecutionNode)>) {
        self.e_nodes.clear();
        self.e_node_ids.clear();
        self.e_node_index.clear();
        e_nodes.sort_unstable_by_key(|(id, _)| *id);
        for (id, e_node) in e_nodes.drain(..) {
            self.push(id, e_node);
        }
    }

    /// Intern flatten's id-based binding fixups into dense [`OutputAddr`]s —
    /// the one place producer ids are hashed, once per compile instead of per
    /// run. Every target exists: flatten only records producers it emitted.
    pub(crate) fn intern_bindings(&mut self, binds: &[PendingBind]) {
        for bind in binds {
            let node_idx = *self
                .e_node_index
                .get(&bind.producer.e_node_id)
                .expect("flatten only binds producers it emitted");
            self.inputs[bind.input_idx as usize].binding = ExecutionBinding::Bind(OutputAddr {
                node_idx,
                port_idx: bind.producer.port_idx as u32,
            });
        }
    }

    /// Apply flatten's resolved event edges. Every endpoint exists for the same
    /// reason bind targets do: flatten resolves emitters and subscribers to
    /// nodes it emitted, and drops the edge itself when either side is missing
    /// or the emitter no longer declares the event. So a miss here is a flatten
    /// bug, and panics rather than silently unwiring an event the graph asked for.
    pub(crate) fn apply_subscriptions(&mut self, subs: &[PendingSubscription]) {
        for sub in subs {
            let subscriber_idx = *self
                .e_node_index
                .get(&sub.subscriber)
                .expect("flatten only subscribes nodes it emitted");
            let emitter_idx = *self
                .e_node_index
                .get(&sub.event.e_node_id)
                .expect("flatten only subscribes to emitters it emitted");
            let events = self[emitter_idx].events;
            assert!(
                sub.event.event_idx < events.len as usize,
                "flatten only subscribes to events the emitter declares"
            );
            self.events[events][sub.event.event_idx]
                .subscribers
                .push(subscriber_idx);
        }
    }

    pub(crate) fn output_idx(&self, address: OutputAddr) -> OutputIdx {
        let outputs = self[address.node_idx].outputs;
        debug_assert!(
            address.port_idx < outputs.len,
            "output port is out of range"
        );
        debug_assert!(
            outputs.start.checked_add(address.port_idx).is_some(),
            "output pool index must fit in u32"
        );
        OutputIdx(outputs.start.wrapping_add(address.port_idx))
    }

    /// Fill the output metadata pool by resolving each node's declared output types
    /// (wildcards followed through bindings) from the full `library` — done once at
    /// flatten, where every compiled node's func is guaranteed present
    /// (`validate_for_execution` resolved them). Results are memoized by
    /// [`OutputAddr`]; unresolved and cyclic wildcard ports store `DataType::Any`.
    pub(crate) fn resolve_output_types(&mut self, library: &Library) {
        let e_nodes = &self.e_nodes;
        let inputs = &self.inputs;
        let source = |port: OutputAddr| {
            let e_node = &e_nodes[port.node_idx];
            let func = match e_node.special {
                Some(special) => special.func(),
                None => library.by_id(e_node.func_id).expect(
                    "a compiled node's func is registered in the library \
                     (validated at validate_for_execution)",
                ),
            };
            match &func.outputs[port.port_idx as usize].ty {
                OutputType::Fixed(data_type) => OutputTypeSource::Fixed(data_type.clone()),
                OutputType::Wildcard { mirrors } => {
                    let input = &inputs[e_node.inputs][*mirrors];
                    let declared = &func.inputs[*mirrors].data_type;
                    match &input.binding {
                        ExecutionBinding::Bind(address) => OutputTypeSource::Bind(*address),
                        ExecutionBinding::Const(value) => OutputTypeSource::Const {
                            declared: declared.clone(),
                            value: value.clone(),
                        },
                        ExecutionBinding::None => OutputTypeSource::Unresolved,
                    }
                }
            }
        };
        let mut resolver = OutputTypeResolver::new(self.outputs.len());
        for (node_idx, e_node) in self.e_nodes.iter_indexed() {
            for port_idx in 0..e_node.outputs.len {
                let data_type = resolver.resolve(OutputAddr { node_idx, port_idx }, &source);
                self.outputs[(e_node.outputs.start + port_idx) as usize].data_type = data_type;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::{ExecutionNode, ExecutionProgram};

    /// Test-facing id lookups: tests and engine introspection address nodes by
    /// their stable id; production paths carry `NodeIdx` instead.
    impl ExecutionProgram {
        pub(crate) fn by_id(&self, id: ExecutionNodeId) -> &ExecutionNode {
            &self[self.e_node_index[&id]]
        }

        pub(crate) fn by_id_mut(&mut self, id: ExecutionNodeId) -> &mut ExecutionNode {
            let node_idx = self.e_node_index[&id];
            &mut self.e_nodes[node_idx]
        }
    }
}

#[cfg(test)]
mod tests;
