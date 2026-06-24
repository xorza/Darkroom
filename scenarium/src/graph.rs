use std::collections::{BTreeMap, BTreeSet, HashMap};

use common::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncId, FuncLib};
use crate::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use anyhow::{Context, ensure};
use common::{Result, SerdeFormat, deserialize, serialize};
use common::{id_type, is_debug};
use hashbrown::HashSet;

id_type!(NodeId);

/// Address of a producer node's output port — the source side of a data
/// binding (`Binding::Bind`).
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
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

/// Serialize `bindings` as a `(port, binding)` sequence: struct keys can't be
/// map keys in string-keyed formats (JSON/TOML/Rhai). Order is deterministic
/// because the source is a `BTreeMap`.
mod binding_map_serde {
    use crate::graph::{Binding, InputPort};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<InputPort, Binding>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<InputPort, Binding>, D::Error> {
        Ok(Vec::<(InputPort, Binding)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

/// Where a node's computed output is cached. `Memory` keeps it only in the live
/// engine (dropped on reload); `Disk` also persists it to the content-addressed
/// store, so an unchanged graph reloads the result instead of recomputing.
/// `Disk` is a *request* honored only for reproducible nodes — a node with an
/// impure node anywhere in its upstream cone has no content digest, so it's
/// silently kept memory-only and never risks serving a stale value. See
/// `docs/disk-cache-design.md`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum CachePersistence {
    #[default]
    Memory,
    Disk,
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

// A node is pure identity: its port/event *arity* comes from the func/def
// (`NodeKind`), and its mutable wiring (input bindings, event subscriptions)
// lives in `Graph`'s flat side-tables — never in per-node Vecs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,

    /// Where this node's output is cached. See [`CachePersistence`].
    /// `#[serde(default)]` → `Memory` keeps pre-field documents memory-only.
    #[serde(default)]
    pub persist: CachePersistence,

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
    /// `FuncLib.subgraphs` instead. See `docs/subgraph-design.md` §4.4.
    #[serde(default)]
    pub subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,
}

/// A graph cloned with fresh node ids, plus the old→new id map (so
/// callers can remap ids the graph doesn't own, e.g. a subgraph def's
/// exposed-event emitters). Result of [`Graph::with_fresh_node_ids`].
pub struct FreshGraph {
    pub graph: Graph,
    pub id_map: HashMap<NodeId, NodeId>,
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
                self.input_binding(InputPort { node_id, port_idx }),
            )
        })
    }

    /// Deep-clone with a freshly generated id for every node, remapping
    /// all bindings + subscriptions onto the new ids. Nested per-graph
    /// subgraph defs are copied verbatim — their ids are private to this
    /// graph's table. Returns the clone plus the old→new id map (callers
    /// like subgraph localization need it to remap exposed-event
    /// emitters). Used to make an independent copy of a subgraph interior.
    pub fn with_fresh_node_ids(&self) -> FreshGraph {
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
                let port = InputPort {
                    node_id: remap(port.node_id),
                    port_idx: port.port_idx,
                };
                let binding = match binding {
                    Binding::Bind(op) => Binding::Bind(OutputPort {
                        node_id: remap(op.node_id),
                        port_idx: op.port_idx,
                    }),
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

    pub fn by_id(&self, id: &NodeId) -> Option<&Node> {
        assert!(!id.is_nil());
        self.nodes.by_key(id)
    }
    pub fn by_id_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());
        self.nodes.by_key_mut(id)
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

    /// Debug-only assert form of [`check_with`]: a violation surfaces as a
    /// panic so a graph the editor itself built wrong is caught loudly in
    /// development. The release-safe, error-returning gate is `check_with`,
    /// which `ExecutionEngine::update` runs in every build.
    pub fn validate_with(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }
        self.check_with(func_lib)
            .expect("graph structural invariant violated");
    }

    /// Full structural validation against `func_lib`, in all builds. Extends
    /// [`check`] (which can't see the library) with every func_lib-dependent
    /// check: each func/subgraph reference resolves, no subgraph contains
    /// itself, boundary nodes sit inside a def, and every binding/subscription
    /// port index is in range. A graph+library is untrusted input at the
    /// compile boundary (a document can be stale against an evolved library),
    /// so an invalid one is a recoverable error the caller surfaces — not a
    /// panic. With this passing, flattening resolves every reference infallibly.
    pub fn check_with(&self, func_lib: &FuncLib) -> Result<()> {
        self.check()?;
        let mut visited: HashSet<SubgraphId> = HashSet::new();
        self.check_level(func_lib, None, &mut visited)
    }

    /// Recursive per-level half of [`check_with`]. `ctx_def` is the enclosing
    /// subgraph definition when checking a def's interior (so boundary nodes
    /// can be checked against the interface), `None` at the top level.
    /// `visited` is the descent path of `SubgraphId`s — re-entering one is the
    /// recursion error.
    fn check_level(
        &self,
        func_lib: &FuncLib,
        ctx_def: Option<&SubgraphDef>,
        visited: &mut HashSet<SubgraphId>,
    ) -> Result<()> {
        // Resolve every node's func/def first (and recurse into composites):
        // the port-count helpers below look funcs/defs up infallibly, so this
        // pass must establish they all resolve before any count is taken.
        for node in self.nodes.iter() {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    ensure!(
                        func_lib.by_id(func_id).is_some(),
                        "node {:?} references func {:?}, absent from the library",
                        node.id,
                        func_id
                    );
                }
                NodeKind::Subgraph(r) => {
                    let def = self.resolve_def(*r, func_lib).with_context(|| {
                        format!(
                            "node {:?} references a missing subgraph definition",
                            node.id
                        )
                    })?;
                    ensure!(
                        visited.insert(def.id),
                        "subgraph {:?} is recursive (contains itself)",
                        def.id
                    );
                    def.graph.check_level(func_lib, Some(def), visited)?;
                    visited.remove(&def.id);
                }
                NodeKind::SubgraphInput => {
                    ensure!(
                        ctx_def.is_some(),
                        "SubgraphInput node is only valid inside a subgraph"
                    );
                }
                NodeKind::SubgraphOutput => {
                    ensure!(
                        ctx_def.is_some(),
                        "SubgraphOutput is only valid inside a subgraph"
                    );
                }
            }
        }

        // When checking a def's interior, each exposed event must name an
        // interior emitter that actually exposes that event.
        if let Some(def) = ctx_def {
            for event in &def.events {
                let emitter = self.by_id(&event.emitter).with_context(|| {
                    format!("exposed event names missing emitter {:?}", event.emitter)
                })?;
                ensure!(
                    event.emitter_event_idx < self.event_count(emitter, func_lib, ctx_def),
                    "exposed event index {} out of range on {:?}",
                    event.emitter_event_idx,
                    event.emitter
                );
            }
        }

        // Every binding addresses ports that exist on both ends.
        for (dst, binding) in self.bindings.iter() {
            let consumer = self
                .by_id(&dst.node_id)
                .with_context(|| format!("binding on missing node {:?}", dst.node_id))?;
            ensure!(
                dst.port_idx < self.input_count(consumer, func_lib, ctx_def),
                "binding on node {:?} input {} is out of range",
                dst.node_id,
                dst.port_idx
            );
            if let Binding::Bind(src) = binding {
                let producer = self
                    .by_id(&src.node_id)
                    .with_context(|| format!("binding from missing node {:?}", src.node_id))?;
                ensure!(
                    src.port_idx < self.output_count(producer, func_lib, ctx_def),
                    "binding from node {:?} output {} is out of range",
                    src.node_id,
                    src.port_idx
                );
            }
        }

        // Every subscription targets an event the emitter actually exposes.
        for s in self.subscriptions.iter() {
            let emitter = self
                .by_id(&s.emitter)
                .with_context(|| format!("subscription from missing emitter {:?}", s.emitter))?;
            ensure!(
                s.event_idx < self.event_count(emitter, func_lib, ctx_def),
                "subscription event index {} out of range on {:?}",
                s.event_idx,
                s.emitter
            );
        }
        Ok(())
    }

    /// Number of input ports a node exposes — by kind. `ctx_def` is the
    /// enclosing def, needed only for `SubgraphOutput` (whose inputs are the
    /// def's exposed outputs).
    fn input_count(&self, node: &Node, func_lib: &FuncLib, ctx_def: Option<&SubgraphDef>) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => func_lib.by_id(func_id).unwrap().inputs.len(),
            NodeKind::Subgraph(r) => self.resolve_def(*r, func_lib).unwrap().inputs.len(),
            NodeKind::SubgraphInput => 0,
            NodeKind::SubgraphOutput => ctx_def.unwrap().outputs.len(),
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

    /// Number of events a node exposes. `SubgraphInput` exposes exactly one —
    /// the trigger that interior nodes subscribe to so they fire when the
    /// enclosing composite is triggered.
    fn event_count(
        &self,
        node: &Node,
        func_lib: &FuncLib,
        _ctx_def: Option<&SubgraphDef>,
    ) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => func_lib.by_id(func_id).unwrap().events.len(),
            NodeKind::Subgraph(r) => self.resolve_def(*r, func_lib).unwrap().events.len(),
            NodeKind::SubgraphInput => 1,
            NodeKind::SubgraphOutput => 0,
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
                    InputPort { node_id, port_idx },
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
                    InputPort { node_id, port_idx },
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
            persist: CachePersistence::Memory,
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
            persist: CachePersistence::Memory,
            disabled: false,
        }
    }
}

