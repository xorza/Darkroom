//! Execution-side logic for the `CachePassthrough` (file-cache) special node: its
//! path-input index, the path resolver, and the path-keyed content digest. These are
//! the halves the engine reads — the digest ([`node_digest`](crate::execution::digest::node_digest))
//! and the output cache — kept here so `execution` doesn't depend upward on `elements`.
//! The node's *interface* (ports + passthrough lambda) lives in
//! [`elements::cache_passthrough`](crate::elements::cache_passthrough). See `README.md` Part C.

use crate::data::StaticValue;
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionBinding, ExecutionInput};

/// Input index of a `CachePassthrough` node's `path` (the second input declared by
/// `cache_passthrough_func`). The output cache and the path-keyed digest read the
/// path from here — keep it in step with that input order.
pub(crate) const CACHE_PATH_INPUT: usize = 1;

/// The `Const` `FsPath` at `node_inputs`'s [`CACHE_PATH_INPUT`] — for a
/// [`CachePassthrough`](crate::node::special::SpecialNode) node, its cache key and
/// load/store location. The single resolver, shared with the output cache, so the
/// path-keyed digest and the load/store can't disagree on the key. `node_inputs` is
/// the node's slice of the program input pool. `None` for a non-const / empty path.
/// Assumes the node is a cache node.
pub(crate) fn cache_node_path(node_inputs: &[ExecutionInput]) -> Option<&str> {
    let path_input = node_inputs.get(CACHE_PATH_INPUT)?;
    let ExecutionBinding::Const(StaticValue::FsPath(path)) = &path_input.binding else {
        return None;
    };
    (!path.is_empty()).then_some(path.as_str())
}

/// A file-cache node's content digest: its `Const` path **alone** ([`cache_node_path`]),
/// under a distinct domain. `input[0]`'s cone is deliberately excluded — the path *is* the
/// reproducibility boundary — so the node presents a digest (and so caches, and lets its
/// upstream be cut) even over an impure/expensive input. `None` for a non-const / empty path
/// ⇒ never cached. Called from [`node_digest`](crate::execution::digest::node_digest).
pub(crate) fn file_cache_digest(node_inputs: &[ExecutionInput]) -> Option<Digest> {
    let path = cache_node_path(node_inputs)?;
    let mut hasher = Digest::hasher();
    hasher
        .write_bytes(b"scenarium-filecache-v1")
        .write_str(path);
    Some(hasher.finish())
}
