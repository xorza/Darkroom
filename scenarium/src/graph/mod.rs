//! The authoring graph: the [`Graph`] type, the satellite types it owns
//! ([`Node`], [`Binding`], [`CacheMode`], …), and **every method a `Graph`
//! has**, in one impl — reads and mutations alike, so the whole surface is one
//! file to read rather than six.
//!
//! The modules beside it keep what those methods work *with*: [`identity`]
//! how a node or port is named, [`detached`] the reversible-removal records,
//! [`error`] what validation rejects, [`interface`] the ports a node declares,
//! [`definition`] the reusable `GraphDef`.

use std::collections::{BTreeMap, BTreeSet};

use ::serde::{Deserialize, Serialize};
use common::{SerdeFormat, SerializeError, deserialize, is_debug, serialize};
use hashbrown::hash_map::Entry;
use hashbrown::{HashMap, HashSet};

use crate::DataType;
use crate::StaticValue;
use crate::graph::definition::{GraphDef, GraphLink};
use crate::graph::detached::{DetachedGraphInput, DetachedGraphOutput, DetachedNode};
use crate::graph::error::ValidationResult;
use crate::graph::error::{GraphDeserializeError, GraphValidationError};
use crate::graph::func::{Func, FuncInput, FuncOutput, OutputType};
use crate::graph::identity::NodeId;
use crate::graph::identity::{GraphId, InputPort, OutputPort, Subscription};
use crate::graph::interface::NodePorts;
use crate::graph::node::output_resolver::{OutputResolver, OutputSource};
use crate::graph::node::{Node, NodeKind, NodeRef, NodeSearch};
use crate::library::Library;

pub(crate) mod definition;
pub(crate) mod detached;
pub(crate) mod error;
pub(crate) mod func;
pub(crate) mod identity;
pub(crate) mod interface;
pub(crate) mod node;
mod serde;

/// Hard cap on graph-tree nesting. Enforced as a validation error (the
/// recursive walk is itself stack-bound), and re-asserted as a release
/// backstop in flatten's descent.
pub(crate) const MAX_NESTING_DEPTH: usize = 256;

/// What a consumer input port is wired to. Stored sparsely: `Graph.bindings`
/// only holds these values, and an absent port is unbound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    Const(StaticValue),
    Bind(OutputPort),
}

// A node is pure authored data. Identity is its key in `Graph::nodes`; port/event
// arity comes from the func or graph interface, and wiring lives in side tables.
/// Deliberately not `Clone`: node and graph ids are unique across a whole
/// document, so duplicating a graph is never a neutral act. Clone through
/// [`Graph::clone_mapped`] (identities remapped) or
/// [`Graph::clone_verbatim`] (identities preserved — undo/redo replay and
/// library composition only).
///
/// The side tables split on how they're reached, not on how sensitive they
/// are: `bindings` and `graphs` are plain maps a caller keys into directly, so
/// they're public rather than fronted by trivial accessors; `nodes` stays
/// `pub(crate)` for cross-module test call sites that force every node's
/// cache mode directly, while `subscriptions` is private — every real way in
/// (fresh-id insertion, recursive lookup, a range query) already goes through a
/// method — `pub(super)` so the wiring and validation passes beside this file
/// can walk it, and no wider.
#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
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
    pub(super) subscriptions: BTreeSet<Subscription>,

    /// Local graph definitions referenced by this graph's `GraphLink::Local`
    /// instances. Shared definitions live in `Library::graphs`.
    #[serde(default)]
    pub graphs: HashMap<GraphId, GraphDef>,
}

/// One data edge as a value: the consumer port, and what it was bound to.
///
/// The unit every reversible edit collects severed wiring into — a
/// A [`DetachedNode`] carries the edges that
/// touched its node, a detached interface port the ones its slot fed — so an
/// attach restores exactly what a detach took.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub port: InputPort,
    pub binding: Binding,
}

/// A remapped clone alongside the node mapping that produced it, so a caller
/// holding ids into the original — a definition's exposed-event emitters —
/// can follow them across the copy. What
/// [`Graph::clone_mapped_with_ids`] hands back.
#[derive(Debug)]
pub(super) struct MappedClone {
    pub graph: Graph,
    pub node_ids: HashMap<NodeId, NodeId>,
}

/// Which way a slot's removal or restoration renumbers the ports around it.
///
/// Detaching or attaching one interface port moves every port above it, and
/// the two directions are exact inverses — so this is one implementation both
/// use, and `Graph`'s four slot mutations differ only in which side they apply
/// it to. An interface *input* is an output slot on the child's inbound
/// boundary node and an input slot on every instance; an interface *output* is
/// the exact swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shift {
    /// A slot was removed: every port above it moves down one.
    Down,
    /// A slot is being restored: it and every port above it move up one.
    Up,
}

impl Shift {
    /// The lowest port index this shift moves, for a slot at `idx`.
    fn first_moved(self, idx: usize) -> usize {
        match self {
            Shift::Down => idx + 1,
            Shift::Up => idx,
        }
    }

    /// Where the port at `port_idx` lands.
    fn apply(self, port_idx: usize) -> usize {
        match self {
            Shift::Down => port_idx - 1,
            Shift::Up => port_idx + 1,
        }
    }

