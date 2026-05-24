use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncId, FuncLib};
use crate::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use anyhow::ensure;
use common::{Result, SerdeFormat, deserialize, serialize};
use common::{id_type, is_debug};
use hashbrown::HashSet;

id_type!(NodeId);

/// Address of a producer node's output port — the source side of a data
/// binding (`Binding::Bind`).
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

/// Address of a consumer node's input port. Used to report unsatisfied
/// inputs (`ExecutionStats::missing_inputs`) and the edges the editor's
/// breaker severs. Distinct from `OutputPort` so source/sink intent can't
/// be confused at a call site.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    #[default]
    None,
    Const(StaticValue),
    Bind(OutputPort),
}

// Port/event names are not stored per-node — they're read from the func
// (`FuncInput`/`FuncOutput`/`FuncEvent`) via the node's func, the single
// source of truth. Inputs and events are positional, matched by index
// (`validate_with` asserts the lengths).
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Input {
    #[serde(default)]
    pub binding: Binding,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub subscribers: Vec<NodeId>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum NodeBehavior {
    #[default]
    AsFunction,
    Once,
}

/// What a node *is*. A plain `Func` instance, a composite `Subgraph`
/// instance, or one of the two interface boundary nodes that may appear
/// only inside a `SubgraphDef.graph` (their port arity comes from the
/// enclosing def's interface, not from a func).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Func(FuncId),
    Subgraph(SubgraphRef),
    /// Inbound boundary: outputs = enclosing def's exposed inputs.
    SubgraphInput,
    /// Outbound boundary: inputs = enclosing def's exposed outputs.
    SubgraphOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,

    #[serde(default)]
    pub behavior: NodeBehavior,

    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub events: Vec<Event>,
}

