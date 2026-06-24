//! Built-in "special" nodes: node kinds with a hardcoded declaration that the
//! engine recognizes by identity, rather than user-registered [`Func`]s.
//!
//! Modeling them as a [`SpecialNode`] enum variant on the node's kind (vs. a flag
//! on every [`Func`]/`Node`) keeps the common path clean: a new special case is a
//! new variant plus a hardcoded spec here, with no new field elsewhere. The
//! interface + passthrough lambda come from [`special_func`]; the engine then
//! special-cases the node's *behavior* (e.g. the cache node's path-keyed
//! load/store and input pruning — see `docs/file-cache-design.md`).

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::data::{DataType, FsPathConfig, FsPathMode};
use crate::function::{Func, FuncInput};

/// A built-in node identified by *kind*, not by a `FuncId`. Its ports + lambda
/// come from [`special_func`] (which ignores any per-instance config carried in
/// the variant); the engine gives it special behavior.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SpecialNode {
    /// Passthrough that caches `output[0]` to the file named by the `Const`
    /// `FsPath` in `input[1]`. When that file exists, the value is loaded from it
    /// and `input[0]`'s upstream is pruned (not computed). See
    /// `docs/file-cache-design.md`.
    ///
    /// `bypass`: ignore any existing cache file and recompute + overwrite every
    /// run (the UI bypass toggle). Per-instance config, so it rides in the variant
    /// rather than as a field on every node.
    CachePassthrough { bypass: bool },
}

/// Every special node (default config), for the editor's node-add menu.
pub const ALL: &[SpecialNode] = &[SpecialNode::CachePassthrough { bypass: false }];

/// Input index of a `CachePassthrough` node's `path` (the second input declared by
/// [`cache_passthrough_func`]). The output cache and the path-keyed digest read the
/// path from here — keep it in step with the input order below.
pub(crate) const CACHE_PATH_INPUT: usize = 1;

/// Stable `FuncId` standing in for the cache node in the flattened program (digest
/// memo / stats attribution). Not registered in any `FuncLib`.
// generated with uuidgen
const CACHE_PASSTHROUGH_FUNC_ID: &str = "2a969ecc-92b7-4136-9c4a-86491c9621d3";

/// The hardcoded interface + passthrough lambda for `kind`, built once. Used by
/// flatten (ports, lambda, behavior), validation (port arity), and the editor
/// (rendering + the node menu). The value port is `Null` — a wildcard, since
/// graph validation doesn't type-check bindings — so any type passes through.
pub fn special_func(kind: SpecialNode) -> &'static Func {
    match kind {
        SpecialNode::CachePassthrough { .. } => {
            static F: OnceLock<Func> = OnceLock::new();
            F.get_or_init(cache_passthrough_func)
        }
    }
}

fn cache_passthrough_func() -> Func {
    Func::new(CACHE_PASSTHROUGH_FUNC_ID, "file cache")
        .category("cache")
        .description(
            "Passes its input through unchanged and caches it to the file at `path`. \
             While that file exists the value is loaded from it and the input's \
             upstream is not recomputed.",
        )
        .input(FuncInput::required("value", DataType::Null))
        // The path input — its index is [`CACHE_PATH_INPUT`].
        .input(FuncInput::required(
            "path",
            DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::NewFile))),
        ))
        .output("value", DataType::Null)
        .lambda(crate::async_lambda!(|_, _, _, inputs, _, outputs| {
            // The engine does the path-keyed file I/O; the node is a plain
            // passthrough of `input[0]`.
            outputs[0] = inputs[0].value.clone();
            Ok(())
        }))
}
