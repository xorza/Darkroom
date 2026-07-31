//! The compile artifact: the flattened graph — topology + code — plus every
//! question a host asks of one.
//!
//! Built once by the compiler's link stage — there are no mutators here, and
//! no pass that fills a field in afterwards — then installed as runtime state;
//! it is deliberately not a persistence format. Mutable state is split between
//! the per-run schedule/executor and the cross-run runtime cache.
//!
//! Self-contained: everything a run needs was copied out of the [`Library`](crate::library::Library)
//! at flatten, so nothing here refers to one.
//!
//! A graph is flat, so an authored node becomes exactly one execution node and
//! the two identity spaces differ only in type. That collapses what used to be
//! a whole side structure — the map from an authored node to the execution
//! nodes it dissolved into — to a pair of conversions on [`ExecutionNodeId`]
//! and one lookup in the artifact's own id index.

pub(crate) mod consumers;

use crate::graph::identity::FuncId;
use hashbrown::HashMap;

use crate::common::column::{Column, Span};
use crate::common::set::IdxSet;
use crate::execution::compiled::consumers::Consumers;
use crate::execution::error::ExecutionIdentityError;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::{EventIdx, InputIdx, NodeIdx, OutputAddr, OutputIdx};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::NodeId;
use crate::graph::node::CacheMode;
use crate::graph::node::special::SpecialNode;
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
}

/// Topology + code for one flat node. Immutable across runs; mutable per-run
/// state lives in `NodeIdx`-aligned columns, and cross-run cache slots are
/// keyed by the node's stable id.
///
/// Built whole by the flatten walk, in emit order, and copied into the dense
/// index space by linking's id sort — which is what `Clone` is for. Everything
/// here is `Copy` but the lambda, and that is an `Arc`, so a copy is a refcount
/// bump and the flat graph stays readable behind it.
#[derive(Default, Debug, Clone)]
pub(crate) struct ExecutionNode {
    pub sink: bool,
    /// The authoring node is disabled. Ambient planning excludes it; an
    /// explicit node seed overrides it for that run.
    pub disabled: bool,
    /// Copied from the node's func at flatten. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub behavior: FuncBehavior,

    /// The authoring node's cache mode, copied from
    /// [`CacheMode`] at flatten. Its two bits
    /// ([`caches_in_ram`](crate::graph::node::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::node::CacheMode::persists_to_disk)) gate RAM retention
    /// and the disk load/store; disk is honored only when the node has a
    /// content digest (a reproducible cone) and a disk root is configured — see `digest.rs`
    /// and `disk_store.rs`.
    pub cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (flattened from
    /// [`NodeKind::Special`](crate::graph::node::NodeKind::Special)). The planner
    /// recognizes the kind — a subscribed `RunSinks` promotes a fired event
    /// into a full sinks run — and it marks the node's interface as coming
    /// from the hardcoded spec rather than the library.
    pub special: Option<SpecialNode>,

    pub inputs: Span<InputIdx>,
    pub outputs: Span<OutputIdx>,
    pub events: Span<EventIdx>,

    pub func_id: FuncId,

    pub lambda: FuncLambda,
}

/// The compile artifact: the flattened, immutable program — lambdas, resolved
/// output types, and bound-path stamping metadata. Self-contained: executing it
/// needs neither the authoring graph nor the library.
///
/// [`Default`] is the empty artifact, which is what a caller hands the
/// compiler's link stage to fill and what an engine holds before anything is
/// installed.
#[derive(Debug, Default)]
pub struct CompiledGraph {
    /// The dense node column — every per-run column and set aligns to it.
    /// Ordered by `ExecutionNodeId` during linking so compiled artifacts and
    /// program walks are deterministic.
    pub(crate) e_nodes: Column<NodeIdx, ExecutionNode>,
    /// `NodeIdx` → authoring-derived id, for the host boundary (reports,
    /// seeds, eviction, cache slots).
    pub(crate) e_node_ids: Column<NodeIdx, ExecutionNodeId>,
    /// Id → `NodeIdx`, for resolving host-supplied identities once per use;
    /// nothing per-run iterates or rebuilds it.
    pub(crate) e_node_index: HashMap<ExecutionNodeId, NodeIdx>,
    pub(crate) inputs: Column<InputIdx, ExecutionInput>,
    pub(crate) events: Column<EventIdx, ExecutionEvent>,
    /// Each node's resolved declared output types (wildcards followed), packed
    /// in the same index space as the plan's output columns. Resolved by the
    /// flatten walk and copied here slot for slot, so the artifact is
    /// self-describing without retaining the func library. Read by the digest
    /// (an output-signature change re-keys). An unresolved wildcard port is
    /// `DataType::Any`. Its length is the artifact's total output count.
    pub(crate) outputs: Column<OutputIdx, ExecutionOutput>,
}

impl std::ops::Index<NodeIdx> for CompiledGraph {
    type Output = ExecutionNode;