impl From<&Func> for Node {
    /// A bare func instance (identity + name). Default input bindings are seeded
    /// by `Graph::add_func_node`.
    fn from(func: &Func) -> Self {
        Node {
            id: NodeId::unique(),
            kind: NodeKind::Func(func.id),
            name: func.name.clone(),
            persist: CachePersistence::Memory,
            disabled: false,
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

#[cfg(test)]
mod tests {
    use crate::data::DataType;
    use crate::function::{Func, FuncInput};
    use crate::graph::{
        Binding, CachePersistence, Graph, InputPort, Node, NodeId, NodeKind, OutputPort,
        Subscription,
    };
    use crate::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
    use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
    use common::SerdeFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();

        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = graph.serialize(format)?;
            let deserialized = Graph::deserialize(&serialized, format)?;
            let serialized_again = deserialized.serialize(format)?;
            assert_eq!(serialized, serialized_again);
        }

        let bin = graph.serialize(SerdeFormat::Bitcode)?;
        let deserialized = Graph::deserialize(&bin, SerdeFormat::Bitcode)?;
        assert_eq!(graph, deserialized);

        Ok(())
    }

    #[test]
    fn check_passes_for_valid_graph() {
        assert!(test_graph().check().is_ok());
    }

    #[test]
    fn persist_round_trips_and_defaults_to_memory() {
        use common::deserialize;
        assert_eq!(CachePersistence::default(), CachePersistence::Memory);

        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut graph = Graph::default();
        let mut node: Node = func_lib.by_name("get_a").unwrap().into();
        node.persist = CachePersistence::Disk;
        graph.add(node);

        for format in [SerdeFormat::Json, SerdeFormat::Bitcode] {
            let bytes = graph.serialize(format).unwrap();
            let back = Graph::deserialize(&bytes, format).unwrap();
            assert_eq!(
                back.by_name("get_a").unwrap().persist,
                CachePersistence::Disk
            );
        }

        // A node authored before `persist` existed deserializes as `Memory`.
        let legacy = r#"{ "id": "00000000-0000-0000-0000-000000000001",
            "kind": { "Func": "00000000-0000-0000-0000-000000000002" }, "name": "n" }"#;
        let node: Node = deserialize(legacy.as_bytes(), SerdeFormat::Json).unwrap();
        assert_eq!(node.persist, CachePersistence::Memory);
    }

