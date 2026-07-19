//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the compiler through graph flattening and installed as runtime
//! state; it is deliberately not a persistence format. All mutable execution
//! state lives in the [`Executor`](crate::execution::executor::Executor); all
//! per-run scheduling state lives in the
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan).

use std::sync::Arc;

use common::Span;
use hashbrown::HashMap;

use crate::graph::{CacheMode, NodeId};
use crate::library::Library;
use crate::node::definition::{Func, FuncBehavior, FuncId, OutputType};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::node::special::SpecialNode;
use crate::{DataType, ResourceStamper, StaticValue};

/// A position in the program's flat output pool. It cannot be confused with a node
/// id or a node-local port number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct OutputIdx(pub(crate) u32);

impl OutputIdx {
    pub(crate) fn idx(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for OutputIdx {
    fn from(i: usize) -> Self {
        OutputIdx(u32::try_from(i).expect("output pool index must fit in u32"))
    }
}

/// A flat output address: producer node id and output port.
#[derive(Clone, Default, Debug)]
pub(crate) struct ExecutionPortAddress {
    pub target: NodeId,
    pub port_idx: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}

/// How an input's delivered value folds its **referent's** identity into the consumer's
/// content digest — present iff the input is *declared* with a resource-reference type
/// (the contract "this node dereferences the reference"), resolved from the func spec +
/// library at flatten. `FsPath` is structural (a `DataType` variant) and folds through the
/// digest's built-in file/dir identity; a nominal custom type folds through the
/// [`ResourceStamper`] registered on its library entry.
#[derive(Debug, Clone)]
pub(crate) enum InputStamper {
    FsPath,
    Custom(Arc<dyn ResourceStamper>),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExecutionInput {
    pub required: bool,
    /// `Some` iff this input is declared with a resource-reference type (see
    /// [`InputStamper`]). Resolved from the library at every flatten.
    pub stamper: Option<InputStamper>,
    pub binding: ExecutionBinding,
}

#[derive(Default, Debug)]
pub(crate) struct ExecutionEvent {
    pub subscribers: Vec<NodeId>,
    pub lambda: EventLambda,
}

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state is keyed by the node id in the executor.
#[derive(Default, Debug)]
pub(crate) struct ExecutionNode {
    pub id: NodeId,

    pub sink: bool,
    /// Copied from the node's func at flatten. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub behavior: FuncBehavior,

    /// The authoring node's cache mode, copied from
    /// [`CacheMode`](crate::graph::CacheMode) at flatten. Its two bits
    /// ([`caches_in_ram`](crate::graph::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::CacheMode::persists_to_disk)) gate RAM reuse and
    /// the disk load/store; disk is honored only when the node has a
    /// content digest (a reproducible cone) and a disk root is configured — see `digest.rs`
    /// and `disk_store.rs`.
    pub cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (flattened from
    /// [`NodeKind::Special`](crate::graph::NodeKind::Special)); the engine
    /// recognizes the kind for special behavior — e.g. the cache node's path-keyed
    /// load/store + input pruning, with the bypass toggle riding in the variant.
    /// See `README.md` Part C.
    pub special: Option<SpecialNode>,

    pub inputs: Span,
    pub outputs: Span,
    pub events: Span,

    pub func_id: FuncId,

    pub lambda: FuncLambda,

    pub name: String,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionProgram {
    pub(crate) e_nodes: HashMap<NodeId, ExecutionNode>,
    /// Deterministic flatten order for scheduling independent nodes. Node identity
    /// and all lookups still use `NodeId`.
    pub(crate) node_order: Vec<NodeId>,
    pub(crate) inputs: Vec<ExecutionInput>,
    pub(crate) events: Vec<ExecutionEvent>,
    /// The output pool: each node's resolved declared output types (wildcards
    /// followed), flat and indexed like the plan's output columns — `e_node.outputs.range()` is
    /// the node's slice. Resolved once at flatten by [`Self::resolve_output_types`]
    /// from the func library (which the program doesn't retain), so the compiled
    /// program is self-describing. Read by the digest (an output-signature change
    /// re-keys) and the disk cache's codec check. An unresolved wildcard port is
    /// `DataType::Any`. Its `len()` is the program's total output count
    /// ([`Self::n_outputs`]).
    pub(crate) output_types: Vec<DataType>,
    /// Whether each pooled output port is pinned — copied from
    /// [`Graph::pinned_outputs`](crate::graph::Graph) at flatten, same
    /// indexing as `output_types`. The planner reads this to count a
    /// pinned port as used even with no in-graph binding.
    pub(crate) output_pinned: Vec<bool>,
}

impl ExecutionProgram {
    /// The program's total output count: the length of the `output_types` pool and the
    /// plan's output columns (every node's output span summed). Derived from
    /// `output_types` rather than stored, so it can't disagree with the pool it sizes.
    pub(crate) fn n_outputs(&self) -> usize {
        self.output_types.len()
    }

    pub(crate) fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.node_order.iter().copied()
    }

    pub(crate) fn output_idx(&self, node_id: NodeId, port_idx: usize) -> OutputIdx {
        let outputs = self.e_nodes[&node_id].outputs;
        assert!(
            port_idx < outputs.len as usize,
            "output port is out of range"
        );
        let port_idx = u32::try_from(port_idx).expect("output port index must fit in u32");
        OutputIdx(outputs.start + port_idx)
    }

    pub(crate) fn pinned_output_indices(&self) -> impl Iterator<Item = OutputIdx> + '_ {
        self.output_pinned
            .iter()
            .enumerate()
            .filter(|(_, pinned)| **pinned)
            .map(|(idx, _)| OutputIdx::from(idx))
    }

    pub(crate) fn is_output_pinned(&self, output_idx: OutputIdx) -> bool {
        self.output_pinned[output_idx.idx()]
    }

    /// `e_node`'s slice of the shared input pool.
    pub(crate) fn node_inputs(&self, e_node: &ExecutionNode) -> &[ExecutionInput] {
        &self.inputs[e_node.inputs.range()]
    }

    /// `e_node`'s slice of the resolved output-type pool.
    pub(crate) fn node_output_types(&self, e_node: &ExecutionNode) -> &[DataType] {
        &self.output_types[e_node.outputs.range()]
    }

    /// Fill the `output_types` pool by resolving each node's declared output types
    /// (wildcards followed through bindings) from the full `library` — done once at
    /// flatten, where every compiled node's func is guaranteed present (`check_for_execution`
    /// resolved them). An unresolved wildcard port stores `DataType::Any`. Builds
    /// into a fresh buffer first so the per-node reads don't alias the write-back.
    /// Each node's types are written through its span because the output pool is
    /// independent from the node map's storage order.
    pub(crate) fn resolve_output_types(&mut self, library: &Library) {
        let total: usize = self.e_nodes.values().map(|n| n.outputs.len as usize).sum();
        let mut types = vec![DataType::Any; total];
        for node_id in self.node_ids() {
            let span = self.e_nodes[&node_id].outputs;
            for port in 0..span.len as usize {
                types[span.start as usize + port] =
                    effective_output_type(self, library, node_id, port, 0).unwrap_or(DataType::Any);
            }
        }
        self.output_types = types;
    }
}

/// Backstop for a wildcard chain that cycles (a malformed program the planner rejects
/// as `CycleDetected`, but output types resolve at flatten, before planning): beyond
/// this depth resolution gives up with `None` rather than recursing forever.
/// Legitimate reroute chains are a handful deep.
const MAX_WILDCARD_DEPTH: usize = 64;

/// The concrete declared output type of `node_id`'s `port`, resolving a wildcard
/// reroute by following its mirrored input through the program's bindings. `None`
/// *only* for an output with no concrete type — a wildcard whose mirror isn't a
/// `Bind` (a const/unbound mirror is never a custom value), an out-of-range port, or
/// a cyclic chain past [`MAX_WILDCARD_DEPTH`]. An **absent func is not a `None` case**:
/// every compiled node's func resolved at `check_for_execution`, so its absence is an invariant
/// violation that panics rather than silently degrading.
fn effective_output_type(
    program: &ExecutionProgram,
    library: &Library,
    node_id: NodeId,
    port: usize,
    depth: usize,
) -> Option<DataType> {
    if depth > MAX_WILDCARD_DEPTH {
        return None;
    }
    let func = node_func(program, library, node_id).expect(
        "a compiled node's func is registered in the library (validated at check_for_execution)",
    );
    match &func.outputs.get(port)?.ty {
        OutputType::Fixed(data_type) => Some(data_type.clone()),
        OutputType::Wildcard { mirrors } => {
            let span = program.e_nodes[&node_id].inputs;
            match &program.inputs[span.range()].get(*mirrors)?.binding {
                ExecutionBinding::Bind(addr) => {
                    effective_output_type(program, library, addr.target, addr.port_idx, depth + 1)
                }
                _ => None,
            }
        }
    }
}

/// The func backing `node_id`: a special node's hardcoded spec, else the library
/// entry for its `func_id`. `None` only if the library doesn't carry the func — which,
/// for the program's own (`check_for_execution`-validated) library, never happens.
fn node_func<'a>(
    program: &'a ExecutionProgram,
    library: &'a Library,
    node_id: NodeId,
) -> Option<&'a Func> {
    match program.e_nodes[&node_id].special {
        Some(special) => Some(special.func()),
        None => library.by_id(&program.e_nodes[&node_id].func_id),
    }
}
