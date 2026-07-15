//! The [`RunSinks`](crate::node::special::SpecialNode::RunSinks) special
//! node: a sink event sink with no data ports. It subscribes to another
//! node's event and, when that event fires, the engine seeds the run with
//! *every* sink node — i.e. re-runs the whole graph — rather than just its
//! own (empty) data cone.
//!
//! It is a `sink` because in this framework only sink nodes are event
//! subscribers (the editor renders a subscription pin for them alone, and
//! `collect_roots` seeds runs from them). A normal sink subscriber runs its
//! own upstream cone when its event fires; this one has no cone, so the planner
//! promotes its firing into a full sink run instead (see
//! `execution::plan::collect_roots`). Its lambda is a no-op — the node is a
//! pure trigger — but it still exists so the scheduled node runs cleanly and
//! lights up in the editor like any other sink. Wire, say, a `Frame Event` into
//! it to re-evaluate the graph every frame.

use std::sync::OnceLock;

use crate::node::definition::Func;

/// Stable `FuncId` standing in for the run-sinks node in the flattened
/// program (stats attribution). Not registered in any `Library`.
const RUN_SINKS_FUNC_ID: &str = "edec890e-5c23-49fb-a131-aaef3844d7c7";

/// The hardcoded interface for `RunSinks`, built once. No inputs, outputs,
/// or events, and a no-op lambda: the node is a pure event-driven trigger whose
/// only effect (running all sinks) is applied by the planner, not the
/// lambda. `sink` so the editor treats it as an event subscriber;
/// `uncacheable` because with no output there is nothing to persist.
pub(crate) fn run_sinks_func() -> &'static Func {
    static F: OnceLock<Func> = OnceLock::new();
    F.get_or_init(build_func)
}

fn build_func() -> Func {
    Func::new(RUN_SINKS_FUNC_ID, "Run on Event")
        .category("System")
        .sink()
        .uncacheable()
        .description(
            "Subscribes to an event and, when it fires, runs every sink \
             node — re-evaluating the whole graph. Has no inputs or outputs; \
             wire an event (e.g. a Frame Event) into it to drive periodic runs.",
        )
        .lambda(crate::async_lambda!(|_, _, _, _, _, _| { Ok(()) }))
}

#[cfg(test)]
mod tests {
    use crate::node::special::SpecialNode;

    #[test]
    fn interface_is_a_portless_sink() {
        let func = SpecialNode::RunSinks.func();
        assert!(func.inputs.is_empty());
        assert!(func.outputs.is_empty());
        assert!(func.events.is_empty());
        // Sink so the editor renders a subscription pin (only sinks
        // subscribe), and scheduled with a real (no-op) lambda.
        assert!(func.sink);
        assert!(!func.lambda.is_none());
        // No output ⇒ the disk-cache toggle is meaningless and hidden.
        assert!(func.uncacheable);
    }
}
