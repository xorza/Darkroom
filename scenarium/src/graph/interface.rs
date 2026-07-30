//! The interface a node presents: input/output/event arity, declared types,
//! and the declaration flags behind them. Shared by compile-time validation
//! and the editor.

use crate::graph::func::{Func, FuncBehavior, FuncEvent, FuncInput, FuncOutput};
use crate::graph::{Binding, InputPort, NodeId};

/// The ports a node declares, borrowed from whatever declares them: a [`Func`]
/// in the library, or a special node's hardcoded spec. Resolved once by
/// [`Graph::node_ports`](crate::graph::Graph::node_ports) so no caller repeats
/// the per-kind lookup.
#[derive(Clone, Copy, Debug)]
pub struct NodePorts<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub inputs: &'a [FuncInput],
    pub outputs: &'a [FuncOutput],
    pub events: &'a [FuncEvent],
    /// The func this node instantiates — a library entry or a special node's
    /// hardcoded spec. Read the declaration *flags* through
    /// [`sink`](Self::sink) / [`uncacheable`](Self::uncacheable) /
    /// [`impure`](Self::impure) rather than through here.
    pub func: &'a Func,
}

impl NodePorts<'_> {
    /// Whether this node performs sink work — no outputs anything downstream
    /// consumes, so a run has to reach it explicitly.
    pub fn sink(self) -> bool {
        self.func.sink
    }

    /// Whether the node manages its own output caching, so a storage policy
    /// set on it would mean nothing.
    pub fn uncacheable(self) -> bool {
        self.func.uncacheable
    }

    /// Whether this node recomputes every run — it has no content digest, so
    /// no cache mode is honored on it.
    pub fn impure(self) -> bool {
        self.func.behavior == FuncBehavior::Impure
    }

    /// The const bindings a fresh instance starts with: one per input that
    /// declares a default, at that input's port index.
    pub fn default_bindings(self, node_id: NodeId) -> impl Iterator<Item = (InputPort, Binding)> {
        self.inputs
            .iter()
            .enumerate()
            .filter_map(move |(port_idx, input)| {
                let default = input.default_value.clone()?;
                Some((InputPort::new(node_id, port_idx), Binding::Const(default)))
            })
    }
}
