//! Built-in "special" nodes: node kinds with a hardcoded declaration that the
//! engine recognizes by identity, rather than user-registered [`Func`]s.
//!
//! Modeling them as a [`SpecialNode`] enum variant on the node's kind (vs. a flag
//! on every [`Func`]/`Node`) keeps the common path clean: a new special case is a
//! new variant plus a hardcoded spec — its interface + lambda live in an
//! `elements/` module (e.g. [`cache_passthrough`](crate::elements::cache_passthrough))
//! — with no new field elsewhere. [`SpecialNode::func`] maps a variant to that
//! interface; the engine then special-cases the node's *behavior* (e.g. the cache
//! node's path-keyed load/store and input pruning — see `docs/file-cache-design.md`).

use serde::{Deserialize, Serialize};

use crate::elements::cache_passthrough::cache_passthrough_func;
use crate::function::Func;

/// A built-in node identified by *kind*, not by a `FuncId`. Its ports + lambda
/// come from [`func`](SpecialNode::func) (which ignores any per-instance config
/// carried in the variant); the engine gives it special behavior.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SpecialNode {
    /// Passthrough that caches `output[0]` to the file named by the `Const`
    /// `FsPath` in `input[1]`. Its interface + lambda live in
    /// [`cache_passthrough`](crate::elements::cache_passthrough).
    ///
    /// `bypass`: ignore any existing cache file and recompute + overwrite every
    /// run (the UI bypass toggle). Per-instance config, so it rides in the variant
    /// rather than as a field on every node.
    CachePassthrough { bypass: bool },
}

/// Every special node (default config), for the editor's node-add menu.
pub const ALL: &[SpecialNode] = &[SpecialNode::CachePassthrough { bypass: false }];

impl SpecialNode {
    /// This node's hardcoded interface + passthrough lambda. Used by flatten (ports,
    /// lambda, behavior), validation (port arity), and the editor (rendering + the
    /// node menu).
    pub fn func(self) -> &'static Func {
        match self {
            SpecialNode::CachePassthrough { .. } => cache_passthrough_func(),
        }
    }
}
