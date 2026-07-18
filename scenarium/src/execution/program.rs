//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the compiler through graph flattening and installed as runtime
//! state; it is deliberately not a persistence format. All mutable execution
//! state lives in the [`Executor`](crate::execution::executor::Executor); all
//! per-run scheduling state lives in the
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan).

use std::ops::{Index, IndexMut};
use std::sync::Arc;

use common::{KeyIndexKey, KeyIndexVec, Span};

use crate::graph::{CacheMode, NodeId};
use crate::library::Library;
use crate::node::definition::{Func, FuncBehavior, FuncId, OutputType};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::node::special::SpecialNode;
use crate::{DataType, ResourceStamper, StaticValue};

/// A position into the flat node table — `e_nodes`, the cache's `slots`, and the
/// per-node plan/cache columns are all indexed by it. Resolved at flatten and stable
/// for the program's lifetime. A newtype so it can't be crossed with an input-pool
/// index, an output-pool index, or a port number: those are different spaces, and
/// mixing them was previously a silent `usize` bug caught only by debug `validate`.
/// `KeyIndexVec<NodeId, _>` indexes by it directly; plain `Vec` columns go through
/// [`NodeIdx::idx`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeIdx(pub(crate) u32);

impl NodeIdx {
    pub(crate) fn idx(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for NodeIdx {
    fn from(i: usize) -> Self {
        NodeIdx(i as u32)
    }
}

impl<V: KeyIndexKey<NodeId>> Index<NodeIdx> for KeyIndexVec<NodeId, V> {
    type Output = V;
    fn index(&self, i: NodeIdx) -> &V {
        &self[i.idx()]
    }
}

impl<V: KeyIndexKey<NodeId>> IndexMut<NodeIdx> for KeyIndexVec<NodeId, V> {
    fn index_mut(&mut self, i: NodeIdx) -> &mut V {
        &mut self[i.idx()]
    }
}

/// A position in the program's flat output pool. It cannot be confused with a node
/// position or a node-local port number.
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

/// A flat output address: producer node `target_idx` (a [`NodeIdx`], resolved at
/// flatten and stable for the program's lifetime), output `port_idx`. The producer's
/// `NodeId` is *not* stored — it's `e_nodes[target_idx].id`; the index is the
/// canonical key here, so there's no id/index pair to keep in sync.
#[derive(Clone, Default, Debug)]
pub(crate) struct ExecutionPortAddress {
    pub target_idx: NodeIdx,
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
    /// Flat positions of the nodes subscribed to this event, resolved at flatten —
    /// like a binding's `target_idx`, so the planner seeds its walk roots without a
    /// per-plan id→index lookup. (Data and event edges now address the same way.)
    pub subscribers: Vec<NodeIdx>,
    pub lambda: EventLambda,
}

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state lives in the executor's slot of the same index.
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

    /// Copied from [`Func::version`](crate::node::definition::Func::version) at flatten
    /// time so the content digest is self-contained in the program. Bumping a
    /// func's version flows here and invalidates its disk-cached outputs.
    pub func_version: u64,

    pub lambda: FuncLambda,

    pub name: String,
}

impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionProgram {
    pub(crate) e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
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

    /// Every node position, typed — `for idx in program.node_indices()` instead of
    /// `for idx in 0..program.e_nodes.len()` so the loop variable is a [`NodeIdx`].
    pub(crate) fn node_indices(&self) -> impl Iterator<Item = NodeIdx> {
        (0..self.e_nodes.len()).map(NodeIdx::from)
    }

    pub(crate) fn output_idx(&self, node_idx: NodeIdx, port_idx: usize) -> OutputIdx {
        let outputs = self.e_nodes[node_idx].outputs;
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
    /// Each node's types are written through its *span*: spans are assigned in
    /// flatten emit order, which diverges from index order whenever a consumer's
    /// `set_input` claimed its producer's index early — a sequential push would
    /// hand nodes each other's types.
    pub(crate) fn resolve_output_types(&mut self, library: &Library) {
        let total: usize = self.e_nodes.iter().map(|n| n.outputs.len as usize).sum();
        let mut types = vec![DataType::Any; total];
        for idx in self.node_indices() {
            let span = self.e_nodes[idx].outputs;
            for port in 0..span.len as usize {
                types[span.start as usize + port] =
                    effective_output_type(self, library, idx, port, 0).unwrap_or(DataType::Any);
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

/// The concrete declared output type of node `idx`'s `port`, resolving a wildcard
/// reroute by following its mirrored input through the program's bindings. `None`
/// *only* for an output with no concrete type — a wildcard whose mirror isn't a
/// `Bind` (a const/unbound mirror is never a custom value), an out-of-range port, or
/// a cyclic chain past [`MAX_WILDCARD_DEPTH`]. An **absent func is not a `None` case**:
/// every compiled node's func resolved at `check_for_execution`, so its absence is an invariant
/// violation that panics rather than silently degrading.
fn effective_output_type(
    program: &ExecutionProgram,
    library: &Library,
    idx: NodeIdx,
    port: usize,
    depth: usize,
) -> Option<DataType> {
    if depth > MAX_WILDCARD_DEPTH {
        return None;
    }
    let func = node_func(program, library, idx).expect(
        "a compiled node's func is registered in the library (validated at check_for_execution)",
    );
    match &func.outputs.get(port)?.ty {
        OutputType::Fixed(data_type) => Some(data_type.clone()),
        OutputType::Wildcard { mirrors } => {
            let span = program.e_nodes[idx].inputs;
            match &program.inputs[span.range()].get(*mirrors)?.binding {
                ExecutionBinding::Bind(addr) => effective_output_type(
                    program,
                    library,
                    addr.target_idx,
                    addr.port_idx,
                    depth + 1,
                ),
                _ => None,
            }
        }
    }
}

/// The func backing node `idx`: a special node's hardcoded spec, else the library
/// entry for its `func_id`. `None` only if the library doesn't carry the func — which,
/// for the program's own (`check_for_execution`-validated) library, never happens.
fn node_func<'a>(
    program: &'a ExecutionProgram,
    library: &'a Library,
    idx: NodeIdx,
) -> Option<&'a Func> {
    match program.e_nodes[idx].special {
        Some(special) => Some(special.func()),
        None => library.by_id(&program.e_nodes[idx].func_id),
    }
}
