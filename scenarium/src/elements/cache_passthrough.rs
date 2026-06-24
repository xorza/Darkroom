//! The [`CachePassthrough`](crate::special::SpecialNode::CachePassthrough) special
//! node: a passthrough that caches `output[0]` to the file named by the `Const`
//! `FsPath` in `input[1]`. When that file exists, the value is loaded from it and
//! `input[0]`'s upstream is pruned (not computed). The engine does the path-keyed
//! file I/O; this module only supplies the node's interface + passthrough lambda.
//! See `docs/file-cache-design.md`.

use std::sync::{Arc, OnceLock};

use blake3::Hasher;

use crate::data::{DataType, FsPathConfig, FsPathMode, StaticValue};
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionBinding, ExecutionInput};
use crate::function::{Func, FuncInput};

/// Domain separator for a file-cache node's path-keyed digest, kept distinct from
/// the normal node-content digest domain so a path can't collide with a normal
/// node's content hash.
const DOMAIN_FILECACHE: &[u8] = b"scenarium-filecache-v1";

/// Input index of a `CachePassthrough` node's `path` (the second input declared by
/// [`cache_passthrough_func`]). The output cache and the path-keyed digest read the
/// path from here — keep it in step with the input order below.
pub(crate) const CACHE_PATH_INPUT: usize = 1;

/// The `Const` `FsPath` at `node_inputs`'s [`CACHE_PATH_INPUT`] — for a
/// [`CachePassthrough`](crate::special::SpecialNode) node, its cache key and
/// load/store location. The single resolver, shared with the output cache (via
/// `pub(crate)`), so the path-keyed digest and the load/store can't disagree on the
/// key. `node_inputs` is the node's slice of the program input pool. `None` for a
/// non-const / empty path. Assumes the node is a cache node.
pub(crate) fn cache_node_path(node_inputs: &[ExecutionInput]) -> Option<&str> {
    let path_input = node_inputs.get(CACHE_PATH_INPUT)?;
    let ExecutionBinding::Const(StaticValue::FsPath(path)) = &path_input.binding else {
        return None;
    };
    (!path.is_empty()).then_some(path.as_str())
}

/// Path-keyed digest of a [`SpecialNode::CachePassthrough`](crate::special::SpecialNode)
/// node: a hash of its `Const` `FsPath` *alone* — deliberately ignoring `input[0]`'s
/// cone, so the file is the sole cache key (the node's whole point). A non-const or
/// empty path ⇒ `None` (never a hit, never stored). The file's `(len, mtime)` is
/// *not* folded in — presence, not content, decides the hit.
pub(crate) fn file_cache_digest(node_inputs: &[ExecutionInput]) -> Option<Digest> {
    let path = cache_node_path(node_inputs)?;
    let mut hasher = Hasher::new();
    hasher.update(DOMAIN_FILECACHE);
    hasher.update(path.as_bytes());
    Some(hasher.finalize().into())
}

/// Stable `FuncId` standing in for the cache node in the flattened program (digest
/// memo / stats attribution). Not registered in any `FuncLib`.
// generated with uuidgen
const CACHE_PASSTHROUGH_FUNC_ID: &str = "2a969ecc-92b7-4136-9c4a-86491c9621d3";

/// The hardcoded interface + passthrough lambda for `CachePassthrough`, built once.
/// The value port is `Null` — a wildcard, since graph validation doesn't type-check
/// bindings — so any type passes through.
pub(crate) fn cache_passthrough_func() -> &'static Func {
    static F: OnceLock<Func> = OnceLock::new();
    F.get_or_init(build_func)
}

fn build_func() -> Func {
    Func::new(CACHE_PASSTHROUGH_FUNC_ID, "file cache")
        .category("cache")
        .description(
            "Passes its input through unchanged and caches it to the file at `path`. \
             While that file exists the value is loaded from it and the input's \
             upstream is not recomputed.",
        )
        .input(FuncInput::required("value", DataType::Null))
        // The path input — its index is [`CACHE_PATH_INPUT`]. Const-only: the
        // engine reads it as a literal `FsPath`, so a wired binding would silently
        // disable caching (see `cache_node_path`).
        .input(
            FuncInput::required(
                "path",
                DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::NewFile))),
            )
            .const_only()
            // Seed an empty path so a freshly-dropped node shows its (const-only)
            // path editor right away; an empty path just means "not caching yet".
            .default(StaticValue::FsPath(String::new())),
        )
        .output("value", DataType::Null)
        .lambda(crate::async_lambda!(|_, _, _, inputs, _, outputs| {
            // The engine does the path-keyed file I/O; the node is a plain
            // passthrough of `input[0]`.
            outputs[0] = inputs[0].value.clone();
            Ok(())
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::special::SpecialNode;

    #[test]
    fn path_input_is_const_only_with_empty_default() {
        let func = SpecialNode::CachePassthrough { bypass: false }.func();
        assert_eq!(func.inputs.len(), 2);

        let path = &func.inputs[CACHE_PATH_INPUT];
        assert_eq!(path.name, "path");
        assert!(path.const_only, "the cache path must reject wired bindings");
        // Seeded empty so a freshly-dropped node shows its path editor.
        assert_eq!(path.default_value, Some(StaticValue::FsPath(String::new())));
    }
}
