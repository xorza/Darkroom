//! The [`CachePassthrough`](crate::special::SpecialNode::CachePassthrough) special
//! node: a passthrough that caches `output[0]` to the file named by the `Const`
//! `FsPath` in `input[1]`. When that file exists, the value is loaded from it and
//! `input[0]`'s upstream is pruned (not computed). The engine does the path-keyed
//! file I/O; this module only supplies the node's interface + passthrough lambda.
//! See `docs/file-cache-design.md`.

use std::sync::{Arc, OnceLock};

use crate::data::{DataType, FsPathConfig, FsPathMode};
use crate::function::{Func, FuncInput};

/// Input index of a `CachePassthrough` node's `path` (the second input declared by
/// [`cache_passthrough_func`]). The output cache and the path-keyed digest read the
/// path from here — keep it in step with the input order below.
pub(crate) const CACHE_PATH_INPUT: usize = 1;

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
