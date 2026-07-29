//! One authored node, and the vocabulary for talking about one.
//!
//! A [`Node`] is authored data and nothing else — it does not store its own id,
//! which lives only in the key that reaches it, so a node can be moved or
//! remapped without carrying a stale identity. What it *is* is [`NodeKind`]: a
//! func instance, a graph instance, a boundary port, or a built-in special.
//! What it does with its result is [`CacheMode`]. How a caller reaches one is
//! [`NodeSearch`] (this graph, or the whole nested tree) and what comes back is
//! a [`NodeRef`], which re-attaches the id the lookup found it by.
//!
//! What a func node instantiates — the [`Func`](crate::graph::func::Func)
//! declaration and the ABIs it runs through — is
//! [`func`](crate::graph::func)'s. The module below owns the built-in
//! [`special`] nodes.

pub(crate) mod special;

use ::serde::{Deserialize, Serialize};

use crate::graph::definition::{GraphDef, GraphLink};
use crate::graph::func::Func;
use crate::graph::identity::FuncId;
use crate::graph::node::special::SpecialNode;

/// Where a node's computed output is cached — the two orthogonal storage bits
/// *keep in RAM* ([`caches_in_ram`](Self::caches_in_ram)) and *persist to disk*
/// ([`persists_to_disk`](Self::persists_to_disk)) as one four-state enum:
///
/// - `None` — cache nowhere: never reused across runs, recomputed whenever its value
///   is needed, and dropped after the run to free RAM.
/// - `Ram` — current reproducible values stay resident in the live engine and are reused
///   across runs, but are lost on reload.
/// - `Disk` — persisted to the disk store (survives reload); its RAM copy
///   is dropped after the run and reloaded lazily when demanded.
/// - `Both` — current reproducible values stay resident in RAM *and* on disk: hot reuse
///   this session plus survival across reloads.
///
/// This is a *storage* choice only — it never affects reproducibility. Disk/RAM reuse is
/// honored only for a node with a content digest (a reproducible cone); a node with an
/// impure node anywhere upstream has no digest, so its output is released after the run
/// and never risks serving a stale value, whatever its mode. The on-disk backend is wired
/// only once a caller attaches a `DiskStore` with a disk root; until
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
    /// Whether a current reproducible value is retained in RAM and reused across runs
    /// (`Ram`/`Both`). The other modes drop the RAM copy after each run.
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

impl NodeKind {
    pub fn is_boundary(&self) -> bool {
        matches!(self, NodeKind::GraphInput | NodeKind::GraphOutput)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub name: String,

    /// Where this node's output is cached. See [`CacheMode`]. A fresh func node
    /// (`From<&Func>`) or special node (`Node::new`) copies its func's
    /// `default_cache_mode`; the remaining func-less constructors seed `None`.
    pub cache: CacheMode,

    /// Disabled nodes remain in the compiled program but ambient planning
    /// excludes them. A binding from one behaves like an unbound input unless
    /// the disabled producer is explicitly included in that run's node seeds.
    pub disabled: bool,
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

    /// A graph instance node shaped from the referenced definition.
    pub fn graph_instance(def: &GraphDef, link: GraphLink) -> Self {
        Node {
            kind: NodeKind::Graph(link),
            name: def.name.clone(),
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
