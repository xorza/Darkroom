//! Built-in "special" nodes: node kinds with a hardcoded declaration that the
//! engine recognizes by identity, rather than user-registered [`Func`]s.
//!
//! Modeling them as a [`SpecialNode`] enum variant on the node's kind (vs. a flag
//! on every [`Func`]/`Node`) keeps the common path clean: a new special case is a
//! new variant plus a hardcoded spec — its interface + lambda live in an
//! `elements/` module (e.g. [`run_sinks`](crate::elements::run_sinks))
//! — with no new field elsewhere. [`SpecialNode::func`] maps a variant to that
//! interface; the engine then special-cases the node's *behavior* (e.g. the
//! run-sinks promotion in the planner's root collection).

use serde::{Deserialize, Serialize};

use crate::elements::run_sinks::run_sinks_func;
use crate::node::definition::Func;

/// A built-in node identified by *kind*, not by a `FuncId`. Its ports + lambda
/// come from [`func`](SpecialNode::func); the engine gives it special behavior.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SpecialNode {
    /// An event sink with no data ports. It computes nothing; when an event it
    /// subscribes to fires, the engine seeds the run with *every* sink node
    /// (re-running the whole graph) instead of a data cone of its own — the
    /// promotion lives in the planner's root collection. Interface in
    /// [`run_sinks`](crate::elements::run_sinks).
    RunSinks,
}

/// Every special node (default config), for the editor's node-add menu.
pub const SPECIAL_NODES: &[SpecialNode] = &[SpecialNode::RunSinks];

impl SpecialNode {
    /// This node's hardcoded interface + lambda. Used by flatten (ports, lambda,
    /// behavior), validation (port arity), and the editor (rendering + the node menu).
    pub fn func(self) -> &'static Func {
        match self {
            SpecialNode::RunSinks => run_sinks_func(),
        }
    }
}
