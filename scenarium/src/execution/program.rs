//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built once by the compiler's link stage — there are no mutators here, and
//! no pass that fills a field in afterwards — then installed as runtime state;
//! it is deliberately not a persistence format. Mutable state is split between
//! the per-run schedule/executor and the cross-run runtime cache.
//!
//! Self-contained: everything a run needs was copied out of the [`Library`](crate::library::Library)
//! at flatten, so nothing here refers to one.

use crate::graph::identity::FuncId;
use hashbrown::HashMap;

use crate::common::column::Column;
use crate::common::pool::{Pool, PoolRange};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::{NodeIdx, OutputAddr, OutputIdx};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
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

    pub inputs: InputRange,
    pub outputs: OutputRange,
    pub events: EventRange,

    pub func_id: FuncId,
    /// Copied from `Func`; changing it invalidates this pure node's cache key.
    pub version: u32,

    pub lambda: FuncLambda,
}

#[derive(Debug, Default)]
pub(crate) struct Program {
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
    pub(crate) inputs: Pool<ExecutionInput>,
    pub(crate) events: Pool<ExecutionEvent>,
    /// Each node's resolved declared output types (wildcards followed), packed
    /// in the same index space as the plan's output columns. Resolved once at
    /// link, from the declarations flatten carried over, so the compiled
    /// program is self-describing without retaining the func library. Read by
    /// the digest (an output-signature change re-keys). An unresolved wildcard
    /// port is `DataType::Any`. Its length is the program's total output count.
    pub(crate) outputs: Pool<ExecutionOutput>,
}

impl std::ops::Index<NodeIdx> for Program {
    type Output = ExecutionNode;

    fn index(&self, index: NodeIdx) -> &ExecutionNode {
        &self.e_nodes[index]
    }
}

impl Program {
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
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::identity::NodeIdx;
    use crate::execution::program::{ExecutionNode, Program};

    /// Test-facing id lookups: tests and engine introspection address nodes by
    /// their stable id; production paths carry `NodeIdx` instead.
    impl Program {
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
