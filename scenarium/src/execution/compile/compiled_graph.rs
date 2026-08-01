//! The compile artifact: the lowered graph — topology + code — plus every
//! question a host asks of one.
//!
//! Built once by the [`Compiler`](crate::execution::compile::Compiler)'s walk —
//! there are no mutators here, and no pass that fills a field in afterwards —
//! then installed as runtime state; it is deliberately not a persistence format.
//! Mutable state is split between the per-run schedule/executor and the
//! cross-run runtime cache.
//!
//! Self-contained: everything a run needs was copied out of the [`Library`](crate::library::Library)
//! at compile, so nothing here refers to one.
//!
//! **One authored node, one execution node, one [`NodeId`].** Nothing splits or
//! merges on the way in, so resolving an authored id against the artifact is a
//! search of the id column it already carries rather than a lookup through a
//! table beside it.

use crate::graph::identity::FuncId;

use crate::common::column::{Column, Span};
use crate::execution::identity::{EventIdx, InputIdx, NodeIdx, OutputAddr, OutputIdx};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::NodeId;
use crate::graph::node::CacheMode;
use crate::graph::node::special::SpecialNode;
use crate::{ConstValue, DataType};

#[derive(Clone, Debug, Default)]
pub(crate) enum ExecutionBinding {
    #[default]
    None,
    Const(ConstValue),
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

/// Topology + code for one lowered node. Immutable across runs; mutable per-run
/// state lives in `NodeIdx`-aligned columns, and cross-run cache slots are
/// keyed by the node's stable id.
///
/// Built whole by the compiler's walk, straight into the dense index space the
/// id sort settled before it. Everything here is `Copy` but the lambda, and that
/// is an `Arc`, so taking one off a declaration is a refcount bump and the
/// library stays readable behind it — which is what `Clone` is for.
#[derive(Default, Debug, Clone)]
pub(crate) struct ExecutionNode {
    pub sink: bool,
    /// The authoring node is disabled. Ambient planning excludes it; an
    /// explicit node seed overrides it for that run.
    pub disabled: bool,
    /// Copied from the node's func at lowering. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub behavior: FuncBehavior,

    /// The authoring node's cache mode, copied from
    /// [`CacheMode`] at lowering. Its two bits
    /// ([`caches_in_ram`](crate::graph::node::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::node::CacheMode::persists_to_disk)) gate RAM retention
    /// and the disk load/store; disk is honored only when the node has a
    /// content digest (a reproducible cone) and a disk root is configured — see `digest.rs`
    /// and `disk_store.rs`.
    pub cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (lowered from
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

/// The compile artifact: the lowered, immutable program — lambdas, resolved
/// output types, and bound-path stamping metadata. Self-contained: executing it
/// needs neither the authoring graph nor the library.
///
/// [`Default`] is the empty artifact, which is what an engine holds before
/// anything is installed.
#[derive(Debug, Default)]
pub struct CompiledGraph {
    /// The dense node column — every per-run column and set aligns to it.
    /// Ordered by `NodeId`, so compiled artifacts and program walks are
    /// deterministic however the authoring graph was walked.
    pub(crate) e_nodes: Column<NodeIdx, ExecutionNode>,
    /// `NodeIdx` → authoring-derived id, for the host boundary (reports,
    /// seeds, eviction, cache slots) — **sorted**, which is what lets it answer
    /// the reverse direction too. See [`node`](Self::node).
    pub(crate) node_ids: Column<NodeIdx, NodeId>,
    pub(crate) inputs: Column<InputIdx, ExecutionInput>,
    pub(crate) events: Column<EventIdx, ExecutionEvent>,
    /// Each node's resolved declared output types (wildcards followed), packed
    /// in the same index space as the plan's output columns. Resolved by the
    /// lowering walk and copied here slot for slot, so the artifact is
    /// self-describing without retaining the func library. Read by the digest
    /// (an output-signature change re-keys). An unresolved wildcard port is
    /// `DataType::Any`. Its length is the artifact's total output count.
    pub(crate) outputs: Column<OutputIdx, DataType>,
}

impl std::ops::Index<NodeIdx> for CompiledGraph {
    type Output = ExecutionNode;

    fn index(&self, index: NodeIdx) -> &ExecutionNode {
        &self.e_nodes[index]
    }
}

impl CompiledGraph {
    /// Where an authored node landed, or `None` if this artifact holds no
    /// compiled work for it.
    ///
    /// The one place the authoring space crosses into the dense one. A binary
    /// search rather than a side index: the compile places nodes in id order, so
    /// `node_ids` is *already* arranged for this lookup — a map beside it would
    /// be a second copy of that arrangement, allocated per compile and carried
    /// for the life of every installed artifact.
    pub(crate) fn node(&self, node_id: NodeId) -> Option<NodeIdx> {
        self.node_ids.search_sorted(&node_id)
    }

    /// Whether this artifact holds compiled work for an authored node.
    ///
    /// The one question the authoring space can ask of a program without
    /// entering the dense space: it answers both "can this node seed a run"
    /// and "does a report naming this node belong to this install".
    pub fn contains(&self, node_id: NodeId) -> bool {
        self.node(node_id).is_some()
    }

    pub(crate) fn output_idx(&self, address: OutputAddr) -> OutputIdx {
        self[address.node_idx].outputs.nth(address.port_idx)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionNode};
    use crate::execution::identity::NodeIdx;
    use crate::graph::identity::NodeId;

    impl CompiledGraph {
        /// Append one node, assigning the next dense index — the fixture form
        /// of the placement a compile performs before it walks.
        ///
        /// Ascending ids, because [`node`](CompiledGraph::node) binary-searches
        /// `node_ids`: a fixture that pushed out of order would build a program
        /// whose own nodes it cannot find. Ascending also means unique, so one
        /// id cannot name two nodes.
        pub(crate) fn push(&mut self, id: NodeId, e_node: ExecutionNode) -> NodeIdx {
            assert!(
                self.node_ids.iter().last().is_none_or(|last| *last < id),
                "a program's nodes are placed in ascending id order"
            );
            let node_idx = NodeIdx(self.e_nodes.len() as u32);
            self.node_ids.push(id);
            self.e_nodes.push(e_node);
            node_idx
        }
    }
}

#[cfg(test)]
mod id_lookups {
    use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionNode};
    use crate::graph::identity::NodeId;

    /// Id lookups for a unit test that stood a program up by hand and knows its
    /// nodes by the ids it gave them. Production paths carry `NodeIdx`, so
    /// nothing outside a test pays the search.
    impl CompiledGraph {
        pub(crate) fn by_id(&self, id: NodeId) -> &ExecutionNode {
            &self[self.node(id).expect("the fixture placed this node")]
        }

        pub(crate) fn by_id_mut(&mut self, id: NodeId) -> &mut ExecutionNode {
            let node_idx = self.node(id).expect("the fixture placed this node");
            &mut self.e_nodes[node_idx]
        }
    }
}
