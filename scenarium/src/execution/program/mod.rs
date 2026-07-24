//! The compiled, flattened graph: topology + code, immutable across runs.
//! Built by the compiler through graph flattening and installed as runtime
//! state; it is deliberately not a persistence format. Mutable state is split
//! between the per-run plan/resolver/executor and the cross-run runtime cache.

pub(crate) mod index;
pub(crate) mod pool;

use std::sync::Arc;

use hashbrown::HashMap;

use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::program::index::OutputIdx;
use crate::execution::program::pool::{Pool, PoolRange};
use crate::graph::CacheMode;
use crate::library::Library;
use crate::node::definition::{FuncBehavior, FuncId, OutputType};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::node::output_type::{OutputTypeResolver, OutputTypeSource};
use crate::node::special::SpecialNode;
use crate::{DataType, ResourceStamper, StaticValue};

#[derive(Clone, Debug, Default)]
pub(crate) enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(ExecutionOutputPort),
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
    pub subscribers: Vec<ExecutionNodeId>,
    pub lambda: EventLambda,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutput {
    pub(crate) data_type: DataType,
    pub(crate) pinned: bool,
}

pub(crate) type InputRange = PoolRange<ExecutionInput>;
pub(crate) type OutputRange = PoolRange<ExecutionOutput>;
pub(crate) type EventRange = PoolRange<ExecutionEvent>;

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state is keyed by the node id in the executor.
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
    /// [`NodeKind::Special`](crate::graph::NodeKind::Special)); the engine
    /// recognizes the kind for special behavior — e.g. the cache node's path-keyed
    /// load/store + input pruning, with the bypass toggle riding in the variant.
    /// See `README.md` Part C.
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
    pub(crate) e_nodes: HashMap<ExecutionNodeId, ExecutionNode>,
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

impl ExecutionProgram {
    pub(crate) fn output_idx(&self, e_node_id: ExecutionNodeId, port_idx: usize) -> OutputIdx {
        let outputs = self.e_nodes[&e_node_id].outputs;
        debug_assert!(
            port_idx < outputs.len as usize,
            "output port is out of range"
        );
        debug_assert!(
            outputs.start.checked_add(port_idx as u32).is_some(),
            "output pool index must fit in u32"
        );
        OutputIdx(outputs.start.wrapping_add(port_idx as u32))
    }

    /// Fill the output metadata pool by resolving each node's declared output types
    /// (wildcards followed through bindings) from the full `library` — done once at
    /// flatten, where every compiled node's func is guaranteed present
    /// (`validate_for_execution` resolved them). Results are memoized by
    /// [`ExecutionOutputPort`]; unresolved and cyclic wildcard ports store `DataType::Any`.
    pub(crate) fn resolve_output_types(&mut self, library: &Library) {
        let e_nodes = &self.e_nodes;
        let inputs = &self.inputs;
        let source = |port: ExecutionOutputPort| {
            let e_node = &e_nodes[&port.e_node_id];
            let func = match e_node.special {
                Some(special) => special.func(),
                None => library.by_id(&e_node.func_id).expect(
                    "a compiled node's func is registered in the library \
                     (validated at validate_for_execution)",
                ),
            };
            match &func.outputs[port.port_idx].ty {
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
        for (e_node_id, e_node) in e_nodes {
            for port_idx in 0..e_node.outputs.len as usize {
                let data_type = resolver.resolve(
                    ExecutionOutputPort {
                        e_node_id: *e_node_id,
                        port_idx,
                    },
                    &source,
                );
                self.outputs[e_node.outputs.start as usize + port_idx].data_type = data_type;
            }
        }
    }
}
