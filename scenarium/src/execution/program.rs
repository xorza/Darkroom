//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the engine (subgraph flattening) and the only serializable part of
//! the pipeline — its persistent form. All mutable execution state lives in the
//! [`Executor`](crate::execution::executor::Executor); all per-run scheduling state in the
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan).

use common::{KeyIndexKey, KeyIndexVec, Span};
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::elements::special::SpecialNode;
use crate::event_lambda::EventLambda;
use crate::function::FuncBehavior;
use crate::graph::NodeId;
use crate::prelude::{FuncId, FuncLambda};

// === Execution Binding ===

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionPortAddress {
    pub target_id: NodeId,
    pub target_idx: usize,
    pub port_idx: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}

impl ExecutionBinding {
    pub(crate) fn as_bind(&self) -> Option<&ExecutionPortAddress> {
        match self {
            ExecutionBinding::Bind(addr) => Some(addr),
            _ => None,
        }
    }
}

// === Execution Node Components ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExecutionInput {
    pub required: bool,
    pub binding: ExecutionBinding,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionEvent {
    pub subscribers: Vec<NodeId>,
    #[serde(skip, default)]
    pub lambda: EventLambda,
}

// === Execution Node ===

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state lives in the executor's slot of the same index.
#[derive(Default, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionNode {
    pub id: NodeId,
    pub(crate) inited: bool,

    pub terminal: bool,
    /// Copied from the node's func at flatten. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub behavior: FuncBehavior,

    /// The authoring node's `Disk` cache request, flattened from
    /// [`CachePersistence`](crate::graph::CachePersistence). Read by the engine's
    /// disk-cache load/store when one is configured; honored only when the node has
    /// a content digest (a reproducible cone) — see `digest.rs`.
    #[serde(default)]
    pub persist: bool,

    /// `Some` for a built-in [`SpecialNode`] (flattened from
    /// [`NodeKind::Special`](crate::graph::NodeKind::Special)); the engine
    /// recognizes the kind for special behavior — e.g. the cache node's path-keyed
    /// load/store + input pruning, with the bypass toggle riding in the variant.
    /// See `docs/file-cache-design.md`.
    #[serde(default)]
    pub special: Option<SpecialNode>,

    pub inputs: Span,
    pub outputs: Span,
    pub events: Span,

    pub func_id: FuncId,

    /// Copied from [`Func::version`](crate::function::Func::version) at flatten
    /// time so the content digest is self-contained in the program. Bumping a
    /// func's version flows here and invalidates its disk-cached outputs.
    #[serde(default)]
    pub func_version: u64,

    #[serde(skip)]
    pub lambda: FuncLambda,

    #[serde(default)]
    pub name: String,
}

impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

// === Execution Program ===

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ExecutionProgram {
    pub(crate) e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    pub(crate) inputs: Vec<ExecutionInput>,
    pub(crate) events: Vec<ExecutionEvent>,
    /// Total output count across all nodes (sum of every output span length) —
    /// the size of the plan's `output_usage` column. Outputs carry no per-node
    /// static data, so there is no output pool; flatten accumulates this total
    /// while assigning spans and stores it here rather than re-summing each plan.
    #[serde(default)]
    pub(crate) n_outputs: usize,
}

impl ExecutionProgram {
    pub(crate) fn clear(&mut self) {
        self.e_nodes.clear();
        self.inputs.clear();
        self.events.clear();
        self.n_outputs = 0;
    }
}
