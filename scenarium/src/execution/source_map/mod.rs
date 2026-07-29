//! Attribution as the artifact answers it: which authored node, and which
//! enclosing instances, each execution node came from.
//!
//! The relation is flatten's — only the walk sees a composite before it
//! dissolves — but this is the *placed* form of it, built during linking and held by the
//! [`CompiledGraph`](crate::execution::compiled::CompiledGraph) for as long as
//! the install lives. So
//! it is stored the way everything else the artifact keeps per node is: a
//! column in the program's index space, read by offset. The host's stable id
//! becomes an index through the program's own id map, and nothing here is keyed
//! by id.

use thiserror::Error;

use crate::common::column::Column;
use crate::execution::identity::NodeIdx;
use crate::graph::identity::NodeId;

/// One graph instance the flatten walk descended through, and the scope
/// enclosing it.
#[derive(Debug, Clone, Copy)]
struct Scope {
    instance: Option<NodeId>,
    parent: u32,
}

/// The instance ancestry every [`Leaf`] points into. Scope 0 is the entry
/// graph, which no instance encloses and therefore ends any walk outward;
/// every other scope is opened under one that already exists, so parents
/// strictly precede their children.
#[derive(Debug)]
pub(crate) struct ScopeTable {
    scopes: Vec<Scope>,
}

impl ScopeTable {
    /// A table holding only the entry graph — where every flatten walk starts.
    pub(crate) fn open() -> Self {
        ScopeTable {
            scopes: vec![Scope {
                instance: None,
                parent: 0,
            }],
        }
    }

    /// Open a scope for `instance` under `parent`, returning its index.
    pub(crate) fn push(&mut self, instance: NodeId, parent: u32) -> u32 {
        let idx = u32::try_from(self.scopes.len()).expect("flatten scope count exceeds u32");
        self.scopes.push(Scope {
            instance: Some(instance),
            parent,
        });
        idx
    }

    fn len(&self) -> usize {
        self.scopes.len()
    }

    /// The instance this scope names, or `None` for the entry graph.
    fn instance(&self, scope: u32) -> Option<NodeId> {
        self.scopes[scope as usize].instance
    }

    fn parent(&self, scope: u32) -> u32 {
        self.scopes[scope as usize].parent
    }
}

/// The authored node behind one flat node, and the scope it was reached
/// through. Flatten records it in the same step that emits the node; linking
/// places it beside that node in the final index space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Leaf {
    pub(crate) scope: u32,
    pub(crate) node_id: NodeId,
}

/// Self-consistency of the two index spaces attribution reads unchecked.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum AttributionValidationError {
    #[error("attribution spans {len} nodes, not the program's {expected}")]
    LeafCount { len: usize, expected: usize },
    #[error("node {node_idx:?} is attributed to out-of-range scope {scope}")]
    LeafScope { node_idx: NodeIdx, scope: u32 },
    #[error("scope {scope} has parent {parent}, which does not precede it")]
    ScopeParent { scope: u32, parent: u32 },
}

/// One leaf per execution node, over the scope chain naming its enclosing
/// instances.
#[derive(Debug)]
pub(crate) struct Attribution {
    scopes: ScopeTable,
    leaves: Column<NodeIdx, Leaf>,
}

impl Default for Attribution {
    /// The empty program's attribution — nothing to attribute, over the lone
    /// entry-graph scope every table opens with.
    fn default() -> Self {
        Attribution {
            scopes: ScopeTable::open(),
            leaves: Column::default(),
        }
    }
}

impl Attribution {
    /// Place the walk's leaves in the program's index space. `leaves` is in
    /// adoption order — linking sorts one vector to fix both.
    pub(super) fn new(scopes: ScopeTable, leaves: Column<NodeIdx, Leaf>) -> Self {
        Attribution { scopes, leaves }
    }

    /// The authored leaf behind one execution node, then each enclosing graph
    /// instance, innermost first.
    pub(crate) fn of(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeId> + '_ {
        let leaf = self.leaves[node_idx];
        // The parent chain is walked to the entry graph, which carries no
        // instance and so ends the `map_while` — the only thing that ends it.
        let enclosing =
            std::iter::successors(Some(leaf.scope), move |&at| Some(self.scopes.parent(at)))
                .map_while(move |at| self.scopes.instance(at));
        std::iter::once(leaf.node_id).chain(enclosing)
    }

    /// Self-consistency against the program of `node_count` nodes this column
    /// is aligned to. Both index spaces it reads are checked here, because
    /// [`of`](Self::of) indexes into them unchecked — and its walk *diverges*
    /// rather than faulting on a parent that doesn't descend, so that is
    /// checked too.
    pub(crate) fn validate(&self, node_count: usize) -> Result<(), AttributionValidationError> {
        if self.leaves.len() != node_count {
            return Err(AttributionValidationError::LeafCount {
                len: self.leaves.len(),
                expected: node_count,
            });
        }
        // Every scope but the entry graph is opened under one that already
        // exists, so parents strictly descend and the chain terminates at 0.
        for scope in 1..self.scopes.len() as u32 {
            let parent = self.scopes.parent(scope);
            if parent >= scope {
                return Err(AttributionValidationError::ScopeParent { scope, parent });
            }
        }
        for (node_idx, leaf) in self.leaves.iter_indexed() {
            if leaf.scope as usize >= self.scopes.len() {
                return Err(AttributionValidationError::LeafScope {
                    node_idx,
                    scope: leaf.scope,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
