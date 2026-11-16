//! One authored node, and the vocabulary for talking about one.
//!
//! A [`Node`] does not store its own id, which lives only in the key that reaches it, so a node
//! can be moved or remapped without carrying a stale identity. What it *is* is [`NodeKind`]: a
//! node with a body, a library func or a built-in special, an instance of a subgraph, or one of a
//! subgraph's two boundary nodes. Iterating a graph yields a
//! [`NodeRef`](crate::graph::node_ref::NodeRef), pairing a node with the id it was found under;
//! the lookups that *take* an id return the node alone.
//!
//! What a node declares resolves through a [`Scope`] into a [`NodeDecl`]; the built-in
//! [`special`] nodes are below.

pub(crate) mod special;

use crate::graph::func::Func;
use crate::graph::identity::{FuncId, SubgraphId};
use crate::graph::node::special::SpecialNode;
use crate::graph::node_decl::NodeDecl;
use crate::graph::scope::Scope;
use ::serde::{Deserialize, Serialize};

/// Where a node's computed output is cached: the storage bits *keep in RAM*
/// ([`caches_in_ram`](Self::caches_in_ram)) and *persist to disk*
/// ([`persists_to_disk`](Self::persists_to_disk)) as one enum:
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
/// then `Disk`/`Both` degrade to memory-only.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum CacheMode {
    #[default]
    None,
    Ram,
    Disk,
    Both,
}

impl CacheMode {
    /// Whether a current reproducible value is retained in RAM and reused across runs.
    pub const fn caches_in_ram(self) -> bool {
        matches!(self, Self::Ram | Self::Both)
    }

    /// Whether the node's value is persisted to the disk store, so it survives a reload.
    pub const fn persists_to_disk(self) -> bool {
        matches!(self, Self::Disk | Self::Both)
    }

    /// The inverse of [`caches_in_ram`](Self::caches_in_ram) and
    /// [`persists_to_disk`](Self::persists_to_disk).
    pub const fn from_bits(ram: bool, disk: bool) -> Self {
        match (ram, disk) {
            (false, false) => Self::None,
            (true, false) => Self::Ram,
            (false, true) => Self::Disk,
            (true, true) => Self::Both,
        }
    }
}

/// What a node *is*. Every kind resolves to a [`NodeDecl`] through a [`Scope`], so one admission
/// rule holds for all; only a node with a body runs and carries a [`CacheMode`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// A node that runs: a library func or a built-in special, cached as `cache` says.
    Body {
        source: BodySource,
        cache: CacheMode,
    },
    /// An instance of the subgraph under the id. It has no cache mode: the nodes inside carry
    /// their own.
    Instance(SubgraphId),
    /// One of a subgraph's two boundary nodes, which show its interface from inside.
    Boundary(Boundary),
}

/// What a node with a body runs.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum BodySource {
    /// An instance of the library func under the id.
    Func(FuncId),
    /// A built-in special node, whose declaration is [`SpecialNode::func`].
    Special(SpecialNode),
}

/// Which of a subgraph's two boundary nodes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Boundary {
    /// The node whose outputs are the interface's inputs.
    Inputs,
    /// The node whose inputs are the interface's outputs.
    Outputs,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub(crate) kind: NodeKind,
    pub name: String,

    /// Disabled nodes remain in the compiled program but ambient planning
    /// excludes them. A binding from one behaves like an unbound input unless
    /// the disabled producer is explicitly included in that run's node seeds.
    /// A disabled instance disables every node inside it.
    pub disabled: bool,
}