    fn index(&self, index: NodeIdx) -> &ExecutionNode {
        &self.e_nodes[index]
    }
}

impl CompiledGraph {
    /// The authored node one execution id came from.
    ///
    /// The id resolves against the artifact first, so an id this install never
    /// emitted is an error rather than a silent answer — which is the whole
    /// reason this is a lookup and not a bare
    /// [`ExecutionNodeId::node_id`](crate::ExecutionNodeId) conversion.
    pub fn attribution(
        &self,
        e_node_id: ExecutionNodeId,
    ) -> Result<NodeId, ExecutionIdentityError> {
        if !self.e_node_index.contains_key(&e_node_id) {
            return Err(ExecutionIdentityError::NodeNotFound { e_node_id });
        }
        Ok(e_node_id.node_id())
    }

    /// The execution node a "run this node" seeds — the node itself, when the
    /// artifact holds it.
    ///
    /// `None` when it does not: a node the program dropped, or one absent from
    /// this program entirely.
    pub fn run_target(&self, node_id: NodeId) -> Option<ExecutionNodeId> {
        self.node(node_id).map(|node_idx| self.e_node_ids[node_idx])
    }

    /// Resolve authored nodes to their execution nodes, then return their
    /// reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let mut in_closure = IdxSet::default();
        in_closure.reset(self.e_nodes.len());
        let mut pending: Vec<NodeIdx> = authored_node_ids
            .iter()
            .filter_map(|node_id| self.node(*node_id))
            .filter(|&node_idx| {
                // The seeds dedup: a caller may name the same node twice.
                let fresh = !in_closure.contains(node_idx);
                in_closure.insert(node_idx);
                fresh
            })
            .collect();
        let consumers = Consumers::reverse(self);
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in consumers.of(node_idx) {
                if !in_closure.contains(consumer_idx) {
                    in_closure.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }

        let closure: Vec<ExecutionNodeId> = in_closure
            .iter()
            .map(|node_idx| self.e_node_ids[node_idx])
            .collect();
        debug_assert!(
            closure.is_sorted(),
            "dense indices are assigned in id order, so an ascending index walk yields ascending ids"
        );
        closure
    }

    pub(crate) fn output_idx(&self, address: OutputAddr) -> OutputIdx {
        self[address.node_idx].outputs.nth(address.port_idx)
    }

    /// Where an authored node landed in the artifact, or `None` if it holds no
    /// compiled work.
    ///
    /// The one place the authoring space crosses into the dense one, so every
    /// question above answers for the same set of nodes.
    fn node(&self, node_id: NodeId) -> Option<NodeIdx> {
        self.e_node_index
            .get(&ExecutionNodeId::from_node(node_id))
            .copied()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::compiled::{CompiledGraph, ExecutionNode};
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::identity::NodeIdx;

    /// Test-facing id lookups: tests and engine introspection address nodes by
    /// their stable id; production paths carry `NodeIdx` instead.
    impl CompiledGraph {
        /// Append one node, assigning the next dense index — the fixture form
        /// of the placement linking performs in one pass. Production programs
        /// are built only by linking a flat graph; this exists so a cache or
        /// digest test can stand one up without one.
        pub(crate) fn push(&mut self, id: ExecutionNodeId, e_node: ExecutionNode) -> NodeIdx {
            let node_idx = NodeIdx(self.e_nodes.len() as u32);
            let previous = self.e_node_index.insert(id, node_idx);
            assert!(previous.is_none(), "flattened node ids must be unique");
            self.e_node_ids.push(id);
            self.e_nodes.push(e_node);
            node_idx
        }

        pub(crate) fn by_id(&self, id: ExecutionNodeId) -> &ExecutionNode {
            &self[self.e_node_index[&id]]
        }

        pub(crate) fn by_id_mut(&mut self, id: ExecutionNodeId) -> &mut ExecutionNode {
            let node_idx = self.e_node_index[&id];
            &mut self.e_nodes[node_idx]
        }
    }
}
