//! The [`Preview`](crate::node::special::SpecialNode::Preview) special node: a
//! sink that snapshots its input into its own (disk-cached by default) output,
//! so the editor can preview the value on demand — independent of what the
//! upstream producer's cache is set to.
//!
//! It is a `sink` so a run always schedules it even with nothing wired
//! downstream: the point is to capture and hold the value for review. The single
//! output is a wildcard passthrough mirroring the input, so the node can *also*
//! be spliced onto a wire as an inline tap (input from upstream, output
//! continuing downstream) while still caching + previewing what flows through.
//! Its default [`CacheMode::Disk`] makes the snapshot survive a reopen; the
//! editor's cache chips can dial it down to RAM-only or off.

use std::mem::take;
use std::sync::OnceLock;

use crate::data::DataType;
use crate::graph::CacheMode;
use crate::node::function::{Func, FuncInput};

/// Stable `FuncId` standing in for the preview node in the flattened program
/// (stats attribution + digest identity). Not registered in any `Library`.
const PREVIEW_FUNC_ID: &str = "f0e10336-3978-4021-a7f9-1575bbd7a0d9";

/// The hardcoded interface for `Preview`: one `Any` input, one wildcard output
/// mirroring it, a passthrough lambda, and Disk caching by default. `sink` so a
/// run always reaches it; `pure` so its output is content-cacheable — the cached
/// snapshot is exactly what the editor previews.
pub(crate) fn preview_func() -> &'static Func {
    static F: OnceLock<Func> = OnceLock::new();
    F.get_or_init(build_func)
}

fn build_func() -> Func {
    Func::new(PREVIEW_FUNC_ID, "Preview")
        .category("System")
        .sink()
        .pure()
        .default_cache_mode(CacheMode::Disk)
        .description(
            "Snapshots its input into an own disk-cached output for on-demand \
             preview in the editor — a review sink whose stored value is \
             independent of the upstream node's cache. The output is a \
             passthrough, so it can also sit inline on a wire as a caching tap.",
        )
        .input(
            FuncInput::required("Value", DataType::Any)
                .description("Value of any type to capture for preview."),
        )
        .wildcard_output("Value", 0)
        .lambda(crate::async_lambda!(|_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);
            outputs[0] = take(&mut inputs[0].value);
            Ok(())
        }))
}

#[cfg(test)]
mod tests {
    use crate::graph::CacheMode;
    use crate::node::function::FuncBehavior;
    use crate::node::special::SpecialNode;

    #[test]
    fn interface_is_a_caching_passthrough_sink() {
        let func = SpecialNode::Preview.func();
        assert_eq!(func.inputs.len(), 1);
        assert_eq!(func.outputs.len(), 1);
        assert!(func.events.is_empty());
        // Sink so a run always reaches it (nothing need be wired downstream).
        assert!(func.sink);
        // Pure + cacheable: the snapshot in its output cache is what the editor
        // previews, so — unlike RunSinks — it is NOT `uncacheable`.
        assert!(!func.uncacheable);
        assert_eq!(func.behavior, FuncBehavior::Pure);
        // Persists by default so a captured snapshot survives a reopen.
        assert_eq!(func.default_cache_mode, CacheMode::Disk);
        assert!(!func.lambda.is_none());
    }
}