    #[test]
    fn check_rejects_dangling_binding() {
        let mut graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        // Repoint sum's input at a node that doesn't exist.
        graph.set_input_binding(InputPort::new(sum_id, 0), (NodeId::unique(), 0).into());

        let err = graph.check().expect_err("dangling binding must fail check");
        assert!(err.to_string().contains("binds to missing node"));
    }

    #[test]
    fn deserialize_rejects_corrupt_graph() {
        let mut graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        graph.set_input_binding(InputPort::new(sum_id, 0), (NodeId::unique(), 0).into());

        // serialize doesn't validate; deserialize must reject the dangling bind
        // (the release-path structural guard, not a debug-only assert).
        let bytes = graph.serialize(SerdeFormat::Bitcode).unwrap();
        assert!(Graph::deserialize(&bytes, SerdeFormat::Bitcode).is_err());
    }

    #[test]
    fn node_remove_test() -> anyhow::Result<()> {
        let mut graph = test_graph();

        let node_id = graph.by_name("sum").unwrap().id;
        graph.remove_by_id(node_id);

        assert!(graph.by_name("sum").is_none());
        assert_eq!(graph.len(), 4);

        // No surviving edge references the removed node (as consumer or producer).
        for (dst, src) in graph.edges() {
            assert_ne!(dst.node_id, node_id);
            assert_ne!(src.node_id, node_id);
        }

        Ok(())
    }

    // === Accessors ===

