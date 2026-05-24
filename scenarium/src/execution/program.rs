//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the engine (subgraph flattening) and the only serializable part of
//! the pipeline — its persistent form. All mutable execution state lives in the
//! [`Executor`](super::executor::Executor); all per-run scheduling state in the
//! [`ExecutionPlan`](super::plan::ExecutionPlan).

use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};

use crate::data::{DataType, StaticValue};
use crate::event_lambda::EventLambda;
use crate::function::FuncBehavior;
use crate::graph::{NodeBehavior, NodeId};
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
    /// Cross-run dirty bit: set by `flatten` when the binding changes at
    /// `update`, consumed/cleared by the executor's run. Bridges update→run,
    /// so unlike the per-run input flags it stays on the input, not the plan.
    pub binding_changed: bool,
    pub binding: ExecutionBinding,
    pub data_type: DataType,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionEvent {
    pub subscribers: Vec<NodeId>,
    #[serde(skip, default)]
    pub lambda: EventLambda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum ExecutionBehavior {
    #[default]
    Impure,
    Pure,
    Once,
}

/// A contiguous slice into one of the program's SoA pools.
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) len: u32,
}

impl Span {
    pub(crate) fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

// === Execution Node ===

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state lives in the executor's slot of the same index.
#[derive(Default, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionNode {
    pub id: NodeId,
    pub(crate) inited: bool,

    pub terminal: bool,
    pub behavior: ExecutionBehavior,

    pub inputs: Span,
    pub outputs: Span,
    pub events: Span,

    pub func_id: FuncId,

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

impl ExecutionNode {
    pub(crate) fn compute_behavior(
        node_behavior: NodeBehavior,
        func_behavior: FuncBehavior,
    ) -> ExecutionBehavior {
        match node_behavior {
            NodeBehavior::AsFunction => match func_behavior {
                FuncBehavior::Pure => ExecutionBehavior::Pure,
                FuncBehavior::Impure => ExecutionBehavior::Impure,
            },
            NodeBehavior::Once => ExecutionBehavior::Once,
        }
    }
}

// === Execution Program ===

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ExecutionProgram {
    pub(crate) e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    pub(crate) inputs: Vec<ExecutionInput>,
    pub(crate) events: Vec<ExecutionEvent>,
}

impl ExecutionProgram {
    /// Total output count across all nodes (sum of every output span length) —
    /// the size of the plan's `output_usage` column. Outputs carry no per-node
    /// static data, so there is no output pool, just the per-node `outputs`
    /// span; the total is derived from those spans rather than stored.
    pub(crate) fn n_outputs(&self) -> usize {
        self.e_nodes.iter().map(|n| n.outputs.len as usize).sum()
    }

    pub(crate) fn clear(&mut self) {
        self.e_nodes.clear();
        self.inputs.clear();
        self.events.clear();
    }
}
