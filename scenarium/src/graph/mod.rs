use std::collections::{BTreeMap, BTreeSet};

use common::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};

use crate::StaticValue;
use crate::graph::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use crate::library::Library;
use crate::node::definition::{Func, FuncId};
use crate::node::special::SpecialNode;
use anyhow::{Context, ensure};
use common::id_type;
use common::{Result, SerdeFormat, deserialize, serialize};
use hashbrown::HashSet;

id_type!(NodeId);

pub(crate) mod clone;
mod query;
pub(crate) mod reconcile;
pub(crate) mod subgraph;
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
/// only holds `Const`/`Bind` entries, and an absent port reads back as `None`
/// — so unbound inputs cost nothing.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    #[default]
    None,
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

/// Serialize `bindings` as a `(port, binding)` sequence: struct keys can't be
/// map keys in string-keyed formats (JSON/TOML/Rhai). Order is deterministic
/// because the source is a `BTreeMap`.
mod binding_map_serde {
    use crate::graph::{Binding, InputPort};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub(crate) fn serialize<S: Serializer>(
        map: &BTreeMap<InputPort, Binding>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<InputPort, Binding>, D::Error> {
        Ok(Vec::<(InputPort, Binding)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
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

/// What a node *is*. A plain `Func` instance, a composite `Subgraph`
/// instance, a built-in [`SpecialNode`] (hardcoded declaration, recognized by
/// the engine), or one of the two interface boundary nodes that may appear only
/// inside a `SubgraphDef.graph` (their port arity comes from the enclosing def's
/// interface, not from a func).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Func(FuncId),
    Subgraph(SubgraphRef),
    /// A built-in special node; its interface comes from
    /// [`SpecialNode::func`].
    Special(SpecialNode),
    /// Inbound boundary: outputs = enclosing def's exposed inputs.
    SubgraphInput,
    /// Outbound boundary: inputs = enclosing def's exposed outputs.
    SubgraphOutput,
}

// A node is pure identity: its port/event *arity* comes from the func/def
// (`NodeKind`), and its mutable wiring (input bindings, event subscriptions)
// lives in `Graph`'s flat side-tables — never in per-node Vecs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,

    /// Where this node's output is cached. See [`CacheMode`]. A fresh func node
    /// (`From<&Func>`) or special node (`Node::new`) copies its func's
    /// `default_cache_mode`; the remaining func-less constructors seed `None`.
    /// `#[serde(default)]` → `None` (the [`CacheMode`] default) for a pre-field
    /// (or pre-rename) document, so a legacy node caches nowhere until re-toggled.
    #[serde(default)]
    pub cache: CacheMode,

    /// Disabled nodes are skipped at flatten time: they emit no execution
    /// node, and any binding resolving to one yields no producer, so a
    /// downstream consumer sees the wire as unbound (→ `MissingInputs` if
    /// the input is required). `#[serde(default)]` → `false` keeps
    /// pre-field documents enabled.
    #[serde(default)]
    pub disabled: bool,
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

/// How deep a node lookup reaches: this graph's own nodes only, or also
/// every local subgraph interior, recursively. The argument to
/// [`Graph::find_node`] / [`Graph::find_node_mut`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeSearch {
    TopLevel,
    Recursive,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    pub(crate) nodes: KeyIndexVec<NodeId, Node>,

    /// Data wiring, keyed by consumer input port. Sparse: only `Const`/`Bind`
    /// ports appear; absent = `Binding::None`. A `BTreeMap` keeps
    /// serialization deterministic and lets a node's ports range contiguously.
    /// Serialized as a sequence of `(port, binding)` pairs — struct keys aren't
    /// valid map keys in string-keyed formats (JSON/TOML/Rhai).
    #[serde(default, with = "binding_map_serde")]
    pub(crate) bindings: BTreeMap<InputPort, Binding>,

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

    /// Local (per-instance) subgraph definitions referenced by this graph's
    /// `NodeKind::Subgraph(SubgraphRef::Local(_))` nodes. Editing one of
    /// these affects only this graph. Shared definitions live in
    /// `Library.subgraphs` instead. See `execution/README.md` Part A §4.4.
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
    /// callers (editor rendering, action-stack replay) rely on it.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.nodes.iter_mut()
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        assert!(!name.is_empty());
        self.nodes.iter().find(|node| node.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Node> {
        assert!(!name.is_empty());
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    /// The node with `id`, at the depth `search` selects. Node ids are
    /// unique across a whole document (subgraph interiors included), so a
    /// [`Recursive`](NodeSearch::Recursive) hit is unambiguous.
    pub fn find_node(&self, id: &NodeId, search: NodeSearch) -> Option<&Node> {
        assert!(!id.is_nil());
        match self.nodes.by_key(id) {
            Some(node) => Some(node),
            None => match search {
                NodeSearch::TopLevel => None,
                NodeSearch::Recursive => self
                    .subgraphs
                    .iter()
                    .find_map(|d| d.graph.find_node(id, NodeSearch::Recursive)),
            },
        }
    }

    /// Mutable counterpart of [`Self::find_node`].
    pub fn find_node_mut(&mut self, id: &NodeId, search: NodeSearch) -> Option<&mut Node> {
        assert!(!id.is_nil());
        // Probe immutably first: returning the mutable borrow straight out
        // of an `if let` would hold it for the whole function under NLL.
        if self.nodes.by_key(id).is_some() {
            return self.nodes.by_key_mut(id);
        }
        match search {
            NodeSearch::TopLevel => None,
            NodeSearch::Recursive => self
                .subgraphs
                .iter_mut()
                .find_map(|d| d.graph.find_node_mut(id, NodeSearch::Recursive)),
        }
    }

    pub fn serialize(&self, format: SerdeFormat) -> Result<Vec<u8>> {
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
    /// Treats `self` as a root graph (boundary nodes are invalid at this
    /// level); nested defs are checked via [`SubgraphDef::check`], which
    /// re-enters in def-interior mode. Port-index ranges (and anything else
    /// needing a `Library`) are checked by `check_with`, not here.
    pub fn check(&self) -> Result<()> {
        self.check_impl(false)?;
        self.check_unique_node_ids(None)
    }

    fn check_unique_node_ids(&self, library: Option<&Library>) -> Result<()> {
        fn collect(
            graph: &Graph,
            library: Option<&Library>,
            visited: &mut HashSet<*const Graph>,
            node_ids: &mut HashSet<NodeId>,
        ) -> Result<()> {
            if !visited.insert(graph) {
                return Ok(());
            }
            for node in graph.nodes.iter() {
                ensure!(
                    node_ids.insert(node.id),
                    "node id {:?} occurs in more than one authoring graph",
                    node.id
                );
            }
            for def in graph.subgraphs.iter() {
                collect(&def.graph, library, visited, node_ids)?;
            }
            if let Some(library) = library {
                for node in graph.nodes.iter() {
                    if let NodeKind::Subgraph(SubgraphRef::Linked(id)) = node.kind
                        && let Some(def) = library.subgraphs.by_key(&id)
                    {
                        collect(&def.graph, Some(library), visited, node_ids)?;
                    }
                }
            }
            Ok(())
        }

        collect(self, library, &mut HashSet::new(), &mut HashSet::new())
    }

    /// The recursive body of [`Self::check`]. `is_def_interior` toggles the
    /// one context-dependent rule: `SubgraphInput`/`SubgraphOutput` are valid
    /// only inside a subgraph def, at most one of each — flattening routes
    /// through the *first* boundary node of a kind, so a duplicate would
    /// silently misroute rather than fail.
    pub(crate) fn check_impl(&self, is_def_interior: bool) -> Result<()> {
        let mut boundary_inputs = 0usize;
        let mut boundary_outputs = 0usize;
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
                // Special nodes carry no id to validate; their declaration is
                // hardcoded and always resolves.
                NodeKind::Special(_) => {}
                NodeKind::SubgraphInput => {
                    ensure!(
                        is_def_interior,
                        "node {:?}: SubgraphInput is only valid inside a subgraph def",
                        node.id
                    );
                    boundary_inputs += 1;
                }
                NodeKind::SubgraphOutput => {
                    ensure!(
                        is_def_interior,
                        "node {:?}: SubgraphOutput is only valid inside a subgraph def",
                        node.id
                    );
                    boundary_outputs += 1;
                }
            }
        }
        ensure!(
            boundary_inputs <= 1,
            "a def interior holds at most one SubgraphInput, found {boundary_inputs}"
        );
        ensure!(
            boundary_outputs <= 1,
            "a def interior holds at most one SubgraphOutput, found {boundary_outputs}"
        );

        for (dst, binding) in &self.bindings {
            ensure!(
                self.nodes.by_key(&dst.node_id).is_some(),
                "binding on missing node {:?}",
                dst.node_id
            );
            if let Binding::Bind(src) = binding {
                ensure!(
                    self.nodes.by_key(&src.node_id).is_some(),
                    "node {:?} input {} binds to missing node {:?}",
                    dst.node_id,
                    dst.port_idx,
                    src.node_id
                );
            }
        }

        for s in &self.subscriptions {
            ensure!(
                self.nodes.by_key(&s.emitter).is_some(),
                "subscription from missing emitter {:?}",
                s.emitter
            );
            ensure!(
                self.nodes.by_key(&s.subscriber).is_some(),
                "node {:?} event {} has missing subscriber {:?}",
                s.emitter,
                s.event_idx,
                s.subscriber
            );
        }

        for port in &self.pinned_outputs {
            ensure!(
                self.nodes.by_key(&port.node_id).is_some(),
                "pinned output on missing node {:?}",
                port.node_id
            );
        }

        for def in self.subgraphs.iter() {
            def.check()
                .with_context(|| format!("in local subgraph {:?}", def.name))?;
        }

        Ok(())
    }

    /// Resolve a `SubgraphRef` to its definition: `Local` from this graph's
    /// own table, `Linked` from the shared `Library`.
    pub fn resolve_def<'a>(
        &'a self,
        r: SubgraphRef,
        library: &'a Library,
    ) -> Option<&'a SubgraphDef> {
        match r {
            SubgraphRef::Local(id) => self.subgraphs.by_key(&id),
            SubgraphRef::Linked(id) => library.subgraphs.by_key(&id),
        }
    }

    /// Add a func instance and seed its inputs' default const bindings.
    /// Returns the new node id.
    pub fn add_func_node(&mut self, func: &Func) -> NodeId {
        let node = Node::from(func);
        let node_id = node.id;
        self.add(node);
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

    /// Add a composite instance and seed its inputs' default const bindings
    /// from the def interface. `r` must reference `def`.
    pub fn add_subgraph_node(&mut self, def: &SubgraphDef, r: SubgraphRef) -> NodeId {
        let node = Node::subgraph_instance(def, r);
        let node_id = node.id;
        self.add(node);
        for (port_idx, io) in def.inputs.iter().enumerate() {
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
    /// A fresh node of the given kind with a unique id and no inputs/events.
    /// The id is minted here (the one RNG-touching constructor); callers fill
    /// in wiring, or use `From<&Func>` / `subgraph_instance` for a node whose
    /// ports are pre-shaped from its definition. A `Special` node copies its
    /// hardcoded func's `default_cache_mode`; every other kind seeds `None`.
    pub fn new(kind: NodeKind) -> Self {
        let cache = match &kind {
            NodeKind::Special(s) => s.func().default_cache_mode,
            _ => CacheMode::None,
        };
        Node {
            id: NodeId::unique(),
            kind,
            name: String::new(),
            cache,
            disabled: false,
        }
    }

    /// A composite instance node shaped from a definition (just identity +
    /// name; default input bindings are seeded by `Graph::add_subgraph_node`).
    /// `r` must reference `def`.
    pub fn subgraph_instance(def: &SubgraphDef, r: SubgraphRef) -> Self {
        assert_eq!(r.id(), def.id, "SubgraphRef must reference the given def");

        Node {
            id: NodeId::unique(),
            kind: NodeKind::Subgraph(r),
            name: def.name.clone(),
            cache: CacheMode::None,
            disabled: false,
        }
    }
}

impl From<&Func> for Node {
    /// A bare func instance (identity + name), copying the func's
    /// `default_cache_mode` into its `cache`. Default input bindings are seeded
    /// by `Graph::add_func_node`.
    fn from(func: &Func) -> Self {
        Node {
            id: NodeId::unique(),
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