impl NodeKind {
    pub fn as_func(&self) -> Option<FuncId> {
        match self {
            NodeKind::Func(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_subgraph(&self) -> Option<SubgraphRef> {
        match self {
            NodeKind::Subgraph(r) => Some(*r),
            _ => None,
        }
    }

    pub fn is_boundary(&self) -> bool {
        matches!(self, NodeKind::SubgraphInput | NodeKind::SubgraphOutput)
    }
}

impl Node {
    /// The func this node instantiates, or `None` for subgraph/boundary
    /// nodes. Convenience shim over `kind`.
    pub fn func_id(&self) -> Option<FuncId> {
        self.kind.as_func()
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    nodes: KeyIndexVec<NodeId, Node>,

    /// Local (per-instance) subgraph definitions referenced by this graph's
    /// `NodeKind::Subgraph(SubgraphRef::Local(_))` nodes. Editing one of
    /// these affects only this graph. Shared definitions live in
    /// `FuncLib.subgraphs` instead. See `docs/subgraph-design.md` §4.4.
    #[serde(default)]
    pub subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,
}

impl Graph {
    pub fn add(&mut self, node: Node) {
        assert!(!node.id.is_nil(), "cannot add a node with a nil id");
        assert!(
            self.nodes.by_key(&node.id).is_none(),
            "node {:?} already exists; adds must use a fresh id",
            node.id
        );
        self.nodes.add(node);
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Iterate nodes in insertion order. This order is load-bearing:
    /// callers (darkroom-egui rendering, action-stack replay) rely on it.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.nodes.iter_mut()
    }
    pub fn remove_by_id(&mut self, id: NodeId) {
        assert!(!id.is_nil());

        self.nodes.remove_by_key(&id);

        self.nodes
            .iter_mut()
            .flat_map(|node| node.inputs.iter_mut())
            .for_each(|input| match &input.binding {
                Binding::Bind(output_binding) if output_binding.node_id == id => {
                    input.binding = Binding::None;
                }
                _ => {}
            });

        self.nodes
            .iter_mut()
            .flat_map(|node| node.events.iter_mut())
            .for_each(|event| {
                event.subscribers.retain(|sub| *sub != id);
            });
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        assert!(!name.is_empty());
        self.nodes.iter().find(|node| node.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Node> {
        assert!(!name.is_empty());
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    pub fn by_id(&self, id: &NodeId) -> Option<&Node> {
        assert!(!id.is_nil());
        self.nodes.by_key(id)
    }
    pub fn by_id_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());
        self.nodes.by_key_mut(id)
    }

    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
        serialize(self, format)
    }
    pub fn deserialize(serialized: &[u8], format: SerdeFormat) -> Result<Graph> {
        let graph: Self = deserialize(serialized, format)?;
        graph.check()?;
        Ok(graph)
    }

    /// Structural validation of a (possibly untrusted) graph — runs in all
    /// builds and returns an error rather than panicking. `validate` below is
    /// the debug-only internal-invariant check; this is the load-path guard.
    /// Port-index ranges need a `FuncLib`, so they're checked by
    /// `validate_with`, not here.
    pub fn check(&self) -> Result<()> {
        for node in self.nodes.iter() {
            ensure!(!node.id.is_nil(), "graph contains a node with a nil id");
            match &node.kind {
                NodeKind::Func(func_id) => {
                    ensure!(!func_id.is_nil(), "node {:?} has a nil func_id", node.id);
                }
                NodeKind::Subgraph(r) => {
                    ensure!(!r.id().is_nil(), "node {:?} has a nil subgraph id", node.id);
                    if let SubgraphRef::Local(id) = r {
                        ensure!(
                            self.subgraphs.by_key(id).is_some(),
                            "node {:?} references missing local subgraph {:?}",
                            node.id,
                            id
                        );
                    }
                }
                NodeKind::SubgraphInput | NodeKind::SubgraphOutput => {}
            }

            for (input_idx, input) in node.inputs.iter().enumerate() {
                if let Binding::Bind(addr) = &input.binding {
                    ensure!(
                        self.nodes.by_key(&addr.node_id).is_some(),
                        "node {:?} input {} binds to missing node {:?}",
                        node.id,
                        input_idx,
                        addr.node_id
                    );
                }
            }

            for (event_idx, event) in node.events.iter().enumerate() {
                for subscriber in &event.subscribers {
                    ensure!(
                        self.nodes.by_key(subscriber).is_some(),
                        "node {:?} event {} has missing subscriber {:?}",
                        node.id,
                        event_idx,
                        subscriber
                    );
                }
            }
        }

        for def in self.subgraphs.iter() {
            ensure!(!def.id.is_nil(), "local subgraph has a nil id");
            def.graph.check()?;
        }

        Ok(())
    }

    /// Resolve a `SubgraphRef` to its definition: `Local` from this graph's
    /// own table, `Linked` from the shared `FuncLib`.
    pub fn resolve_def<'a>(
        &'a self,
        r: SubgraphRef,
        func_lib: &'a FuncLib,
    ) -> Option<&'a SubgraphDef> {
        match r {
            SubgraphRef::Local(id) => self.subgraphs.by_key(&id),
            SubgraphRef::Linked(id) => func_lib.subgraphs.by_key(&id),
        }
    }

    /// Debug-only internal-invariant gate (compiled out in release, so the
    /// per-edit / per-`update` callers pay nothing there). Shares its
    /// structural definition with `check`; panics because a violation here is
    /// our bug, not bad input.
    pub fn validate(&self) {
        if !is_debug() {
            return;
        }
        self.check().expect("graph structural invariant violated");
    }

    pub fn validate_with(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        self.validate();

        let mut visited: HashSet<SubgraphId> = HashSet::new();
        self.validate_level(func_lib, None, &mut visited);
    }

    /// Recursive per-level validation. `ctx_def` is the enclosing subgraph
    /// definition when validating a def's interior (so boundary nodes can be
    /// checked against the interface), `None` at the top level. `visited`
    /// carries the descent path of `SubgraphId`s for the recursion guard.
    fn validate_level(
        &self,
        func_lib: &FuncLib,
        ctx_def: Option<&SubgraphDef>,
        visited: &mut HashSet<SubgraphId>,
    ) {
        for node in self.nodes.iter() {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    let func = func_lib.by_id(func_id).unwrap();
                    assert_eq!(node.inputs.len(), func.inputs.len());
                    assert_eq!(node.events.len(), func.events.len());
                }
                NodeKind::Subgraph(r) => {
                    let def = self
                        .resolve_def(*r, func_lib)
                        .expect("subgraph node references a missing definition");
                    assert_eq!(node.inputs.len(), def.inputs.len());
                    assert_eq!(node.events.len(), def.events.len());

                    assert!(
                        visited.insert(def.id),
                        "subgraph {:?} is recursive (contains itself)",
                        def.id
                    );
                    def.graph.validate_level(func_lib, Some(def), visited);
                    visited.remove(&def.id);
                }
                NodeKind::SubgraphInput => {
                    assert!(
                        ctx_def.is_some(),
                        "SubgraphInput node is only valid inside a subgraph"
                    );
                    assert!(node.inputs.is_empty(), "SubgraphInput has no data inputs");
                }
                NodeKind::SubgraphOutput => {
                    let def = ctx_def.expect("SubgraphOutput is only valid inside a subgraph");
                    assert_eq!(node.inputs.len(), def.outputs.len());
                }
            }

            for input in node.inputs.iter() {
                if let Binding::Bind(addr) = &input.binding {
                    let producer = self.by_id(&addr.node_id).unwrap();
                    let out_count = self.output_count(producer, func_lib, ctx_def);
                    assert!(addr.port_idx < out_count);
                }
            }
        }
    }

    /// Number of output ports a node exposes — by kind. `ctx_def` is the
    /// enclosing definition, needed only for `SubgraphInput` (whose outputs
    /// are the def's exposed inputs).
    fn output_count(
        &self,
        node: &Node,
        func_lib: &FuncLib,
        ctx_def: Option<&SubgraphDef>,
    ) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => func_lib.by_id(func_id).unwrap().outputs.len(),
            NodeKind::Subgraph(r) => self.resolve_def(*r, func_lib).unwrap().outputs.len(),
            NodeKind::SubgraphInput => ctx_def.unwrap().inputs.len(),
            NodeKind::SubgraphOutput => 0,
        }
    }
}

