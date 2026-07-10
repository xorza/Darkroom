//! Built-in "special" nodes: node kinds with a hardcoded declaration that the
//! engine recognizes by identity, rather than user-registered [`Func`]s.
//!
//! Modeling them as a [`SpecialNode`] enum variant on the node's kind (vs. a flag
//! on every [`Func`]/`Node`) keeps the common path clean: a new special case is a
//! new variant plus a hardcoded spec — its interface + lambda live in an
//! `elements/` module (e.g. [`run_terminals`](crate::elements::run_terminals))
//! — with no new field elsewhere. [`SpecialNode::func`] maps a variant to that
//! interface; the engine then special-cases the node's *behavior* (e.g. the
//! run-terminals promotion in the planner's root collection).

use serde::{Deserialize, Serialize};

use crate::elements::run_terminals::run_terminals_func;
use crate::node::function::Func;

/// A built-in node identified by *kind*, not by a `FuncId`. Its ports + lambda
/// come from [`func`](SpecialNode::func); the engine gives it special behavior.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SpecialNode {
    /// An event sink with no data ports. It computes nothing; when an event it
    /// subscribes to fires, the engine seeds the run with *every* terminal node
    /// (re-running the whole graph) instead of a data cone of its own — the
    /// promotion lives in the planner's root collection. Interface in
    /// [`run_terminals`](crate::elements::run_terminals).
    RunTerminals,
}

/// Every special node (default config), for the editor's node-add menu.
pub const ALL: &[SpecialNode] = &[SpecialNode::RunTerminals];

impl SpecialNode {
    /// This node's hardcoded interface + lambda. Used by flatten (ports, lambda,
    /// behavior), validation (port arity), and the editor (rendering + the node menu).
    pub fn func(self) -> &'static Func {
        match self {
            SpecialNode::RunTerminals => run_terminals_func(),
        }
    }
}
