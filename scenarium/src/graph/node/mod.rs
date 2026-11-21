//! One authored node, and the vocabulary for talking about one.
//!
//! A [`Node`] does not store its own id, which lives only in the key that reaches it, so a node
//! can be moved or remapped without carrying a stale identity. What it *is* is [`NodeKind`]: a
//! node with a body, a library func or a built-in special, an instance of a compound, or one of a
//! compound's two boundary nodes. Iterating a graph yields a
//! [`NodeRef`](crate::graph::node_ref::NodeRef), pairing a node with the id it was found under;
//! the lookups that *take* an id return the node alone.
//!
//! What a node declares resolves through a [`Scope`] into a [`NodeDecl`]; the built-in
//! [`special`] nodes are below.

pub(crate) mod special;

use crate::graph::func::Func;
use crate::graph::identity::{CompoundId, FuncId};
use crate::graph::node::special::SpecialNode;
use crate::graph::node_decl::NodeDecl;
use crate::graph::scope::Scope;
use ::serde::{Deserialize, Serialize};

/// How a node's computed output is kept between runs: how long RAM keeps it, and whether disk
/// holds a copy. Every combination is a mode a person may want, so the two are fields of their
/// own.
///
/// This is a *storage* choice only — it never affects reproducibility. A value is kept only for a
/// node with a content digest (a reproducible cone); a node with an impure node anywhere upstream
/// has no digest, so its output is released after the run and never risks serving a stale value,
/// whatever its mode. The on-disk backend is wired only once a caller attaches a `DiskStore` with a
/// disk root; until then the disk bit degrades to memory alone.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct CacheMode {
    /// How long RAM keeps the output.
    pub ram: Keep,
    /// The output is persisted to the disk store, so it survives a reload; a value RAM does not
    /// keep is read back from it when a run demands it.
    pub disk: bool,
}

/// How long RAM keeps a node's output after the run that made it.
///
/// - **Every output is kept by default, until memory runs short**, as Houdini keeps each SOP's
///   cooked geometry and Nuke its buffer cache, and ComfyUI caches every node's result
///   ([caching](https://docs.comfy.org/development/comfyui-server/startup-flags)): an edit then
///   recomputes what it changed and serves the rest. The engine drops the least recently used of
///   these while the memory the process can still allocate stays under its headroom.
/// - **`Never` is Houdini's Unload flag** ([flags](https://www.sidefx.com/docs/houdini/network/flags.html)):
///   a node whose output is cheap to make, or too big to keep, recomputes when demanded.
/// - **`Always` is Nuke's cached box**: a node whose output is dear to make stays resident whatever
///   the memory, until its digest changes.
///
/// Ordered by how long each keeps, the order [`CacheMode::join`] takes the longer of.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default, PartialOrd, Ord,
)]
pub enum Keep {
    /// Dropped once the run that made it has delivered it.
    Never,
    /// Kept until memory runs short, the least recently used dropped first.
    #[default]
    Auto,
    /// Kept whatever the memory.
    Always,
}

impl CacheMode {
    /// The default, kept in RAM until memory runs short and on no disk.
    pub const AUTO: Self = Self {
        ram: Keep::Auto,
        disk: false,
    };

    /// Kept in RAM whatever the memory, on no disk.
    pub const ALWAYS: Self = Self {
        ram: Keep::Always,
        disk: false,
    };

    /// Kept nowhere: recomputed whenever a run demands it.
    pub const NEVER: Self = Self {
        ram: Keep::Never,
        disk: false,
    };

    /// Whether a current reproducible value is retained in RAM after its run and reused across
    /// runs, while memory allows or always.
    pub const fn caches_in_ram(self) -> bool {
        !matches!(self.ram, Keep::Never)
    }

    /// Whether the node's value is persisted to the disk store, so it survives a reload.
    pub const fn persists_to_disk(self) -> bool {
        self.disk
    }

    /// What both modes keep: RAM as long as either keeps it, disk where either saves it.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        Self {
            ram: self.ram.max(other.ram),
            disk: self.disk || other.disk,
        }
    }
}

/// What a node *is*. Every kind resolves to a [`NodeDecl`] through a [`Scope`], so one admission
/// rule holds for all; only a node with a body runs, and it and an instance carry a
/// [`CacheMode`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// A node that runs: a library func or a built-in special, cached as `cache` says.
    Body {
        source: BodySource,
        cache: CacheMode,
    },
    /// An instance of `compound`. Its `cache` caches the values that leave it: the compile joins
    /// it into each program node the compound's `Outputs` node reads, at this instance's path;
    /// the nodes inside keep their own.
    Instance {
        compound: CompoundId,
        cache: CacheMode,
    },
    /// One of a compound's two boundary nodes, which show its interface from inside.
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

