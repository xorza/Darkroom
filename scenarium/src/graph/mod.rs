use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use ::serde::{Deserialize, Serialize};
use anyhow::Result;
use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

use crate::StaticValue;
use crate::graph::interface::{GraphEvent, GraphId, GraphLink};
use crate::library::Library;
use crate::node::definition::{Func, FuncId};
use crate::node::definition::{FuncInput, FuncOutput};
use crate::node::special::SpecialNode;
use common::id_type;
use common::{SerdeFormat, deserialize, serialize};

id_type!(NodeId);

pub(crate) mod clone;
pub(crate) mod interface;
mod query;
pub(crate) mod reconcile;
mod serde;
#[cfg(test)]
mod tests;
mod validate;
pub(crate) mod wiring;

/// Address of a producer node's output port — the source side of a data
/// binding (`Binding::Bind`).
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl OutputPort {
    pub fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}

/// Address of a consumer node's input port. Keys a node's data binding in
/// `Graph.bindings`, and reports unsatisfied inputs
/// (`ExecutionStats::missing_inputs`) / edges the editor's breaker severs.
/// Distinct from `OutputPort` so source/sink intent can't be confused.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl InputPort {
    pub fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}

/// What a consumer input port is wired to. Stored sparsely: `Graph.bindings`
/// only holds these values, and an absent port is unbound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    Const(StaticValue),
    Bind(OutputPort),
}

/// One event-subscription edge: `subscriber` fires when `emitter`'s event
/// `event_idx` triggers. Ordered (emitter, event_idx, subscriber) so a
/// `BTreeSet` ranges over one emitter-event's subscribers contiguously.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub emitter: NodeId,
    pub event_idx: usize,
    pub subscriber: NodeId,
}

/// Where a node's computed output is cached — the two orthogonal storage bits
/// *keep in RAM* ([`caches_in_ram`](Self::caches_in_ram)) and *persist to disk*
/// ([`persists_to_disk`](Self::persists_to_disk)) as one four-state enum:
///
/// - `None` — cache nowhere: never reused across runs, recomputed whenever its value
///   is needed, and dropped after the run to free RAM.
/// - `Ram` — resident in the live engine and reused across runs, but lost on reload.
/// - `Disk` — persisted to the disk store (survives reload); its RAM copy
///   is demoted to disk-only after the run and reloaded lazily.
/// - `Both` — resident in RAM *and* on disk: hot reuse this session plus survival across
///   reloads.
///
/// This is a *storage* choice only — it never affects reproducibility. Disk/RAM reuse is
/// honored only for a node with a content digest (a reproducible cone); a node with an
/// impure node anywhere upstream has no digest, so it's silently kept memory-only and
/// never risks serving a stale value, whatever its mode. The on-disk backend is wired only
/// once a caller enables it (`ExecutionEngine::set_disk_store` with a disk root); until
/// then `Disk`/`Both` degrade to memory-only. See `execution/README.md` Part B.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum CacheMode {
    #[default]
    None,
    Ram,
    Disk,
    Both,
}

impl CacheMode {
    /// Whether the node's value is retained in RAM and reused across runs (`Ram`/`Both`).
    /// The other modes drop or demote the RAM copy after each run.
    pub fn caches_in_ram(self) -> bool {
        matches!(self, CacheMode::Ram | CacheMode::Both)
    }

    /// Whether the node's value is persisted to the disk store
    /// (`Disk`/`Both`), so it survives a reload.
    pub fn persists_to_disk(self) -> bool {
        matches!(self, CacheMode::Disk | CacheMode::Both)
    }

    /// Compose a mode from the two storage bits — the inverse of
    /// [`caches_in_ram`](Self::caches_in_ram)/[`persists_to_disk`](Self::persists_to_disk),
    /// used by the editor's two independent cache toggles.
    pub fn from_bits(ram: bool, disk: bool) -> Self {
        match (ram, disk) {
            (false, false) => CacheMode::None,
            (true, false) => CacheMode::Ram,
            (false, true) => CacheMode::Disk,
            (true, true) => CacheMode::Both,
        }
    }
}

/// What a node *is*. A plain `Func` instance, a nested `Graph`
/// instance, a built-in [`SpecialNode`] (hardcoded declaration, recognized by
/// the engine), or one of the two graph-interface boundary nodes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Func(FuncId),
    Graph(GraphLink),
    /// A built-in special node; its interface comes from
    /// [`SpecialNode::func`].
    Special(SpecialNode),
    /// Inbound boundary: outputs = the graph's exposed inputs.
    GraphInput,
    /// Outbound boundary: inputs = the graph's exposed outputs.
    GraphOutput,
}

// A node is pure authored data. Identity is its key in `Graph::nodes`; port/event
// arity comes from the func or graph interface, and wiring lives in side tables.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub name: String,

    /// Where this node's output is cached. See [`CacheMode`]. A fresh func node
    /// (`From<&Func>`) or special node (`Node::new`) copies its func's
    /// `default_cache_mode`; the remaining func-less constructors seed `None`.
    pub cache: CacheMode,

    /// Disabled nodes are skipped at flatten time: they emit no execution
    /// node, and any binding resolving to one yields no producer, so a
    /// downstream consumer sees the wire as unbound (→ `MissingInputs` if
    /// the input is required).
    pub disabled: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    pub id: NodeId,
    node: &'a Node,
}