impl Node {
    /// What this node declares: a library func, a special node's built-in, or the ports a
    /// subgraph's interface gives its instances and boundary nodes.
    ///
    /// # Panics
    ///
    /// If `scope` does not resolve the node. A graph holds only nodes that
    /// resolve — [`GraphSet::reconcile`](crate::GraphSet::reconcile) keeps the
    /// rest as ghost nodes at load — so a miss is a broken contract; a node that may not
    /// resolve is asked through [`Scope::resolve`].
    pub fn decl<'a>(&self, scope: Scope<'a>) -> NodeDecl<'a> {
        match scope.resolve(self) {
            Ok(decl) => decl,
            Err(reason) => panic!(
                "a graph node resolves, and {:?} does not: {reason}",
                self.name
            ),
        }
    }

    /// The id of the func this node instantiates, or `None` for any other kind.
    pub const fn func_id(&self) -> Option<FuncId> {
        match self.kind {
            NodeKind::Body {
                source: BodySource::Func(func_id),
                ..
            } => Some(func_id),
            _ => None,
        }
    }

    /// The id of the func this node's declaration is, a built-in's included, or `None` for a
    /// node with no body: what a [`Manifest`](crate::Manifest) lists the declaration under.
    pub(crate) fn declared_func(&self) -> Option<FuncId> {
        match self.kind {
            NodeKind::Body {
                source: BodySource::Func(func_id),
                ..
            } => Some(func_id),
            NodeKind::Body {
                source: BodySource::Special(special),
                ..
            } => Some(special.func().id()),
            NodeKind::Instance(_) | NodeKind::Boundary(_) => None,
        }
    }

    /// The built-in this node is, if it is one.
    pub const fn special(&self) -> Option<SpecialNode> {
        match self.kind {
            NodeKind::Body {
                source: BodySource::Special(special),
                ..
            } => Some(special),
            _ => None,
        }
    }

    /// The subgraph this node is an instance of, if it is one.
    pub const fn subgraph(&self) -> Option<SubgraphId> {
        match self.kind {
            NodeKind::Instance(subgraph_id) => Some(subgraph_id),
            _ => None,
        }
    }

    /// Point this instance at subgraph `id`, as Make Unique points the instances it picks at the
    /// copy it makes.
    ///
    /// # Panics
    /// If this node is no instance.
    pub fn set_subgraph(&mut self, id: SubgraphId) {
        let NodeKind::Instance(placed) = &mut self.kind else {
            panic!("only an instance places a subgraph");
        };
        *placed = id;
    }

    /// The boundary node this is, if it is one.
    pub const fn boundary(&self) -> Option<Boundary> {
        match self.kind {
            NodeKind::Boundary(boundary) => Some(boundary),
            _ => None,
        }
    }

    /// Where this node's output is cached, or `None` for a node with no body: an instance or a
    /// boundary node. A fresh func node (`From<&Func>`) or special node (`From<SpecialNode>`)
    /// copies its func's `default_cache_mode`.
    pub const fn cache(&self) -> Option<CacheMode> {
        match self.kind {
            NodeKind::Body { cache, .. } => Some(cache),
            NodeKind::Instance(_) | NodeKind::Boundary(_) => None,
        }
    }

    /// Mutable counterpart of [`Self::cache`].
    pub const fn cache_mut(&mut self) -> Option<&mut CacheMode> {
        match &mut self.kind {
            NodeKind::Body { cache, .. } => Some(cache),
            NodeKind::Instance(_) | NodeKind::Boundary(_) => None,
        }
    }

    /// An instance of the subgraph held under `id`, with no name of its own: a host shows the
    /// subgraph's name until the instance is named, so a renamed subgraph reads renamed on each
    /// instance.
    pub const fn instance(id: SubgraphId) -> Self {
        Self {
            kind: NodeKind::Instance(id),
            name: String::new(),
            disabled: false,
        }
    }
}

impl From<&Func> for Node {
    /// Copies the func's name and `default_cache_mode`; `Graph::add_func_node` seeds the default
    /// input bindings.
    fn from(func: &Func) -> Self {
        Self {
            kind: NodeKind::Body {
                source: BodySource::Func(func.id()),
                cache: func.default_cache_mode(),
            },
            name: func.name().to_owned(),
            disabled: false,
        }
    }
}

impl From<SpecialNode> for Node {
    /// A built-in, named and cached as its hardcoded declaration says.
    fn from(special: SpecialNode) -> Self {
        let func = special.func();
        Self {
            kind: NodeKind::Body {
                source: BodySource::Special(special),
                cache: func.default_cache_mode(),
            },
            name: func.name().to_owned(),
            disabled: false,
        }
    }
}

impl From<Boundary> for Node {
    /// A subgraph's boundary node, named for its side.
    fn from(boundary: Boundary) -> Self {
        Self {
            kind: NodeKind::Boundary(boundary),
            name: boundary.name().to_owned(),
            disabled: false,
        }
    }
}

impl Boundary {
    /// The name a boundary node shows on its side.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Inputs => "Inputs",
            Self::Outputs => "Outputs",
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::identity::FuncId;
    use crate::graph::node::{BodySource, CacheMode, Node, NodeKind};

    impl Node {
        /// A node of func `func_id` with no wiring, before anything resolved its
        /// func: unnamed and on `CacheMode::None`, as a document saved against a
        /// library that later dropped the func leaves it.
        pub(crate) const fn unresolved(func_id: FuncId) -> Self {
            Self {
                kind: NodeKind::Body {
                    source: BodySource::Func(func_id),
                    cache: CacheMode::None,
                },
                name: String::new(),
                disabled: false,
            }
        }
    }
}
