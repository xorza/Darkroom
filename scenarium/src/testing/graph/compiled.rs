//! The compile bridge: [`TestGraph::compile`] and the [`Compiled`] view it
//! hands back.
//!
//! Gated, and `pub(crate)`, because every accessor here answers with a
//! crate-private type — an `ExecutionNode`, an `ExecutionBinding`. Nothing
//! outside this crate could read one, so nothing outside it is offered one.

use hashbrown::HashMap;

use crate::DataType;
use crate::execution::compile::Compiler;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding, ExecutionNode};
use crate::execution::compile::error::CompileError;
use crate::execution::identity::NodeIdx;
use crate::graph::identity::NodeId;
use crate::testing::graph::TestGraph;

impl TestGraph {
    /// Lower this fixture, keeping the names. Panics on a compile error —
    /// for the tests where the refusal *is* the subject, see
    /// [`try_compile`](Self::try_compile).
    pub(crate) fn compile(&self) -> Compiled {
        self.try_compile().expect("the fixture graph compiles")
    }

    pub(crate) fn try_compile(&self) -> std::result::Result<Compiled, CompileError> {
        Ok(Compiled {
            program: Compiler::default().compile(&self.graph, &self.library)?,
            ids: self.ids.clone(),
            names: self
                .ids
                .iter()
                .map(|(name, node_id)| (*node_id, name.clone()))
                .collect(),
        })
    }
}

/// A [`TestGraph`] lowered, still answering by the names the fixture gave its
/// nodes.
///
/// A compile crosses into the dense index space. Here the crossing happens
/// once, so a test names the same node it authored rather than translating an
/// id per assertion.
///
/// The artifact stays public: a test that corrupts one to prove a validator
/// catches it, or asks something the accessors below do not shorten, reaches
/// straight for it.
#[derive(Debug)]
pub(crate) struct Compiled {
    pub(crate) program: CompiledGraph,
    ids: HashMap<String, NodeId>,
    names: HashMap<NodeId, String>,
}

impl Compiled {
    pub(crate) fn id(&self, name: &str) -> NodeId {
        *self
            .ids
            .get(name)
            .unwrap_or_else(|| panic!("no node named {name:?} in this fixture"))
    }

    /// Where `name` landed in the dense space.
    pub(crate) fn idx(&self, name: &str) -> NodeIdx {
        self.program
            .node(self.id(name))
            .unwrap_or_else(|| panic!("{name:?} holds no compiled work"))
    }

    /// The name a dense index belongs to — for reading a raw address, like a
    /// wire's interned far end, back into the fixture's own vocabulary.
    pub(crate) fn name(&self, node_idx: NodeIdx) -> &str {
        let node_id = self.program.node_ids[node_idx];
        self.names
            .get(&node_id)
            .unwrap_or_else(|| panic!("{node_id:?} is not a node this fixture named"))
    }

    pub(crate) fn node(&self, name: &str) -> &ExecutionNode {
        &self.program[self.idx(name)]
    }

    /// What `name`'s input `input` lowered to: a wire interned to its
    /// producer's address, a literal that satisfied the port, or nothing —
    /// which is where a type-mismatched binding ends up.
    pub(crate) fn binding(&self, name: &str, input: usize) -> &ExecutionBinding {
        let e_node = self.node(name);
        assert!(
            input < e_node.inputs.len as usize,
            "{name:?} declares no input {input}",
        );
        &self.program.inputs[e_node.inputs.nth(input as u32)].binding
    }

    /// The name of the node feeding `name`'s input `input` — `None` when the
    /// port lowered to a literal or to nothing.
    pub(crate) fn producer(&self, name: &str, input: usize) -> Option<&str> {
        match self.binding(name, input) {
            ExecutionBinding::Bind(address) => Some(self.name(address.node_idx)),
            ExecutionBinding::Const(_) | ExecutionBinding::None => None,
        }
    }

    /// `name`'s resolved output types, wildcards followed — the types the
    /// artifact carries, which no declaration in the library need mention.
    pub(crate) fn output_types(&self, name: &str) -> &[DataType] {
        &self.program.outputs[self.node(name).outputs]
    }

    /// The names subscribed to `name`'s event `event_idx`, in the order the
    /// walk wired them. Empty means nothing subscribes, never "not wired yet".
    pub(crate) fn subscribers(&self, name: &str, event_idx: usize) -> Vec<&str> {
        let events = self.node(name).events;
        self.program.events[events][event_idx]
            .subscribers
            .iter()
            .map(|&node_idx| self.name(node_idx))
            .collect()
    }
}
