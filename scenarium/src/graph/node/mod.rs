//! One authored node, and the vocabulary for talking about one.
//!
//! A [`Node`] is authored data and nothing else — it does not store its own id,
//! which lives only in the key that reaches it, so a node can be moved or
//! remapped without carrying a stale identity. What it *is* is [`NodeKind`]: a
//! func instance, addressed by the [`FuncRef`] it was authored against, or a
//! built-in special. What it does with its result is
//! [`CacheMode`]. Iterating a graph yields a [`NodeRef`](crate::graph::node_ref::NodeRef),
//! pairing a node with
//! the id it was found under; the lookups that *take* an id return the node
//! alone.
//!
//! What a func node instantiates — the [`Func`]
//! declaration and the ABIs it runs through — is
//! [`func`](crate::graph::func)'s. The module below owns the built-in
//! [`special`] nodes.

pub(crate) mod special;

use crate::graph::func::Func;
use crate::graph::identity::{FuncId, FuncRef};
use crate::graph::node::special::SpecialNode;
use crate::library::Library;
use ::serde::{Deserialize, Serialize};

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
    /// Whether a current reproducible value is retained in RAM and reused across runs
    /// (`Ram`/`Both`). The other modes drop the RAM copy after each run.
    pub const fn caches_in_ram(self) -> bool {
        matches!(self, Self::Ram | Self::Both)
    }

    /// Whether the node's value is persisted to the disk store
    /// (`Disk`/`Both`), so it survives a reload.
    pub const fn persists_to_disk(self) -> bool {
        matches!(self, Self::Disk | Self::Both)
    }

    /// Compose a mode from the two storage bits — the inverse of
    /// [`caches_in_ram`](Self::caches_in_ram)/[`persists_to_disk`](Self::persists_to_disk),
    /// used by the editor's two independent cache toggles.
    pub const fn from_bits(ram: bool, disk: bool) -> Self {
        match (ram, disk) {
            (false, false) => Self::None,
            (true, false) => Self::Ram,
            (false, true) => Self::Disk,
            (true, true) => Self::Both,
        }
    }
}

/// What a node *is*. A plain `Func` instance, or a built-in [`SpecialNode`]
/// (hardcoded declaration, recognized by the engine).
///
/// Both resolve to a [`Func`] declaration — the only difference is where that
/// declaration comes from. Internal to the crate: hosts read a node through
/// [`Node::func_id`], [`Node::special`] and [`Node::func`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// An instance of the library func the [`FuncRef`] names, authored against
    /// the ports it records — which its bindings and subscriptions are indexed by.
    Func(FuncRef),
    /// A built-in special node; its interface comes from
    /// [`SpecialNode::func`].
    Special(SpecialNode),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub(crate) kind: NodeKind,
    pub name: String,

    /// Where this node's output is cached. See [`CacheMode`]. A fresh func node
    /// (`From<&Func>`) or special node (`From<SpecialNode>`) copies its func's
    /// `default_cache_mode`; a node whose func nothing resolved seeds `None`.
    pub cache: CacheMode,

    /// Disabled nodes remain in the compiled program but ambient planning
    /// excludes them. A binding from one behaves like an unbound input unless
    /// the disabled producer is explicitly included in that run's node seeds.
    pub disabled: bool,
}

impl Node {
    /// The declaration this node instantiates: a library func, or a special
    /// node's built-in.
    ///
    /// # Panics
    ///
    /// If `library` holds no func under the node's id. A graph holds only nodes
    /// that resolve — [`Graph::reconcile`](crate::Graph::reconcile) detaches the
    /// rest at load — so a miss is a broken contract; a node that may not
    /// resolve is asked through [`Library::func_of`].
    pub fn func<'a>(&self, library: &'a Library) -> &'a Func {
        match &self.kind {
            NodeKind::Func(func_ref) => {
                let func = library
                    .by_id(func_ref.id())
                    .expect("a graph node's func is in the library");
                debug_assert_eq!(
                    func.func_ref(),
                    *func_ref,
                    "a graph node's func has the ports it was authored against"
                );
                func
            }
            NodeKind::Special(special) => special.func(),
        }
    }

    /// The id of the func this node instantiates, or `None` for a built-in.
    pub const fn func_id(&self) -> Option<FuncId> {
        match self.kind {
            NodeKind::Func(func_ref) => Some(func_ref.id()),
            NodeKind::Special(_) => None,
        }
    }

    /// The built-in this node is, if it is one.
    pub const fn special(&self) -> Option<SpecialNode> {
        match self.kind {
            NodeKind::Special(special) => Some(special),
            NodeKind::Func(_) => None,
        }
    }
}

impl From<&Func> for Node {
    /// A bare func instance copying the func's name and `default_cache_mode`.
    /// Default input bindings are seeded by `Graph::add_func_node`.
    fn from(func: &Func) -> Self {
        Self {
            kind: NodeKind::Func(func.func_ref()),
            name: func.name().to_owned(),
            cache: func.default_cache_mode(),
            disabled: false,
        }
    }
}

impl From<SpecialNode> for Node {
    /// A built-in, named and cached as its hardcoded declaration says.
    fn from(special: SpecialNode) -> Self {
        let func = special.func();
        Self {
            kind: NodeKind::Special(special),
            name: func.name().to_owned(),
            cache: func.default_cache_mode(),
            disabled: false,
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::identity::FuncRef;
    use crate::graph::node::{CacheMode, Node, NodeKind};

    impl Node {
        /// A node of `func_ref` with no wiring, before anything resolved its
        /// func: unnamed and on `CacheMode::None`, as a document saved against a
        /// library that later dropped the func leaves it.
        pub(crate) const fn unresolved(func_ref: FuncRef) -> Self {
            Self {
                kind: NodeKind::Func(func_ref),
                name: String::new(),
                cache: CacheMode::None,
                disabled: false,
            }
        }
    }
}