    #[test]
    fn node_kind_accessors() {
        let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
        let func = NodeKind::Func(func_id);
        assert_eq!(func.as_func(), Some(func_id));
        assert_eq!(func.as_subgraph(), None);
        assert!(!func.is_boundary());

        let sub_id = SubgraphId::unique();
        let sub = NodeKind::Subgraph(SubgraphRef::Local(sub_id));
        assert_eq!(sub.as_func(), None);
        assert_eq!(sub.as_subgraph().map(|r| r.id()), Some(sub_id));
        assert!(!sub.is_boundary());

        assert!(NodeKind::SubgraphInput.is_boundary());
        assert!(NodeKind::SubgraphOutput.is_boundary());
        assert_eq!(NodeKind::SubgraphInput.as_func(), None);
        assert_eq!(NodeKind::SubgraphOutput.as_subgraph(), None);
    }

    #[test]
    fn node_func_id_shims_kind() {
        let func_id = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();
        assert_eq!(Node::new(NodeKind::Func(func_id)).func_id(), Some(func_id));
        assert_eq!(Node::new(NodeKind::SubgraphInput).func_id(), None);
    }

    #[test]
    fn binding_accessors() {
        let out = OutputPort {
            node_id: NodeId::unique(),
            port_idx: 2,
        };
        let bind = Binding::Bind(out);
        assert_eq!(bind.as_output_binding(), Some(&out));
        assert!(bind.is_some());
        assert!(!bind.is_none());

        let konst = Binding::from(5i64);
        assert_eq!(konst.as_output_binding(), None);
        assert!(konst.is_some()); // a Const is a real binding
        assert!(!konst.is_none());

        let none = Binding::None;
        assert_eq!(none.as_output_binding(), None);
        assert!(!none.is_some());
        assert!(none.is_none());
    }

    #[test]
    fn binding_conversions() {
        let nid = NodeId::unique();
        let from_port: Binding = OutputPort {
            node_id: nid,
            port_idx: 1,
        }
        .into();
        let from_tuple: Binding = (nid, 1usize).into();
        assert_eq!(from_port, from_tuple);
        assert_eq!(from_port.as_output_binding().unwrap().port_idx, 1);

        assert_eq!(Binding::from(7i64), Binding::Const(7i64.into()));
    }

    // === Data bindings ===

    #[test]
    fn node_bindings_yields_ports_in_order_with_none_gaps() {
        let graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let get_b_id = graph.by_name("get_b").unwrap().id;

        // sum has two bound inputs; ask for arity 3 to exercise the unbound gap.
        let bindings: Vec<_> = graph.node_bindings(sum_id, 3).collect();
        assert_eq!(
            bindings,
            vec![
                (
                    0,
                    Binding::Bind(OutputPort {
                        node_id: get_a_id,
                        port_idx: 0
                    })
                ),
                (
                    1,
                    Binding::Bind(OutputPort {
                        node_id: get_b_id,
                        port_idx: 0
                    })
                ),
                (2, Binding::None),
            ]
        );
    }

    // === Event subscriptions ===

    #[test]
    fn subscribe_unsubscribe_is_subscribed() {
        let graph = test_graph();
        let emitter = graph.by_name("get_a").unwrap().id;
        let sub = graph.by_name("sum").unwrap().id;
        let mut graph = graph;

        assert!(!graph.is_subscribed(emitter, 0, sub));
        graph.subscribe(emitter, 0, sub);
        assert!(graph.is_subscribed(emitter, 0, sub));

        // Distinct event_idx is a distinct edge.
        assert!(!graph.is_subscribed(emitter, 1, sub));

        // Re-subscribing is idempotent (BTreeSet dedups).
        graph.subscribe(emitter, 0, sub);
        assert_eq!(graph.subscriptions().count(), 1);

        graph.unsubscribe(emitter, 0, sub);
        assert!(!graph.is_subscribed(emitter, 0, sub));
        assert_eq!(graph.subscriptions().count(), 0);
    }

    #[test]
    fn subscribers_ranges_one_emitter_event() {
        let mut graph = test_graph();
        let emitter = graph.by_name("get_a").unwrap().id;
        let s1 = graph.by_name("sum").unwrap().id;
        let s2 = graph.by_name("mult").unwrap().id;
        let other = graph.by_name("print").unwrap().id;

        graph.subscribe(emitter, 0, s1);
        graph.subscribe(emitter, 0, s2);
        graph.subscribe(emitter, 1, other); // different event: must not leak in

        let mut got: Vec<NodeId> = graph.subscribers(emitter, 0).collect();
        got.sort();
        let mut want = vec![s1, s2];
        want.sort();
        assert_eq!(got, want);

        assert_eq!(
            graph.subscribers(emitter, 1).collect::<Vec<_>>(),
            vec![other]
        );
        assert_eq!(graph.subscribers(emitter, 2).count(), 0);
    }

    // === Snapshot / restore (editor undo) ===

