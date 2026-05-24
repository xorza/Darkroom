use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncId, FuncLib};
use anyhow::ensure;
use common::{Result, SerdeFormat, deserialize, serialize};
use common::{id_type, is_debug};

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
// (`FuncInput`/`FuncOutput`/`FuncEvent`) via `node.func_id`, the single
// source of truth. Inputs and events are positional, matched to the func by
// index (`validate_with` asserts the lengths).
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub func_id: FuncId,
    pub name: String,

    #[serde(default)]
    pub behavior: NodeBehavior,

    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    nodes: KeyIndexVec<NodeId, Node>,
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
    /// `dependent_nodes` returns matches in this order, and callers
    /// (darkroom-egui rendering, action-stack replay) rely on it.
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

    /// All nodes that transitively depend on `node_id` (excluding itself),
    /// returned in `iter()` insertion order. Callers rely on this order —
    /// see the `iter()` doc comment.
    pub fn dependent_nodes(&self, node_id: &NodeId) -> Vec<NodeId> {
        assert!(!node_id.is_nil());
        assert!(
            self.by_id(node_id).is_some(),
            "node must exist to find dependents"
        );

        // Build reverse adjacency once: consumers[X] = nodes that Bind to X.
        // O(N·d) up front vs. the previous O(N²·d) repeated linear scans.
        let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for node in self.nodes.iter() {
            for input in &node.inputs {
                if let Binding::Bind(addr) = &input.binding {
                    consumers.entry(addr.node_id).or_default().push(node.id);
                }
            }
        }

        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![*node_id];
        while let Some(current) = stack.pop() {
            if let Some(next) = consumers.get(&current) {
                for &id in next {
                    if seen.insert(id) {
                        stack.push(id);
                    }
                }
            }
        }

        self.nodes
            .iter()
            .filter_map(|n| seen.contains(&n.id).then_some(n.id))
            .collect()
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
            ensure!(
                !node.func_id.is_nil(),
                "node {:?} has a nil func_id",
                node.id
            );

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

        Ok(())
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

        for node in self.nodes.iter() {
            let func = func_lib.by_id(&node.func_id).unwrap();
            assert_eq!(node.inputs.len(), func.inputs.len());
            assert_eq!(node.events.len(), func.events.len());

            for input in node.inputs.iter() {
                if let Binding::Bind(port_address) = &input.binding {
                    let output_node = self.by_id(&port_address.node_id).unwrap();
                    let output_func = func_lib.by_id(&output_node.func_id).unwrap();
                    assert!(port_address.port_idx < output_func.outputs.len());
                }
            }
        }
    }
}

impl Node {
    /// A fresh node with a unique id and no func/inputs/events. The id is
    /// minted here (the one RNG-touching constructor); callers fill in
    /// `func_id` and wiring, or use `From<&Func>` for a func-shaped node.
    // No `Default`: id minting is intentionally explicit, not a silent
    // side effect of `Node::default()`.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Node {
            id: NodeId::unique(),
            func_id: FuncId::nil(),
            name: String::new(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![],
            events: vec![],
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
            func_id: func.id,
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

    #[test]
    fn dependent_nodes() {
        let graph = test_graph();

        let get_id = |name: &str| {
            graph
                .by_name(name)
                .unwrap_or_else(|| panic!("Node named \"{name}\" not found"))
                .id
        };

        let get_a = get_id("get_a");
        let get_b = get_id("get_b");
        let sum = get_id("sum");
        let mult = get_id("mult");
        let print = get_id("print");

        assert_eq!(graph.dependent_nodes(&get_a), vec![sum, mult, print]);
        assert_eq!(graph.dependent_nodes(&get_b), vec![sum, mult, print]);
        assert_eq!(graph.dependent_nodes(&sum), vec![mult, print]);
        assert_eq!(graph.dependent_nodes(&mult), vec![print]);
        assert!(graph.dependent_nodes(&print).is_empty());
    }
}
