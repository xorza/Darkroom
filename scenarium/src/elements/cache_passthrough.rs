//! The [`CachePassthrough`](crate::node::special::SpecialNode::CachePassthrough) special
//! node: a passthrough that caches `output[0]` to the file named by the `Const`
//! `FsPath` in `input[1]`. When that file exists, the value is loaded from it and
//! `input[0]`'s upstream is pruned (not computed). The engine does the path-keyed
//! file I/O; this module only supplies the node's interface + passthrough lambda.
//! See `../execution/README.md` Part C.

use std::sync::{Arc, OnceLock};

use crate::data::{DataType, FsPathConfig, FsPathMode, StaticValue};
use crate::node::function::{Func, FuncInput};

/// Stable `FuncId` standing in for the cache node in the flattened program (digest
/// memo / stats attribution). Not registered in any `Library`.
// generated with uuidgen
const CACHE_PASSTHROUGH_FUNC_ID: &str = "2a969ecc-92b7-4136-9c4a-86491c9621d3";

/// The hardcoded interface + passthrough lambda for `CachePassthrough`, built once.
/// The value output is a *wildcard* mirroring the value input (see
/// [`Func::wildcard_output`]), so any type passes through and the editor reports
/// the output as whatever concrete type is wired in.
pub(crate) fn cache_passthrough_func() -> &'static Func {
    static F: OnceLock<Func> = OnceLock::new();
    F.get_or_init(build_func)
}

fn build_func() -> Func {
    Func::new(CACHE_PASSTHROUGH_FUNC_ID, "file cache")
        .category("cache")
        // It owns its caching (explicit-path store), so the editor's generic
        // disk-cache toggle doesn't apply.
        .uncacheable()
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
        .wildcard_output("value", 0)
        // Keyed on the explicit path *alone* (see [`file_cache_digest`], dispatched from
        // `node_digest` on the `CachePassthrough` special kind), so `input[0]` may be
        // impure/expensive and the node still presents a digest — the path is the
        // reproducibility boundary. Presence, not content, decides the disk hit; an
        // empty/absent path ⇒ not cacheable.
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
    use crate::execution::cache_node::CACHE_PATH_INPUT;
    use crate::node::special::SpecialNode;

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

    #[test]
    fn func_is_uncacheable() {
        // It does its own explicit-path caching, so the editor must not offer the
        // generic disk-cache toggle.
        let func = SpecialNode::CachePassthrough { bypass: false }.func();
        assert!(func.uncacheable);
    }
}