    /// Reorder ports collected ascending so each destination is free when it's
    /// written: compacting down writes low-to-high, opening up writes
    /// high-to-low.
    fn order<T>(self, ports: &mut [T]) {
        if self == Shift::Up {
            ports.reverse();
        }
    }
}

impl Graph {
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
    pub fn find(&self, id: NodeId, search: NodeSearch) -> Option<&Node> {
        assert!(!id.is_nil());
        match self.nodes.get(&id) {
            Some(node) => Some(node),
            None => match search {
                NodeSearch::TopLevel => None,
                NodeSearch::Recursive => self
                    .graphs
                    .values()
                    .find_map(|def| def.body.find(id, NodeSearch::Recursive)),
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
                .find_map(|def| def.body.find_by_name(name, NodeSearch::Recursive)),
        }
    }
    pub fn serialize(&self, format: SerdeFormat) -> Result<Vec<u8>, SerializeError> {
        serialize(self, format)
    }
    /// The graph whose `graphs` map holds `id` — the *owner* boundary ops
    /// are called on — searching the whole nested tree. `self` when the
    /// def is a direct child here. Graph ids are unique across a document
    /// (like node ids), so a hit is unambiguous.
    pub fn find_graph_parent(&self, id: GraphId) -> Option<&Graph> {
        if self.graphs.contains_key(&id) {
            return Some(self);
        }
        self.graphs
            .values()
            .find_map(|def| def.body.find_graph_parent(id))
    }
    /// The local definition `id` anywhere in this graph's nested tree —
    /// unambiguous for the same reason as [`Self::find_graph_parent`].
    pub fn find_graph(&self, id: GraphId) -> Option<&GraphDef> {
        self.find_graph_parent(id).map(|parent| &parent.graphs[&id])
    }

    /// The ports `node` declares, or `None` for a boundary node (its arity
    /// mirrors the enclosing interface, which a bare body doesn't carry) or an
    /// unresolved func/graph reference. The one place a node kind is mapped to
    /// its declaration.
    pub fn node_ports<'a>(&'a self, node: &'a Node, library: &'a Library) -> Option<NodePorts<'a>> {
        match &node.kind {
            NodeKind::Func(func_id) => Some(library.by_id(*func_id)?.ports()),
            NodeKind::Graph(link) => Some(self.resolve_graph(*link, library)?.ports()),
            NodeKind::Special(special) => Some(special.func().ports()),
            NodeKind::GraphInput | NodeKind::GraphOutput => None,
        }
    }
    /// The declared type of input `port`, or `None` when it can't be resolved —
    /// a boundary node (its arity mirrors the enclosing interface, with no
    /// per-port type here) or a missing func/graph. The caller treats `None` as
    /// the polymorphic `Any`.
    pub(super) fn input_type(&self, library: &Library, port: InputPort) -> Option<DataType> {
        self.input_spec(library, port).map(|i| i.data_type.clone())
    }
    /// The declared [`FuncInput`] of input `port` — its full spec (type +
    /// `value_variants` + flags), or `None` for a boundary / unresolved node.
    /// Resolution mirrors [`Self::input_type`].
    pub(super) fn input_spec<'a>(
        &'a self,
        library: &'a Library,
        port: InputPort,
    ) -> Option<&'a FuncInput> {
        let node = self.find(port.node_id, NodeSearch::TopLevel)?;
        self.node_ports(node, library)?.inputs.get(port.port_idx)
    }
    /// The effective type produced at output `port`, following *wildcard*
    /// outputs up the graph: a wildcard output (e.g. a passthrough / reroute —
    /// see [`OutputType::Wildcard`](crate::graph::func::OutputType))
    /// reports the resolved type of whatever feeds the input it mirrors, so a
    /// value's type survives the hop: a `Bind` follows the producer up, while a
    /// `Const` takes the mirrored input's declared type (which carries the full
    /// `FsPathConfig` / `Enum` id), or the const's own scalar type on a wildcard
    /// (`Any`-declared) input. The walk dead-ends polymorphic (`Any`) when the
    /// mirrored input is unbound, the producer is a boundary or missing, or a
    /// binding cycle is hit. Used by the editor for port-type display, connection
    /// compatibility, graph-interface inference, and compile-time validation.
    pub fn resolve_output_type(&self, library: &Library, port: OutputPort) -> DataType {
        OutputResolver::new().resolve(port, &|output| self.output_source(library, output))
    }
    fn output_source(&self, library: &Library, port: OutputPort) -> OutputSource<OutputPort> {
        let Some(out) = self.output_spec(library, port) else {
            return OutputSource::Unresolved;
        };
        let OutputType::Wildcard { mirrors } = &out.ty else {
            return OutputSource::Fixed(out.ty.declared());
        };
        let mirror = InputPort::new(port.node_id, *mirrors);
        match self.bindings.get(&mirror) {
            Some(Binding::Bind(source)) => OutputSource::Bind(*source),
            Some(Binding::Const(value)) => OutputSource::Const {
                declared: self.input_type(library, mirror).unwrap_or_default(),
                value: value.clone(),
            },
            None => OutputSource::Unresolved,
        }
    }
    /// The declared [`FuncOutput`] of output `port` — its name + [`OutputType`],
    /// or `None` for a boundary / unresolved node. The output-side mirror of
    /// [`Self::input_spec`].
    fn output_spec<'a>(&'a self, library: &'a Library, port: OutputPort) -> Option<&'a FuncOutput> {
        let node = self.find(port.node_id, NodeSearch::TopLevel)?;
        self.node_ports(node, library)?.outputs.get(port.port_idx)
    }
    /// Every data edge as (consumer input ← producer output). Const bindings
    /// are not edges and are skipped.
    pub fn edges(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(dst, binding)| match binding {
                Binding::Bind(src) => Some((*dst, *src)),
                _ => None,
            })
    }
    /// Whether binding an output of `producer` into an input on `consumer`
    /// would close a directed data cycle. A node wired to itself counts.
    ///
    /// The planner rejects a cyclic graph outright
    /// ([`Error::CycleDetected`](crate::Error)), so this is what keeps one from
    /// being authored at all: the editor asks before it commits a bind. Port
    /// indices don't enter into it — a cycle is a property of the node graph,
    /// and any output of `producer` reaching any input of `consumer` closes the
    /// same loop.
    ///
    /// Walks forward from `consumer` over the edges already there; reaching
    /// `producer` means the proposed one closes the loop. Const bindings aren't
    /// edges, so they can't take part.
    pub fn produces_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut readers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (destination, source) in self.edges() {
            readers
                .entry(source.node_id)
                .or_default()
                .push(destination.node_id);
        }
        let mut stack = vec![consumer];
        let mut seen: HashSet<NodeId> = HashSet::from_iter([consumer]);
        while let Some(node) = stack.pop() {
            for &next in readers.get(&node).into_iter().flatten() {
                if next == producer {
                    return true;
                }
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        false
    }
    /// Clone with every node *and* nested-graph identity remapped to a fresh
    /// one, throughout the local graph tree. Both id kinds are unique across
    /// a whole document, so every copy boundary must sever both; `Local`
    /// links are rewritten per level (a node references only its own graph's
    /// map — resolution is parent-scoped). `Shared` links name library graphs
    /// and stay as-is.
    pub fn clone_mapped(&self) -> Graph {
        self.clone_mapped_with_ids().graph
    }

    pub(super) fn clone_mapped_with_ids(&self) -> MappedClone {
        let mut node_ids = HashMap::with_capacity(self.nodes.len());
        let graph_id_map: HashMap<GraphId, GraphId> = self
            .graphs
            .keys()
            .map(|graph_id| (*graph_id, GraphId::unique()))
            .collect();
        let mut nodes = HashMap::with_capacity(self.nodes.len());
        for (node_id, node) in &self.nodes {
            let new_id = NodeId::unique();
            node_ids.insert(*node_id, new_id);
            let mut node = node.clone();
            // A dangling link (def already missing) keeps its old id —
            // drift tolerance, same as everywhere else.
            if let NodeKind::Graph(GraphLink::Local(graph_id)) = &mut node.kind
                && let Some(new_graph_id) = graph_id_map.get(graph_id)
            {
                *graph_id = *new_graph_id;
            }
            nodes.insert(new_id, node);
        }
        let remap = |id: NodeId| node_ids.get(&id).copied().unwrap_or(id);
        let bindings = self
            .bindings
            .iter()
            .map(|(port, binding)| {
                let port = InputPort::new(remap(port.node_id), port.port_idx);
                let binding = match binding {
                    Binding::Bind(output) => Binding::bind(remap(output.node_id), output.port_idx),
                    other => other.clone(),
                };
                (port, binding)
            })
            .collect();
        let subscriptions = self
            .subscriptions
            .iter()
            .map(|subscription| Subscription {
                emitter: remap(subscription.emitter),
                event_idx: subscription.event_idx,
                subscriber: remap(subscription.subscriber),
            })
            .collect();
        let graphs = self
            .graphs
            .iter()
            .map(|(graph_id, def)| (graph_id_map[graph_id], def.clone_mapped()))
            .collect();
        let graph = Graph {
            nodes,
            bindings,
            subscriptions,
            graphs,
        };
        MappedClone { graph, node_ids }
    }
    /// Clone keeping every identity — node ids and nested graph ids —
    /// exactly as they are. The counterpart to [`Self::clone_mapped`], and
    /// the reason `Graph` isn't `Clone`: preserving identities is only sound
    /// where the original is *not* concurrently present in the same
    /// document, i.e. undo/redo replay of a stored step and library
    /// composition. Anywhere else, use `clone_mapped`.
    pub fn clone_verbatim(&self) -> Graph {
        Graph {
            nodes: self.nodes.clone(),
            bindings: self.bindings.clone(),
            subscriptions: self.subscriptions.clone(),
            graphs: self
                .graphs
                .iter()
                .map(|(graph_id, def)| (*graph_id, def.clone_verbatim()))
                .collect(),
        }
    }

    pub fn snapshot_node(&self, node_id: NodeId) -> Option<DetachedNode> {
        let node = self.find(node_id, NodeSearch::TopLevel)?.clone();
        Some(DetachedNode {
            node_id,
            node,
            bindings: self.bindings_touching(node_id),
            subscriptions: self
                .subscriptions
                .iter()
                .copied()
                .filter(|subscription| subscription.touches(node_id))
                .collect(),
        })
    }
    pub fn is_subscribed(&self, emitter: NodeId, event_idx: usize, subscriber: NodeId) -> bool {
        self.subscriptions.contains(&Subscription {
            emitter,
            event_idx,
            subscriber,
        })
    }
    pub fn bindings_touching(&self, node_id: NodeId) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(port, binding)| binding.touches(**port, node_id))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }
    pub fn subscriptions(&self) -> impl Iterator<Item = Subscription> + '_ {
        self.subscriptions.iter().copied()
    }
    pub fn subscribers(
        &self,
        emitter: NodeId,
        event_idx: usize,
    ) -> impl Iterator<Item = NodeId> + '_ {
        let lower = Subscription {
            emitter,
            event_idx,
            subscriber: NodeId::nil(),
        };
        let upper = Subscription {
            emitter,
            event_idx: event_idx + 1,
            subscriber: NodeId::nil(),
        };
        self.subscriptions
            .range(lower..upper)
            .map(|subscription| subscription.subscriber)
    }

    /// Resolve a graph instance link from this graph or the shared library.
    pub fn resolve_graph<'a>(
        &'a self,
        link: GraphLink,
        library: &'a Library,
    ) -> Option<&'a GraphDef> {
        match link {
            GraphLink::Local(id) => self.graphs.get(&id),
            GraphLink::Shared(id) => library.graphs.get(&id),
        }
    }

    /// Insert a local definition, *replacing* any existing one under `graph_id`
    /// — map semantics, which undo/redo replay depends on when it re-applies a
    /// recorded step. Contrast
    /// [`Library::register_graph`](crate::library::Library::register_graph),
    /// which refuses a duplicate. Document-wide graph-id uniqueness is
    /// [`Graph::validate`]'s job, not this call's.
    pub fn insert_graph(&mut self, graph_id: GraphId, graph: GraphDef) {
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

    /// Mutable counterpart of [`Self::find`].
    pub fn find_mut(&mut self, id: NodeId, search: NodeSearch) -> Option<&mut Node> {
        assert!(!id.is_nil());
        match search {
            NodeSearch::TopLevel => self.nodes.get_mut(&id),
            NodeSearch::Recursive => {
                let Self { nodes, graphs, .. } = self;
                nodes.get_mut(&id).or_else(|| {
                    graphs
                        .values_mut()
                        .find_map(|def| def.body.find_mut(id, NodeSearch::Recursive))
                })
            }
        }
    }

    pub fn deserialize(
        serialized: &[u8],
        format: SerdeFormat,
    ) -> Result<Graph, GraphDeserializeError> {
        let graph: Self = deserialize(serialized, format)?;
        graph.validate()?;
        Ok(graph)
    }

    /// Mutable counterpart of [`Self::find_graph_parent`].
    pub fn find_graph_parent_mut(&mut self, id: GraphId) -> Option<&mut Graph> {
        if self.graphs.contains_key(&id) {
            return Some(self);
        }
        self.graphs
            .values_mut()
            .find_map(|def| def.body.find_graph_parent_mut(id))
    }

    /// Mutable counterpart of [`Self::find_graph`].
    pub fn find_graph_mut(&mut self, id: GraphId) -> Option<&mut GraphDef> {
        self.find_graph_parent_mut(id)
            .map(|parent| parent.graphs.get_mut(&id).unwrap())
    }

    /// Add a func instance and seed its inputs' default const bindings.
    /// Returns the new node id.
    pub fn add_func_node(&mut self, func: &Func) -> NodeId {
        self.add_instance(Node::from(func), func.ports())
    }

    /// Add a graph instance and seed its inputs' default const bindings.
    pub fn add_graph_node(&mut self, def: &GraphDef, link: GraphLink) -> NodeId {
        self.add_instance(Node::graph_instance(def, link), def.ports())
    }

    /// Add `node` and seed the const bindings its declaration defaults to.
    fn add_instance(&mut self, node: Node, ports: NodePorts<'_>) -> NodeId {
        let node_id = self.add(node);
        self.bindings.extend(ports.default_bindings(node_id));
        node_id
    }

    pub fn detach_node(&mut self, node_id: NodeId) -> DetachedNode {
        assert!(!node_id.is_nil());
        let detached = self
            .snapshot_node(node_id)
            .expect("cannot detach a node that is not in the graph");
        self.nodes.remove(&node_id);
        self.bindings
            .retain(|port, binding| !binding.touches(*port, node_id));
        self.subscriptions
            .retain(|subscription| !subscription.touches(node_id));
        detached
    }
    pub fn attach_node(&mut self, detached: DetachedNode) {
        detached.assert_valid();
        assert!(
            !self.nodes.contains_key(&detached.node_id),
            "cannot attach a node that is already in the graph"
        );
        assert!(
            detached
                .bindings
                .iter()
                .all(|entry| !self.bindings.contains_key(&entry.port)),
            "cannot attach over bindings created after detachment"
        );
        assert!(
            detached
                .subscriptions
                .iter()
                .all(|subscription| !self.subscriptions.contains(subscription)),
            "cannot attach over subscriptions created after detachment"
        );

        let DetachedNode {
            node_id,
            node,
            bindings,
            subscriptions,
        } = detached;
        self.insert(node_id, node);
        self.bindings.extend(
            bindings
                .into_iter()
                .map(|entry| (entry.port, entry.binding)),
        );
        self.subscriptions.extend(subscriptions);
    }
    pub fn set_input_binding(&mut self, port: InputPort, binding: impl Into<Option<Binding>>) {
        match binding.into() {
            Some(binding) => {
                self.bindings.insert(port, binding);
            }
            None => {
                self.bindings.remove(&port);
            }
        }
    }
    pub fn subscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.insert(Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }
    pub fn unsubscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.remove(&Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }

    /// Remove subgraph input `idx` from local child `graph_id`: drop its
    /// spec, sever the interior edges on the boundary output, drop
    /// every instance binding into the slot, and shift the ports above it
    /// down by one on both sides. Returns the removed state for
    /// [`Self::attach_graph_input`].
    pub fn detach_graph_input(&mut self, graph_id: GraphId, idx: usize) -> DetachedGraphInput {
        let detached = self
            .snapshot_graph_input(graph_id, idx)
            .expect("cannot detach a graph input that does not exist");
        let instances = self.local_instances(graph_id);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.inputs.remove(idx);
        if let Some(boundary) = child.body.boundary_node(NodeKind::GraphInput) {
            child.body.remove_output_slot(boundary, idx);
        }
        for instance in instances {
            self.remove_input_slot(instance, idx);
        }
        detached
    }
    /// Exact inverse of [`Self::detach_graph_input`]: shift ports back up
    /// and restore the spec, interior edges, and instance bindings.
    /// Panics on a malformed record — one whose wiring doesn't reference the
    /// detached slot, or that overlaps wiring created after detachment.
    pub fn attach_graph_input(&mut self, graph_id: GraphId, detached: DetachedGraphInput) {
        let instances = self.local_instances(graph_id);
        let child = self
            .graphs
            .get(&graph_id)
            .expect("cannot attach a graph input to a missing graph");
        let boundary = child.body.boundary_node(NodeKind::GraphInput);
        assert!(
            detached.idx <= child.inputs.len(),
            "attach index out of range"
        );
        detached.assert_targets_slot(&instances, boundary);
        // Still in the validation phase: `insert_output_slot` renumbers
        // bound *values*, not consumer-keyed ports, so these targets are
        // exactly as occupied now as when the restore reaches them.
        //
        // Deliberately not the parent side: `insert_input_slot` shifts
        // every key at `idx` and above up by one, so the parent restore
        // always lands on a slot that shift has just vacated — checking
        // it against pre-shift occupancy would refuse every valid attach.
        child.body.assert_restorable(&detached.interior);

        let DetachedGraphInput {
            idx,
            spec,
            interior,
            parent,
        } = detached;
        for instance in &instances {
            self.insert_input_slot(*instance, idx);
        }
        self.restore_bindings(parent);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.inputs.insert(idx, spec);
        if let Some(boundary) = boundary {
            child.body.insert_output_slot(boundary, idx);
        }
        child.body.restore_bindings(interior);
    }
    /// Remove subgraph output `idx` from local child `graph_id` — the
    /// output-side mirror of [`Self::detach_graph_input`].
    pub fn detach_graph_output(&mut self, graph_id: GraphId, idx: usize) -> DetachedGraphOutput {
        let detached = self
            .snapshot_graph_output(graph_id, idx)
            .expect("cannot detach a graph output that does not exist");
        let instances = self.local_instances(graph_id);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.outputs.remove(idx);
        if let Some(boundary) = child.body.boundary_node(NodeKind::GraphOutput) {
            child.body.remove_input_slot(boundary, idx);
        }
        for instance in instances {
            self.remove_output_slot(instance, idx);
        }
        detached
    }
    /// Exact inverse of [`Self::detach_graph_output`]. Panics on a malformed
    /// record — one whose wiring doesn't reference the detached slot, or
    /// that overlaps wiring created after detachment.
    pub fn attach_graph_output(&mut self, graph_id: GraphId, detached: DetachedGraphOutput) {
        let instances = self.local_instances(graph_id);
        let child = self
            .graphs
            .get(&graph_id)
            .expect("cannot attach a graph output to a missing graph");
        let boundary = child.body.boundary_node(NodeKind::GraphOutput);
        assert!(
            detached.idx <= child.outputs.len(),
            "attach index out of range"
        );
        detached.assert_targets_slot(&instances, boundary);
        // Mirror of the input side, with the two halves swapped: here it
        // is the *parent* consumers whose keys `insert_output_slot`
        // leaves alone (it renumbers bound values), while the interior
        // entry lands on `(boundary, idx)`, which `insert_input_slot`
        // always vacates.
        self.assert_restorable(&detached.parent);

        let DetachedGraphOutput {
            idx,
            spec,
            interior,
            parent,
        } = detached;
        for instance in &instances {
            self.insert_output_slot(*instance, idx);
        }
        self.restore_bindings(parent);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.outputs.insert(idx, spec);
        if let Some(boundary) = boundary {
            child.body.insert_input_slot(boundary, idx);
        }
        child.body.restore_bindings(interior);
    }
    /// Sever output port `(node, idx)`'s consumers and compact the ports above
    /// it down onto the freed slot.
    fn remove_output_slot(&mut self, node: NodeId, idx: usize) {
        let slot = OutputPort::new(node, idx);
        self.bindings
            .retain(|_, binding| !matches!(binding, Binding::Bind(src) if *src == slot));
        self.shift_bound_values(node, idx, Shift::Down);
    }
    /// Open a free output slot at `(node, idx)`; the caller restores the
    /// wiring it carried.
    fn insert_output_slot(&mut self, node: NodeId, idx: usize) {
        self.shift_bound_values(node, idx, Shift::Up);
    }
    /// Drop the binding on input port `(node, idx)` and compact the ports
    /// above it down onto the freed slot.
    fn remove_input_slot(&mut self, node: NodeId, idx: usize) {
        self.bindings.remove(&InputPort::new(node, idx));
        self.shift_binding_keys(node, idx, Shift::Down);
    }
    /// Open a free input slot at `(node, idx)`.
    fn insert_input_slot(&mut self, node: NodeId, idx: usize) {
        self.shift_binding_keys(node, idx, Shift::Up);
    }
    /// Put severed bindings back, refusing to overwrite wiring authored after
    /// the detachment.
    ///
    /// **Every port is checked before any is written.** Inserting first
    /// meant the newer binding was already gone by the time the assert
    /// fired, and the entries restored ahead of the conflicting one
    /// stayed applied — an attach that "refused" had still destroyed the
    /// wiring it was refusing to overwrite.
    ///
    /// Each caller pre-flights the *other* half of its record through
    /// [`Self::assert_restorable`] before it shifts any slot; this call is
    /// what covers the half whose ports only that shift vacates.
    ///
    /// Takes the `Vec` rather than an `IntoIterator` because the two
    /// passes need it twice, and both call sites already own one.
    fn restore_bindings(&mut self, entries: Vec<BindingEntry>) {
        self.assert_restorable(&entries);
        for entry in entries {
            self.bindings.insert(entry.port, entry.binding);
        }
    }
    /// Renumber binding *values* `Bind(node, j)` around slot `idx`.
    fn shift_bound_values(&mut self, node: NodeId, idx: usize, shift: Shift) {
        let first_moved = shift.first_moved(idx);
        for binding in self.bindings.values_mut() {
            if let Binding::Bind(src) = binding
                && src.node_id == node
                && src.port_idx >= first_moved
            {
                src.port_idx = shift.apply(src.port_idx);
            }
        }
    }
    /// Rekey bindings *on* `(node, j)` around slot `idx`.
    fn shift_binding_keys(&mut self, node: NodeId, idx: usize, shift: Shift) {
        let mut ports: Vec<InputPort> = self
            .bindings
            .range(InputPort::new(node, shift.first_moved(idx))..=InputPort::new(node, usize::MAX))
            .map(|(port, _)| *port)
            .collect();
        shift.order(&mut ports);
        for port in ports {
            let binding = self.bindings.remove(&port).unwrap();
            self.bindings
                .insert(InputPort::new(node, shift.apply(port.port_idx)), binding);
        }
    }

    /// Validate this graph and its complete local graph tree, structurally:
    /// everything a document must satisfy to *be* a graph, answerable without a
    /// library. This is what deserialization checks, where no library exists
    /// yet.
    pub fn validate(&self) -> ValidationResult<()> {
        self.validate_shape(&mut HashSet::new(), &mut HashSet::new(), 0, true)
    }

    /// Debug-only assert form of [`Self::validate`].
    pub fn validate_debug(&self) {
        if !is_debug() {
            return;
        }
        self.validate()
            .expect("graph structural invariant violated");
    }

    /// Validate this graph as an execution entry against `library`: structurally
    /// first, then every reference it makes.
    ///
    /// Two passes rather than one interleaved walk, because they answer
    /// different questions and only one of them can always be asked. Structure
    /// is what the document must satisfy to be a graph at all; this adds what it
    /// takes to compile one against a *particular* library, which the same
    /// document may pass today and fail tomorrow. Running structure first also
    /// lets the second pass assume it: every id it reads names a node that is
    /// there.
    pub fn validate_with(&self, library: &Library) -> ValidationResult<()> {
        if self.nodes.values().any(|node| node.kind.is_boundary()) {
            return Err(GraphValidationError::EntryBoundaryNodes);
        }
        self.validate()?;
        self.validate_references(library, &mut HashSet::new(), &mut HashSet::new(), 0, true)
    }

    /// Debug-only assert form of [`Self::validate_with`].
    pub fn validate_with_debug(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.validate_with(library)
            .expect("graph execution invariant violated");
    }

    /// The structural walk. `node_ids` and `graph_ids` accumulate across the
    /// whole reachable tree — both must be unique document-wide, so a bare id is
    /// an unambiguous address — while `depth` bounds a walk that is itself
    /// stack-bound.
    ///
    /// `entry` marks the execution entry graph, the one place a func declaring
    /// `entry_only` may live. Passed rather than derived from `depth`:
    /// [`GraphDef::validate`] enters at depth 0 for the definition's own body.
    pub(super) fn validate_shape(
        &self,
        node_ids: &mut HashSet<NodeId>,
        graph_ids: &mut HashSet<GraphId>,
        depth: usize,
        entry: bool,
    ) -> ValidationResult<()> {
        if depth > MAX_NESTING_DEPTH {
            return Err(GraphValidationError::NestingTooDeep {
                max: MAX_NESTING_DEPTH,
            });
        }
        let mut boundary_inputs = 0usize;
        let mut boundary_outputs = 0usize;
        for (node_id, node) in &self.nodes {
            if node_id.is_nil() {
                return Err(GraphValidationError::NilNodeId);
            }
            if !node_ids.insert(*node_id) {
                return Err(GraphValidationError::DuplicateNodeId { node_id: *node_id });
            }
            match &node.kind {
                NodeKind::Func(func_id) => {
                    if func_id.is_nil() {
                        return Err(GraphValidationError::NilFuncId { node_id: *node_id });
                    }
                }
                NodeKind::Graph(link) => {
                    if link.id().is_nil() {
                        return Err(GraphValidationError::NilGraphId { node_id: *node_id });
                    }
                    if let GraphLink::Local(graph_id) = link
                        && !self.graphs.contains_key(graph_id)
                    {
                        return Err(GraphValidationError::MissingLocalGraph {
                            node_id: *node_id,
                            graph_id: *graph_id,
                        });
                    }
                }
                NodeKind::Special(special) => {
                    let func = special.func();
                    if func.entry_only && !entry {
                        return Err(GraphValidationError::EntryOnlyFunc {
                            node_id: *node_id,
                            func_id: func.id,
                        });
                    }
                }
                NodeKind::GraphInput => boundary_inputs += 1,
                NodeKind::GraphOutput => boundary_outputs += 1,
            }
        }
        if boundary_inputs > 1 {
            return Err(GraphValidationError::MultipleGraphInputs {
                count: boundary_inputs,
            });
        }
        if boundary_outputs > 1 {
            return Err(GraphValidationError::MultipleGraphOutputs {
                count: boundary_outputs,
            });
        }

        // Drift is tolerated everywhere below: a binding, subscription, or
        // pin referencing a port the current library no longer declares —
        // and a wire or const whose type no longer matches — stays valid
        // and degrades to unbound at flatten time (a required input
        // surfaces as a missing-input verdict; see flatten's `typed_binding`).
        // Deleting or rejecting it would destroy authored wiring that
        // revives when the library or the upstream types come back.
        for (destination, binding) in &self.bindings {
            if !self.nodes.contains_key(&destination.node_id) {
                return Err(GraphValidationError::BindingMissingNode {
                    node_id: destination.node_id,
                });
            }
            if let Binding::Bind(src) = binding
                && !self.nodes.contains_key(&src.node_id)
            {
                return Err(GraphValidationError::BindingMissingProducer {
                    destination: *destination,
                    producer: *src,
                });
            }
        }

        for subscription in &self.subscriptions {
            if !self.nodes.contains_key(&subscription.emitter) {
                return Err(GraphValidationError::MissingSubscriptionEmitter {
                    node_id: subscription.emitter,
                });
            }
            if !self.nodes.contains_key(&subscription.subscriber) {
                return Err(GraphValidationError::MissingSubscriber {
                    emitter: subscription.emitter,
                    event_idx: subscription.event_idx,
                    subscriber: subscription.subscriber,
                });
            }
        }

        for (graph_id, nested) in &self.graphs {
            if graph_id.is_nil() {
                return Err(GraphValidationError::NilLocalGraphId);
            }
            if !graph_ids.insert(*graph_id) {
                return Err(GraphValidationError::DuplicateGraphId {
                    graph_id: *graph_id,
                });
            }
            nested
                .validate_shape(node_ids, graph_ids, depth + 1)
                .map_err(|source| GraphValidationError::LocalGraph {
                    name: nested.name.clone(),
                    source: Box::new(source),
                })?;
        }

        Ok(())
    }

    /// Everything in this graph that names a declaration: funcs resolve, graph
    /// links resolve, and a `Bind` does not sit on a `const_only` input.
    ///
    /// Recurses into local definitions through their own
    /// [`validate_references`](GraphDef::validate_references) rather than their
    /// full validation — the structural pass already walked them, and walking
    /// them again would re-insert their ids and report every one as a
    /// duplicate. A *shared* definition is the opposite case: nothing has seen
    /// it, so it gets both passes.
    pub(super) fn validate_references(
        &self,
        library: &Library,
        validated: &mut HashSet<GraphId>,
        path: &mut HashSet<GraphId>,
        depth: usize,
        entry: bool,
    ) -> ValidationResult<()> {
        if depth > MAX_NESTING_DEPTH {
            return Err(GraphValidationError::NestingTooDeep {
                max: MAX_NESTING_DEPTH,
            });
        }
        for (node_id, node) in &self.nodes {
            match &node.kind {
                NodeKind::Func(func_id) => match library.by_id(*func_id) {
                    None => {
                        return Err(GraphValidationError::MissingFunc {
                            node_id: *node_id,
                            func_id: *func_id,
                        });
                    }
                    Some(func) if func.entry_only && !entry => {
                        return Err(GraphValidationError::EntryOnlyFunc {
                            node_id: *node_id,
                            func_id: *func_id,
                        });
                    }
                    Some(_) => {}
                },
                NodeKind::Graph(link) => {
                    let nested = self
                        .resolve_graph(*link, library)
                        .ok_or(GraphValidationError::MissingGraph { node_id: *node_id })?;
                    if let GraphLink::Shared(id) = link {
                        nested.validate_shared(*id, library, validated, path, depth)?;
                    }
                }
                NodeKind::Special(_) | NodeKind::GraphInput | NodeKind::GraphOutput => {}
            }
        }

        for (destination, binding) in &self.bindings {
            if matches!(binding, Binding::Bind(_))
                && self
                    .input_spec(library, *destination)
                    .is_some_and(|input| input.const_only)
            {
                return Err(GraphValidationError::ConstOnlyBinding { port: *destination });
            }
        }

        for nested in self.graphs.values() {
            nested
                .validate_references(library, validated, path, depth + 1)
                .map_err(|source| GraphValidationError::LocalGraph {
                    name: nested.name.clone(),
                    source: Box::new(source),
                })?;
        }

        Ok(())
    }

    /// What [`Self::detach_graph_input`] of `(graph_id, idx)` would remove —
    /// pure, for undo capture. `None` when the child graph, its definition,
    /// or the slot doesn't exist.
    pub fn snapshot_graph_input(
        &self,
        graph_id: GraphId,
        idx: usize,
    ) -> Option<DetachedGraphInput> {
        let child = self.graphs.get(&graph_id)?;
        let spec = child.inputs.get(idx)?.clone();
        let interior = match child.body.boundary_node(NodeKind::GraphInput) {
            Some(boundary) => child.body.output_slot_consumers(boundary, idx),
            None => Vec::new(),
        };
        let parent = self
            .local_instances(graph_id)
            .into_iter()
            .filter_map(|instance| self.input_slot_binding(instance, idx))
            .collect();
        Some(DetachedGraphInput {
            idx,
            spec,
            interior,
            parent,
        })
    }

    /// What [`Self::detach_graph_output`] of `(graph_id, idx)` would remove —
    /// pure, for undo capture.
    pub fn snapshot_graph_output(
        &self,
        graph_id: GraphId,
        idx: usize,
    ) -> Option<DetachedGraphOutput> {
        let child = self.graphs.get(&graph_id)?;
        let spec = child.outputs.get(idx)?.clone();
        let interior = match child.body.boundary_node(NodeKind::GraphOutput) {
            Some(boundary) => child
                .body
                .input_slot_binding(boundary, idx)
                .into_iter()
                .collect(),
            None => Vec::new(),
        };
        let mut parent = Vec::new();
        for instance in self.local_instances(graph_id) {
            parent.extend(self.output_slot_consumers(instance, idx));
        }
        Some(DetachedGraphOutput {
            idx,
            spec,
            interior,
            parent,
        })
    }

    /// Ids of this graph's `Graph(Local(graph_id))` instance nodes.
    fn local_instances(&self, graph_id: GraphId) -> Vec<NodeId> {
        self.iter()
            .filter_map(|node| match node.kind {
                NodeKind::Graph(GraphLink::Local(id)) if id == graph_id => Some(node.id),
                _ => None,
            })
            .collect()
    }

    /// This graph's single boundary node of `kind`, if present.
    pub(crate) fn boundary_node(&self, kind: NodeKind) -> Option<NodeId> {
        self.iter()
            .find(|node| node.kind == kind)
            .map(|node| node.id)
    }

    /// What [`Self::remove_output_slot`] would sever.
    fn output_slot_consumers(&self, node: NodeId, idx: usize) -> Vec<BindingEntry> {
        let slot = OutputPort::new(node, idx);
        self.bindings
            .iter()
            .filter(|(_, binding)| matches!(binding, Binding::Bind(src) if *src == slot))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }

    /// What [`Self::remove_input_slot`] would drop.
    fn input_slot_binding(&self, node: NodeId, idx: usize) -> Option<BindingEntry> {
        let port = InputPort::new(node, idx);
        self.bindings.get(&port).map(|binding| BindingEntry {
            port,
            binding: binding.clone(),
        })
    }

    /// Assert `entries` can all be restored, writing nothing: no port
    /// already holds wiring, and no port is named twice.
    ///
    /// The pre-flight half of [`Self::restore_bindings`], hoisted by both
    /// `attach_*` operations into their validation phase so a refusal
    /// happens before the first mutation. `entries` comes from a
    /// caller-held `Detached*` record, so a duplicate port in it is
    /// caller-supplied too and is refused in release like the rest;
    /// checking it here rather than on the way in means it also refuses
    /// before anything moves. Quadratic over a port list of a few entries,
    /// on the editor's undo path.
    fn assert_restorable(&self, entries: &[BindingEntry]) {
        for (i, entry) in entries.iter().enumerate() {
            assert!(
                !self.bindings.contains_key(&entry.port),
                "cannot attach over the binding on {:?}, created after detachment",
                entry.port,
            );
            assert!(
                !entries[..i]
                    .iter()
                    .any(|earlier| earlier.port == entry.port),
                "detached record restores {:?} twice",
                entry.port,
            );
        }
    }
}

impl Binding {
    /// A data binding wired to producer `node_id`'s output port `port_idx`.
    pub fn bind(node_id: NodeId, port_idx: usize) -> Self {
        Binding::Bind(OutputPort::new(node_id, port_idx))
    }

    /// Whether this edge — the binding on `port` — touches `node_id` from
    /// either end: it lands on that node, or it reads from it. How a detach
    /// finds the wiring it has to take with it, and how the record it
    /// produced proves it took only that.
    pub(crate) fn touches(&self, port: InputPort, node_id: NodeId) -> bool {
        port.node_id == node_id || matches!(self, Binding::Bind(src) if src.node_id == node_id)
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

#[cfg(test)]
mod tests;