impl Deref for NodeRef<'_> {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        self.node
    }
}

impl NodeKind {
    pub fn as_func(&self) -> Option<FuncId> {
        match self {
            NodeKind::Func(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_graph(&self) -> Option<GraphLink> {
        match self {
            NodeKind::Graph(link) => Some(*link),
            _ => None,
        }
    }

    pub fn is_boundary(&self) -> bool {
        matches!(self, NodeKind::GraphInput | NodeKind::GraphOutput)
    }
}

impl Node {
    /// The func this node instantiates, or `None` for graph/boundary nodes.
    pub fn func_id(&self) -> Option<FuncId> {
        self.kind.as_func()
    }
}

/// How deep a node lookup reaches: this graph's own nodes only, or also every
/// local nested graph, recursively. The argument to
/// [`Graph::find_node`], [`Graph::find_node_mut`], and
/// [`Graph::find_node_by_name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeSearch {
    TopLevel,
    Recursive,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    pub name: String,
    pub category: String,

    /// Interface in port order. `inputs[i]` corresponds to `GraphInput`
    /// output port `i`; `outputs[j]` corresponds to `GraphOutput` input port
    /// `j`.
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,

    /// Exposed outgoing events re-exported from interior emitters.
    #[serde(default)]
    pub events: Vec<GraphEvent>,

    /// Shared-library graph this value was copied from, if any.
    #[serde(default)]
    pub origin: Option<GraphId>,

    pub(crate) nodes: HashMap<NodeId, Node>,

    /// Data wiring, keyed by consumer input port. Sparse: only bound ports
    /// appear; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and lets a node's ports range contiguously.
    /// Serialized as a sequence of `(port, binding)` pairs — struct keys aren't
    /// valid map keys in string-keyed formats (JSON/TOML).
    #[serde(default, with = "crate::graph::serde")]
    pub bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A
    /// `BTreeSet` dedups, keeps serialization deterministic, and ranges over
    /// one emitter-event's subscribers contiguously.
    #[serde(default)]
    pub(crate) subscriptions: BTreeSet<Subscription>,

    /// Output ports read by a consumer outside the graph (e.g. a GUI
    /// inspector), flagged so the plan counts them as used even with no
    /// in-graph binding. Presence, not a richer value, is the flag — same
    /// sparse-side-table shape as `subscriptions`.
    #[serde(default)]
    pub(crate) pinned_outputs: BTreeSet<OutputPort>,

    /// Local graphs referenced by this graph's `GraphLink::Local` instances.
    /// Shared graphs live in `Library::graphs`.
    #[serde(default)]
    pub graphs: HashMap<GraphId, Graph>,
}

impl Graph {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }

    pub fn input(mut self, input: FuncInput) -> Self {
        self.inputs.push(input);
        self
    }

    pub fn inputs(mut self, inputs: impl IntoIterator<Item = FuncInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }

    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
        self
    }

    pub fn outputs(mut self, outputs: impl IntoIterator<Item = FuncOutput>) -> Self {
        self.outputs.extend(outputs);
        self
    }

    pub fn event(mut self, event: GraphEvent) -> Self {
        self.events.push(event);
        self
    }

    pub fn events(mut self, events: impl IntoIterator<Item = GraphEvent>) -> Self {
        self.events.extend(events);
        self
    }

    pub fn origin(mut self, origin: GraphId) -> Self {
        self.origin = Some(origin);
        self
    }

    pub fn insert_graph(&mut self, graph_id: GraphId, graph: Graph) {
        assert!(!graph_id.is_nil(), "cannot insert a graph with a nil id");
        self.graphs.insert(graph_id, graph);
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let node_id = NodeId::unique();
        self.insert(node_id, node);
        node_id
    }

    pub fn insert(&mut self, node_id: NodeId, node: Node) {
        assert!(!node_id.is_nil(), "cannot add a node with a nil id");
        match self.nodes.entry(node_id) {
            Entry::Vacant(entry) => {
                entry.insert(node);
            }
            Entry::Occupied(_) => {
                panic!("node {node_id:?} already exists; adds must use a fresh id")
            }
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Iteration order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = NodeRef<'_>> {
        self.nodes
            .iter()
            .map(|(id, node)| NodeRef { id: *id, node })
    }

    /// The node with `id`, at the depth `search` selects. Node ids are unique
    /// across a whole document (nested graphs included), so a
    /// [`Recursive`](NodeSearch::Recursive) hit is unambiguous.
    pub fn find(&self, id: &NodeId, search: NodeSearch) -> Option<&Node> {
        assert!(!id.is_nil());
        match self.nodes.get(id) {
            Some(node) => Some(node),
            None => match search {
                NodeSearch::TopLevel => None,
                NodeSearch::Recursive => self
                    .graphs
                    .values()
                    .find_map(|graph| graph.find(id, NodeSearch::Recursive)),
            },
        }
    }

    /// A node named `name`, at the depth `search` selects.
    ///
    /// Names are not unique, so this returns an arbitrary match. Recursive
    /// searches prefer a match in this graph over one in a nested graph.
    pub fn find_by_name(&self, name: &str, search: NodeSearch) -> Option<NodeRef<'_>> {
        assert!(!name.is_empty());
        if let Some((id, node)) = self.nodes.iter().find(|(_, node)| node.name == name) {
            return Some(NodeRef { id: *id, node });
        }

        match search {
            NodeSearch::TopLevel => None,
            NodeSearch::Recursive => self
                .graphs
                .values()
                .find_map(|graph| graph.find_by_name(name, NodeSearch::Recursive)),
        }
    }