impl Node {
    /// A fresh node of the given kind with a unique id and no inputs/events.
    /// The id is minted here (the one RNG-touching constructor); callers fill
    /// in wiring, or use `From<&Func>` / `subgraph_instance` for a node whose
    /// ports are pre-shaped from its definition.
    pub fn new(kind: NodeKind) -> Self {
        Node {
            id: NodeId::unique(),
            kind,
            name: String::new(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![],
            events: vec![],
        }
    }

    /// A composite instance node shaped from a definition: one `Input` per
    /// exposed input (seeded with its default), one `Event` per exposed
    /// (outgoing) event. `r` must reference `def`.
    pub fn subgraph_instance(def: &SubgraphDef, r: SubgraphRef) -> Self {
        assert_eq!(r.id(), def.id, "SubgraphRef must reference the given def");

        let inputs = def
            .inputs
            .iter()
            .map(|io| Input {
                binding: match &io.default_value {
                    Some(default) => Binding::Const(default.clone()),
                    None => Binding::None,
                },
            })
            .collect();
        let events = (0..def.events.len()).map(|_| Event::default()).collect();

        Node {
            id: NodeId::unique(),
            kind: NodeKind::Subgraph(r),
            name: def.name.clone(),
            behavior: NodeBehavior::AsFunction,
            inputs,
            events,
        }
    }
}

impl From<&Func> for Node {
    fn from(func: &Func) -> Self {
        let inputs: Vec<Input> = func
            .inputs
            .iter()
            .map(|func_input| Input {
                binding: match &func_input.default_value {
                    Some(default) => Binding::Const(default.clone()),
                    None => Binding::None,
                },
            })
            .collect();

        let events: Vec<Event> = func.events.iter().map(|_| Event::default()).collect();

        Node {
            id: NodeId::unique(),
            kind: NodeKind::Func(func.id),
            name: func.name.clone(),
            behavior: func.node_default_behavior,
            inputs,
            events,
        }
    }
}

impl Binding {
    pub fn as_output_binding(&self) -> Option<&OutputPort> {
        match self {
            Binding::Bind(output_binding) => Some(output_binding),
            _ => None,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.is_none()
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Binding::None)
    }
}

impl KeyIndexKey<NodeId> for Node {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl From<OutputPort> for Binding {
    fn from(value: OutputPort) -> Self {
        Binding::Bind(value)
    }
}

impl From<(NodeId, usize)> for Binding {
    fn from((output_node_id, output_idx): (NodeId, usize)) -> Self {
        Binding::Bind(OutputPort {
            node_id: output_node_id,
            port_idx: output_idx,
        })
    }
}

impl From<StaticValue> for Binding {
    fn from(value: StaticValue) -> Self {
        Binding::Const(value)
    }
}

impl From<i64> for Binding {
    fn from(value: i64) -> Self {
        Binding::Const(value.into())
    }
}

impl NodeBehavior {
    pub fn toggle(&mut self) {
        match self {
            NodeBehavior::AsFunction => *self = NodeBehavior::Once,
            NodeBehavior::Once => *self = NodeBehavior::AsFunction,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::Graph;
    use crate::testing::test_graph;
    use common::SerdeFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();

        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = graph.serialize(format);
            let deserialized = Graph::deserialize(&serialized, format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized, serialized_again);
        }

        let bin = graph.serialize(SerdeFormat::Bitcode);
        let deserialized = Graph::deserialize(&bin, SerdeFormat::Bitcode)?;
        assert_eq!(graph, deserialized);

        Ok(())
    }

    #[test]
    fn check_passes_for_valid_graph() {
        assert!(test_graph().check().is_ok());
    }

    #[test]
    fn check_rejects_dangling_binding() {
        use crate::graph::NodeId;

        let mut graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        // Repoint sum's input at a node that doesn't exist.
        graph.by_id_mut(&sum_id).unwrap().inputs[0].binding = (NodeId::unique(), 0).into();

        let err = graph.check().expect_err("dangling binding must fail check");
        assert!(err.to_string().contains("binds to missing node"));
    }

    #[test]
    fn deserialize_rejects_corrupt_graph() {
        use crate::graph::NodeId;

        let mut graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        graph.by_id_mut(&sum_id).unwrap().inputs[0].binding = (NodeId::unique(), 0).into();

        // serialize doesn't validate; deserialize must reject the dangling bind
        // (the release-path structural guard, not a debug-only assert).
        let bytes = graph.serialize(SerdeFormat::Bitcode);
        assert!(Graph::deserialize(&bytes, SerdeFormat::Bitcode).is_err());
    }

    #[test]
    fn node_remove_test() -> anyhow::Result<()> {
        let mut graph = test_graph();

        let node_id = graph.by_name("sum").unwrap().id;
        graph.remove_by_id(node_id);

        assert!(graph.by_name("sum").is_none());
        assert_eq!(graph.len(), 4);

        for input in graph.iter().flat_map(|node| node.inputs.iter()) {
            if let Some(binding) = input.binding.as_output_binding() {
                assert_ne!(binding.node_id, node_id);
            }
        }

        Ok(())
    }
}
