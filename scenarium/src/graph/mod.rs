use std::collections::{BTreeMap, BTreeSet, HashMap};

use common::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::graph::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use crate::library::Library;
use crate::node::function::{Func, FuncId};
use crate::node::special::SpecialNode;
use anyhow::ensure;
use common::id_type;
use common::{Result, SerdeFormat, deserialize, serialize};
use hashbrown::HashSet;

id_type!(NodeId);

mod query;
pub mod subgraph;
#[cfg(test)]
mod tests;
mod validate;

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

/// True when `binding` (at `port`) references `node_id` on either end — i.e.
/// `port` is the node's own input, or it's a `Bind` edge feeding into it.
/// Single source of truth for both `remove_by_id`'s drop and the undo capture.
fn binding_touches(port: InputPort, binding: &Binding, node_id: NodeId) -> bool {
    port.node_id == node_id || matches!(binding, Binding::Bind(src) if src.node_id == node_id)
}

/// True when `s` references `node_id` as emitter or subscriber.
fn subscription_touches(s: &Subscription, node_id: NodeId) -> bool {
    s.emitter == node_id || s.subscriber == node_id
}

/// Whether adding a data edge `producer → consumer` (producer's output feeding
/// consumer's input) would close a directed cycle: `producer`'s node is already
/// reachable from `consumer`'s node along the existing edges. `edges` yields
/// every data edge as `(producer_node, consumer_node)`.
///
/// The free function so the editor's snap pre-filter can reuse it over its own
/// per-frame edge mirror (a render projection, not a `Graph`);
/// [`Graph::would_create_cycle`] is the wrapper for callers holding a `Graph`.
/// Either way the execution planner is the authoritative backstop
/// (`Error::CycleDetected`).
pub fn closes_data_cycle(
    edges: impl Iterator<Item = (NodeId, NodeId)>,
    producer: NodeId,
    consumer: NodeId,
) -> bool {
    if producer == consumer {
        return true;
    }
    // One pass builds the producer→consumers adjacency; then walk it downstream
    // from `consumer`. Reaching `producer` means the new edge closes the loop.
    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (src, dst) in edges {
        adjacency.entry(src).or_default().push(dst);
    }
    let mut stack = vec![consumer];
    let mut seen: HashSet<NodeId> = [consumer].into_iter().collect();
    while let Some(node) = stack.pop() {
        for &next in adjacency.get(&node).into_iter().flatten() {
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
/// - `Disk` — persisted to the content-addressed store (survives reload); its RAM copy
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

    /// Whether the node's value is persisted to the content-addressed disk store
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
    /// copies its func's `default_cache_mode`; the func-less constructors seed
    /// `None`. `#[serde(default)]` → `None` (the [`CacheMode`] default) for a
    /// pre-field (or pre-rename) document, so a legacy node caches nowhere until
    /// re-toggled.
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
    nodes: KeyIndexVec<NodeId, Node>,

    /// Data wiring, keyed by consumer input port. Sparse: only `Const`/`Bind`
    /// ports appear; absent = `Binding::None`. A `BTreeMap` keeps
    /// serialization deterministic and lets a node's ports range contiguously.
    /// Serialized as a sequence of `(port, binding)` pairs — struct keys aren't
    /// valid map keys in string-keyed formats (JSON/TOML/Rhai).
    #[serde(default, with = "binding_map_serde")]
    bindings: BTreeMap<InputPort, Binding>,

    /// Event wiring: every (emitter event → subscriber) edge, flat. A
    /// `BTreeSet` dedups, keeps serialization deterministic, and ranges over
    /// one emitter-event's subscribers contiguously.
    #[serde(default)]
    subscriptions: BTreeSet<Subscription>,

    /// Local (per-instance) subgraph definitions referenced by this graph's
    /// `NodeKind::Subgraph(SubgraphRef::Local(_))` nodes. Editing one of
    /// these affects only this graph. Shared definitions live in
    /// `Library.subgraphs` instead. See `execution/README.md` Part A §4.4.
    #[serde(default)]
    pub subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,
}

/// A graph cloned with fresh node ids, plus the old→new id map (so
/// callers can remap ids the graph doesn't own, e.g. a subgraph def's
/// exposed-event emitters). Result of [`Graph::with_fresh_node_ids`].
pub(crate) struct FreshGraph {
    pub(crate) graph: Graph,
    pub(crate) id_map: HashMap<NodeId, NodeId>,
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
    pub fn remove_by_id(&mut self, id: NodeId) {
        assert!(!id.is_nil());

        self.nodes.remove_by_key(&id);

        // Drop the node's own input bindings, and any edge feeding *into* it.
        self.bindings
            .retain(|port, binding| !binding_touches(*port, binding, id));

        // Drop subscriptions where it is either the emitter or a subscriber.
        self.subscriptions.retain(|s| !subscription_touches(s, id));
    }

    // === Data bindings ===

    /// What input `port` is wired to (`None` when unbound).
    pub fn input_binding(&self, port: InputPort) -> Binding {
        self.bindings.get(&port).cloned().unwrap_or(Binding::None)
    }

    /// Set (or, for `None`, clear) the binding of input `port`.
    pub fn set_input_binding(&mut self, port: InputPort, binding: Binding) {
        if matches!(binding, Binding::None) {
            self.bindings.remove(&port);
        } else {
            self.bindings.insert(port, binding);
        }
    }

    /// A node's input bindings in port order `0..arity`, yielding `None` for
    /// unbound ports. `arity` comes from the node's func/def.
    pub fn node_bindings(
        &self,
        node_id: NodeId,
        arity: usize,
    ) -> impl Iterator<Item = (usize, Binding)> + '_ {
        (0..arity).map(move |port_idx| {
            (
                port_idx,
                self.input_binding(InputPort::new(node_id, port_idx)),
            )
        })
    }

    /// Deep-clone with a freshly generated id for every node, remapping
    /// all bindings + subscriptions onto the new ids. Nested per-graph
    /// subgraph defs are copied verbatim — their ids are private to this
    /// graph's table. Returns the clone plus the old→new id map (callers
    /// like subgraph localization need it to remap exposed-event
    /// emitters). Used to make an independent copy of a subgraph interior.
    pub(crate) fn with_fresh_node_ids(&self) -> FreshGraph {
        let mut id_map = HashMap::with_capacity(self.nodes.len());
        let mut nodes = KeyIndexVec::with_capacity(self.nodes.len());
        for node in self.nodes.iter() {
            let new_id = NodeId::unique();
            id_map.insert(node.id, new_id);
            let mut clone = node.clone();
            clone.id = new_id;
            nodes.add(clone);
        }
        let remap = |id: NodeId| id_map.get(&id).copied().unwrap_or(id);
        let bindings = self
            .bindings
            .iter()
            .map(|(port, binding)| {
                let port = InputPort::new(remap(port.node_id), port.port_idx);
                let binding = match binding {
                    Binding::Bind(op) => Binding::bind(remap(op.node_id), op.port_idx),
                    other => other.clone(),
                };
                (port, binding)
            })
            .collect();
        let subscriptions = self
            .subscriptions
            .iter()
            .map(|s| Subscription {
                emitter: remap(s.emitter),
                event_idx: s.event_idx,
                subscriber: remap(s.subscriber),
            })
            .collect();
        let graph = Graph {
            nodes,
            bindings,
            subscriptions,
            subgraphs: self.subgraphs.clone(),
        };
        FreshGraph { graph, id_map }
    }

    // === Event subscriptions ===

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

    pub fn is_subscribed(&self, emitter: NodeId, event_idx: usize, subscriber: NodeId) -> bool {
        self.subscriptions.contains(&Subscription {
            emitter,
            event_idx,
            subscriber,
        })
    }

    /// Every binding that references `node_id` as consumer (its own inputs)
    /// or producer (edges feeding into it) — i.e. exactly what `remove_by_id`
    /// would drop. For snapshot/restore (editor undo).
    pub fn bindings_touching(&self, node_id: NodeId) -> Vec<(InputPort, Binding)> {
        self.bindings
            .iter()
            .filter(|(port, binding)| binding_touches(**port, binding, node_id))
            .map(|(port, binding)| (*port, binding.clone()))
            .collect()
    }

    /// Every subscription with `node_id` as emitter or subscriber — what
    /// `remove_by_id` would drop. For snapshot/restore (editor undo).
    pub fn subscriptions_touching(&self, node_id: NodeId) -> Vec<Subscription> {
        self.subscriptions
            .iter()
            .copied()
            .filter(|s| subscription_touches(s, node_id))
            .collect()
    }

    /// Re-insert wiring captured by `bindings_touching` / `subscriptions_touching`.
    pub fn restore_wiring(
        &mut self,
        bindings: &[(InputPort, Binding)],
        subscriptions: &[Subscription],
    ) {
        for (port, binding) in bindings {
            self.set_input_binding(*port, binding.clone());
        }
        self.subscriptions.extend(subscriptions.iter().copied());
    }

    /// Every subscription edge in this graph (deterministic order).
    pub fn subscriptions(&self) -> impl Iterator<Item = Subscription> + '_ {
        self.subscriptions.iter().copied()
    }

    /// Drop data bindings left dangling when a node's func/def shrank its
    /// interface (e.g. a document loaded against a newer library): the
    /// consumer input or the producer output is now out of range, or the
    /// referenced node is gone. Boundary-node arity follows the enclosing def
    /// (not the library), so bindings touching one are left to
    /// [`crate::Graph`]'s reconcile path. Structural only — an in-range but
    /// type-incompatible binding is left for the run / [`Self::check_with`] to
    /// surface, since auto-dropping a retyped wire is a stronger call.
    /// Recurses into local subgraph defs. Returns the number removed;
    /// idempotent.
    pub fn prune_dangling_wiring(&mut self, library: &Library) -> usize {
        // `retain` would need `&self` for the port-count lookups while holding
        // `&mut self.bindings` / `&mut self.subscriptions`; swap each out so the
        // filter can read the rest of `self`, then swap it back.
        let mut bindings = std::mem::take(&mut self.bindings);
        let before = bindings.len();
        bindings.retain(|dst, binding| self.binding_live(*dst, binding, library));
        self.bindings = bindings;
        let mut removed = before - self.bindings.len();

        let mut subs = std::mem::take(&mut self.subscriptions);
        let before = subs.len();
        subs.retain(|s| self.subscription_live(s, library));
        self.subscriptions = subs;
        removed += before - self.subscriptions.len();

        for def in self.subgraphs.iter_mut() {
            removed += def.graph.prune_dangling_wiring(library);
        }
        removed
    }

    /// Whether a binding still addresses ports that exist on both ends — the
    /// structural validity [`Self::prune_dangling_wiring`] keeps. An in-range
    /// but type-incompatible binding stays (auto-dropping a retyped wire is a
    /// stronger call left to the run / [`Self::check_with`]).
    fn binding_live(&self, dst: InputPort, binding: &Binding, library: &Library) -> bool {
        self.find_node(&dst.node_id, NodeSearch::TopLevel)
            .is_some_and(|c| self.port_in_range(c, dst.port_idx, true, library))
            && match binding {
                Binding::Bind(src) => self
                    .find_node(&src.node_id, NodeSearch::TopLevel)
                    .is_some_and(|p| self.port_in_range(p, src.port_idx, false, library)),
                Binding::None | Binding::Const(_) => true,
            }
    }

    /// Whether a subscription still references a live event: its emitter is
    /// present and `event_idx` is in range. A present emitter whose func/def is
    /// *missing* from the library has unknowable arity, so it's kept (valid
    /// again once the library is restored); a removed emitter drops the edge.
    fn subscription_live(&self, s: &Subscription, library: &Library) -> bool {
        match self.find_node(&s.emitter, NodeSearch::TopLevel) {
            None => false,
            Some(emitter) => self
                .event_count_opt(emitter, library)
                .is_none_or(|count| s.event_idx < count),
        }
    }

    /// Whether `node`'s `idx`-th input (`is_input`) or output port exists.
    /// Resolved without the infallible `*_count` helpers (which `unwrap` the
    /// library): a node whose func/def is *missing* from the library — a stub
    /// for a doc saved against a richer library — has unknowable arity, so its
    /// wiring is kept (`true`) rather than dropped or panicked on; it becomes
    /// valid again if the library is restored. Boundary nodes (arity follows
    /// the enclosing def, not the library) are likewise kept — reconcile owns
    /// them, and their counts would need a `ctx_def`.
    fn port_in_range(&self, node: &Node, idx: usize, is_input: bool, library: &Library) -> bool {
        let side = |inputs: usize, outputs: usize| idx < if is_input { inputs } else { outputs };
        match &node.kind {
            NodeKind::Func(id) => library
                .by_id(id)
                .is_none_or(|f| side(f.inputs.len(), f.outputs.len())),
            NodeKind::Subgraph(r) => self
                .resolve_def(*r, library)
                .is_none_or(|d| side(d.inputs.len(), d.outputs.len())),
            NodeKind::Special(s) => {
                let f = s.func();
                side(f.inputs.len(), f.outputs.len())
            }
            NodeKind::SubgraphInput | NodeKind::SubgraphOutput => true,
        }
    }

    /// The emitter's event count, or `None` when its func/def is missing from
    /// the library (unknowable). The fallible peer of [`Self::event_count`]
    /// (which delegates here) — used by the subscription prune so a stub
    /// node's wiring survives.
    fn event_count_opt(&self, node: &Node, library: &Library) -> Option<usize> {
        match &node.kind {
            NodeKind::Func(id) => library.by_id(id).map(|f| f.events.len()),
            NodeKind::Subgraph(r) => self.resolve_def(*r, library).map(|d| d.events.len()),
            NodeKind::Special(s) => Some(s.func().events.len()),
            NodeKind::SubgraphInput => Some(1),
            NodeKind::SubgraphOutput => Some(0),
        }
    }

    /// Subscribers of one emitter event, in `NodeId` order.
    pub fn subscribers(
        &self,
        emitter: NodeId,
        event_idx: usize,
    ) -> impl Iterator<Item = NodeId> + '_ {
        let lo = Subscription {
            emitter,
            event_idx,
            subscriber: NodeId::nil(),
        };
        let hi = Subscription {
            emitter,
            event_idx: event_idx + 1,
            subscriber: NodeId::nil(),
        };
        // The bounded range yields exactly this (emitter, event_idx)'s
        // subscribers — `Subscription`'s ordering puts event_idx ahead of
        // subscriber, so no post-filter is needed.
        self.subscriptions.range(lo..hi).map(|s| s.subscriber)
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
    /// Port-index ranges need a `Library`, so they're checked by
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
                // Special nodes carry no id to validate; their declaration is
                // hardcoded and always resolves.
                NodeKind::Special(_) | NodeKind::SubgraphInput | NodeKind::SubgraphOutput => {}
            }
        }

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

        for def in self.subgraphs.iter() {
            ensure!(!def.id.is_nil(), "local subgraph has a nil id");
            def.graph.check()?;
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

    // === Construction helpers (seed default bindings) ===

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
    /// ports are pre-shaped from its definition.
    pub fn new(kind: NodeKind) -> Self {
        Node {
            id: NodeId::unique(),
            kind,
            name: String::new(),
            cache: CacheMode::None,
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

impl From<(NodeId, usize)> for Binding {
    fn from((output_node_id, output_idx): (NodeId, usize)) -> Self {
        Binding::bind(output_node_id, output_idx)
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
