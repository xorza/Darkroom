//! The [`RunTerminals`](crate::node::special::SpecialNode::RunTerminals) special
//! node: a terminal event sink with no data ports. It subscribes to another
//! node's event and, when that event fires, the engine seeds the run with
//! *every* terminal node — i.e. re-runs the whole graph — rather than just its
//! own (empty) data cone.
//!
//! It is a `terminal` because in this framework only terminal nodes are event
//! subscribers (the editor renders a subscription pin for them alone, and
//! `collect_roots` seeds runs from them). A normal terminal subscriber runs its
//! own upstream cone when its event fires; this one has no cone, so the planner
//! promotes its firing into a full terminal run instead (see
//! `execution::plan::collect_roots`). Its lambda is a no-op — the node is a
//! pure trigger — but it still exists so the scheduled node runs cleanly and
//! lights up in the editor like any other sink. Wire, say, a `Frame Event` into
//! it to re-evaluate the graph every frame.

use std::sync::OnceLock;

use crate::node::function::Func;

/// Stable `FuncId` standing in for the run-terminals node in the flattened
/// program (stats attribution). Not registered in any `Library`.
const RUN_TERMINALS_FUNC_ID: &str = "edec890e-5c23-49fb-a131-aaef3844d7c7";

/// The hardcoded interface for `RunTerminals`, built once. No inputs, outputs,
/// or events, and a no-op lambda: the node is a pure event-driven trigger whose
/// only effect (running all terminals) is applied by the planner, not the
/// lambda. `terminal` so the editor treats it as an event subscriber;
/// `uncacheable` because with no output there is nothing to persist.
pub(crate) fn run_terminals_func() -> &'static Func {
    static F: OnceLock<Func> = OnceLock::new();
    F.get_or_init(build_func)
}

fn build_func() -> Func {
    Func::new(RUN_TERMINALS_FUNC_ID, "Run on Event")
        .category("System")
        .terminal()
        .uncacheable()
        .description(
            "Subscribes to an event and, when it fires, runs every terminal \
             node — re-evaluating the whole graph. Has no inputs or outputs; \
             wire an event (e.g. a Frame Event) into it to drive periodic runs.",
        )
        .lambda(crate::async_lambda!(|_, _, _, _, _, _| { Ok(()) }))
}

#[cfg(test)]
mod tests {
    use crate::node::special::SpecialNode;

    #[test]
    fn interface_is_a_portless_terminal() {
        let func = SpecialNode::RunTerminals.func();
        assert!(func.inputs.is_empty());
        assert!(func.outputs.is_empty());
        assert!(func.events.is_empty());
        // Terminal so the editor renders a subscription pin (only terminals
        // subscribe), and scheduled with a real (no-op) lambda.
        assert!(func.terminal);
        assert!(!func.lambda.is_none());
        // No output ⇒ the disk-cache toggle is meaningless and hidden.
        assert!(func.uncacheable);
    }
}