/// Which of a compound's two boundary nodes.
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
    /// The node's own name; `None` shows its declaration's, so a func or a compound renamed reads
    /// renamed on each node nobody named ([`Scope::title`]).
    #[serde(default)]
    pub name: Option<String>,

    /// Disabled nodes remain in the compiled program but ambient planning
    /// excludes them. A binding from one behaves like an unbound input unless
    /// the disabled producer is explicitly included in that run's node seeds.
    /// A disabled instance disables every node inside it.
    pub disabled: bool,
}

impl Node {
    /// What this node declares: a library func, a special node's built-in, or the ports a
    /// compound's interface gives its instances and boundary nodes.
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
            NodeKind::Instance { .. } | NodeKind::Boundary(_) => None,
        }
    }

    /// This node as a node of func `to`, its name, cache mode and disabled flag kept: what a load
    /// makes of a node of a func the library redirects.
    ///
    /// # Panics
    /// If this node runs no library func.
    pub(crate) fn redirected(&self, to: FuncId) -> Self {
        let NodeKind::Body {
            source: BodySource::Func(_),
            cache,
        } = self.kind
        else {
            panic!("only a func node is redirected");
        };
        Self {
            kind: NodeKind::Body {
                source: BodySource::Func(to),
                cache,
            },
            ..self.clone()
        }
    }

    /// Whether the node runs: a func or a built-in, which the compile emits as a program node,
    /// where an instance and a boundary node emit none.
    pub const fn has_body(&self) -> bool {
        matches!(self.kind, NodeKind::Body { .. })
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

    /// The compound this node is an instance of, if it is one.
    pub const fn compound(&self) -> Option<CompoundId> {
        match self.kind {
            NodeKind::Instance { compound, .. } => Some(compound),
            _ => None,
        }
    }

    /// Point this instance at compound `id`, as Make Unique points the instances it picks at the
    /// copy it makes.
    ///
    /// # Panics
    /// If this node is no instance.
    pub fn set_compound(&mut self, id: CompoundId) {
        let NodeKind::Instance {
            compound: placed, ..
        } = &mut self.kind
        else {
            panic!("only an instance places a compound");
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

    /// Where this node's output is cached, or an instance's outputs, or `None` for a boundary
    /// node. A fresh func node (`From<&Func>`) or special node (`From<SpecialNode>`) copies its
    /// func's `default_cache_mode`; a fresh instance caches nothing.
    pub const fn cache(&self) -> Option<CacheMode> {
        match self.kind {
            NodeKind::Body { cache, .. } | NodeKind::Instance { cache, .. } => Some(cache),
            NodeKind::Boundary(_) => None,
        }
    }

    /// Mutable counterpart of [`Self::cache`].
    pub const fn cache_mut(&mut self) -> Option<&mut CacheMode> {
        match &mut self.kind {
            NodeKind::Body { cache, .. } | NodeKind::Instance { cache, .. } => Some(cache),
            NodeKind::Boundary(_) => None,
        }
    }

    /// An instance of the compound held under `id`, with no name of its own.
    pub const fn instance(id: CompoundId) -> Self {
        Self {
            kind: NodeKind::Instance {
                compound: id,
                cache: CacheMode::AUTO,
            },
            name: None,
            disabled: false,
        }
    }
}

impl From<&Func> for Node {
    /// A node of `func` with no name of its own, cached as the func's `default_cache_mode`;
    /// `Graph::add_func_node` seeds the default input bindings.
    fn from(func: &Func) -> Self {
        Self {
            kind: NodeKind::Body {
                source: BodySource::Func(func.id()),
                cache: func.default_cache_mode(),
            },
            name: None,
            disabled: false,
        }
    }
}

impl From<SpecialNode> for Node {
    /// A built-in with no name of its own, cached as its hardcoded declaration says.
    fn from(special: SpecialNode) -> Self {
        Self {
            kind: NodeKind::Body {
                source: BodySource::Special(special),
                cache: special.func().default_cache_mode(),
            },
            name: None,
            disabled: false,
        }
    }
}

impl From<Boundary> for Node {
    /// A compound's boundary node, which shows its side's name.
    fn from(boundary: Boundary) -> Self {
        Self {
            kind: NodeKind::Boundary(boundary),
            name: None,
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
        /// func: unnamed and on the default `CacheMode`, as a document saved against a
        /// library that later dropped the func leaves it.
        pub(crate) const fn unresolved(func_id: FuncId) -> Self {
            Self {
                kind: NodeKind::Body {
                    source: BodySource::Func(func_id),
                    cache: CacheMode::AUTO,
                },
                name: None,
                disabled: false,
            }
        }
    }
}