    /// Mutable counterpart of [`Self::find_node`].
    pub fn find_mut(&mut self, id: &NodeId, search: NodeSearch) -> Option<&mut Node> {
        assert!(!id.is_nil());
        match search {
            NodeSearch::TopLevel => self.nodes.get_mut(id),
            NodeSearch::Recursive => {
                let Self { nodes, graphs, .. } = self;
                nodes.get_mut(id).or_else(|| {
                    graphs
                        .values_mut()
                        .find_map(|graph| graph.find_mut(id, NodeSearch::Recursive))
                })
            }
        }
    }

    pub fn serialize(&self, format: SerdeFormat) -> Result<Vec<u8>> {
        Ok(serialize(self, format)?)
    }
    pub fn deserialize(serialized: &[u8], format: SerdeFormat) -> Result<Graph> {
        let graph: Self = deserialize(serialized, format)?;
        graph.check()?;
        Ok(graph)
    }

    /// Resolve a graph instance link from this graph or the shared library.
    pub fn resolve_graph<'a>(&'a self, link: GraphLink, library: &'a Library) -> Option<&'a Graph> {
        match link {
            GraphLink::Local(id) => self.graphs.get(&id),
            GraphLink::Shared(id) => library.graphs.get(&id),
        }
    }

    /// Add a func instance and seed its inputs' default const bindings.
    /// Returns the new node id.
    pub fn add_func_node(&mut self, func: &Func) -> NodeId {
        let node = Node::from(func);
        let node_id = self.add(node);
        for (port_idx, func_input) in func.inputs.iter().enumerate() {
            if let Some(default) = &func_input.default_value {
                self.set_input_binding(
                    InputPort::new(node_id, port_idx),
                    Binding::Const(default.clone()),
                );
            }
        }
        node_id
    }

    /// Add a graph instance and seed its inputs' default const bindings.
    pub fn add_graph_node(&mut self, graph: &Graph, link: GraphLink) -> NodeId {
        let node = Node::graph_instance(graph, link);
        let node_id = self.add(node);
        for (port_idx, io) in graph.inputs.iter().enumerate() {
            if let Some(default) = &io.default_value {
                self.set_input_binding(
                    InputPort::new(node_id, port_idx),
                    Binding::Const(default.clone()),
                );
            }
        }
        node_id
    }
}

impl Node {
    /// A fresh node of the given kind with no inputs/events. Callers fill in
    /// wiring, or use `From<&Func>` / `graph_instance` for a node shaped
    /// from its definition. A `Special` node copies its hardcoded func's
    /// `default_cache_mode`; every other kind seeds `None`.
    pub fn new(kind: NodeKind) -> Self {
        let cache = match &kind {
            NodeKind::Special(s) => s.func().default_cache_mode,
            _ => CacheMode::None,
        };
        Node {
            kind,
            name: String::new(),
            cache,
            disabled: false,
        }
    }

    /// A graph instance node shaped from the referenced graph.
    pub fn graph_instance(graph: &Graph, link: GraphLink) -> Self {
        Node {
            kind: NodeKind::Graph(link),
            name: graph.name.clone(),
            cache: CacheMode::None,
            disabled: false,
        }
    }
}

impl From<&Func> for Node {
    /// A bare func instance copying the func's `default_cache_mode` into its
    /// `cache`. Default input bindings are seeded by `Graph::add_func_node`.
    fn from(func: &Func) -> Self {
        Node {
            kind: NodeKind::Func(func.id),
            name: func.name.clone(),
            cache: func.default_cache_mode,
            disabled: false,
        }
    }
}

impl Binding {
    /// A data binding wired to producer `node_id`'s output port `port_idx`.
    pub fn bind(node_id: NodeId, port_idx: usize) -> Self {
        Binding::Bind(OutputPort::new(node_id, port_idx))
    }

    pub fn as_output_binding(&self) -> Option<&OutputPort> {
        match self {
            Binding::Bind(output_binding) => Some(output_binding),
            _ => None,
        }
    }
}

impl From<OutputPort> for Binding {
    fn from(value: OutputPort) -> Self {
        Binding::Bind(value)
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