    #[test]
    fn wiring_snapshot_round_trips_through_restore() {
        let mut graph = test_graph();
        let sum_id = graph.by_name("sum").unwrap().id;
        let get_a_id = graph.by_name("get_a").unwrap().id;

        // Add a subscription that touches `sum` so both arms are exercised.
        graph.subscribe(get_a_id, 0, sum_id);

        let node = graph.by_id(&sum_id).unwrap().clone();
        let bindings = graph.bindings_touching(sum_id);
        let subs = graph.subscriptions_touching(sum_id);

        // sum touches: its own inputs (sum,0),(sum,1) + the edge (mult,0)<-sum.
        assert_eq!(bindings.len(), 3);
        assert_eq!(
            subs,
            vec![Subscription {
                emitter: get_a_id,
                event_idx: 0,
                subscriber: sum_id
            }]
        );

        let edges_before = graph.edges().count();
        graph.remove_by_id(sum_id);
        assert_eq!(graph.edges().count(), edges_before - 3);
        assert!(graph.subscriptions_touching(sum_id).is_empty());

        // Undo: re-add the node, then re-apply its wiring.
        graph.add(node);
        graph.restore_wiring(&bindings, &subs);

        assert_eq!(graph.edges().count(), edges_before);
        assert!(graph.is_subscribed(get_a_id, 0, sum_id));
        assert_eq!(graph.bindings_touching(sum_id), bindings);
    }

    // === Construction helpers seed default const bindings ===

    fn func_with_default(default: i64) -> Func {
        Func {
            name: "withdefault".into(),
            inputs: vec![FuncInput {
                name: "x".into(),
                required: false,
                data_type: DataType::Int,
                default_value: Some(default.into()),
                value_variants: vec![],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn add_func_node_seeds_default_const_binding() {
        let func = func_with_default(7);
        let mut graph = Graph::default();
        let id = graph.add_func_node(&func);

        assert_eq!(graph.by_id(&id).unwrap().func_id(), Some(func.id));
        assert_eq!(
            graph.input_binding(InputPort::new(id, 0)),
            Binding::Const(7i64.into())
        );
    }

    #[test]
    fn add_func_node_leaves_defaultless_inputs_unbound() {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let sum = func_lib.by_name("sum").unwrap(); // inputs have no defaults
        let mut graph = Graph::default();
        let id = graph.add_func_node(sum);

        assert_eq!(graph.input_binding(InputPort::new(id, 0)), Binding::None);
        assert_eq!(graph.input_binding(InputPort::new(id, 1)), Binding::None);
    }

    #[test]
    fn add_subgraph_node_seeds_default_const_binding() {
        let mut input = FuncInput {
            name: "A".into(),
            required: false,
            data_type: DataType::Int,
            default_value: Some(3i64.into()),
            value_variants: vec![],
        };
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Def".into(),
            category: "Test".into(),
            graph: Graph::default(),
            inputs: vec![input.clone(), {
                input.default_value = None;
                input
            }],
            outputs: vec![],
            events: vec![],
            origin: None,
        };

        let mut graph = Graph::default();
        let id = graph.add_subgraph_node(&def, SubgraphRef::Local(def.id));

        // Port 0 had a default; port 1 did not.
        assert_eq!(
            graph.input_binding(InputPort::new(id, 0)),
            Binding::Const(3i64.into())
        );
        assert_eq!(graph.input_binding(InputPort::new(id, 1)), Binding::None);
    }

    // === Definition resolution ===

    #[test]
    fn resolve_def_picks_local_or_linked_source() {
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        let linked_id = SubgraphId::unique();
        func_lib.add_subgraph(SubgraphDef {
            id: linked_id,
            name: "Linked".into(),
            category: "Test".into(),
            graph: Graph::default(),
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            origin: None,
        });

        let mut graph = Graph::default();
        let local_id = SubgraphId::unique();
        graph.subgraphs.add(SubgraphDef {
            id: local_id,
            name: "Local".into(),
            category: "Test".into(),
            graph: Graph::default(),
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            origin: None,
        });

        assert_eq!(
            graph
                .resolve_def(SubgraphRef::Local(local_id), &func_lib)
                .unwrap()
                .name,
            "Local"
        );
        assert_eq!(
            graph
                .resolve_def(SubgraphRef::Linked(linked_id), &func_lib)
                .unwrap()
                .name,
            "Linked"
        );
        // A local ref whose id only exists in the func_lib does not resolve.
        assert!(
            graph
                .resolve_def(SubgraphRef::Local(linked_id), &func_lib)
                .is_none()
        );
    }
}
